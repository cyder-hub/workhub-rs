#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT_DIR"

choose_port() {
    python3 - <<'PY' 2>/dev/null || printf '18080\n'
import socket

with socket.socket(socket.AF_INET, socket.SOCK_STREAM) as sock:
    sock.bind(("127.0.0.1", 0))
    print(sock.getsockname()[1])
PY
}

PORT="${MCP_SMOKE_PORT:-$(choose_port)}"
MCP_PATH="${MCP_SMOKE_PATH:-/stage1-mcp}"
case "$MCP_PATH" in
    /*) ;;
    *) MCP_PATH="/$MCP_PATH" ;;
esac

cargo build --quiet --bin mcp-atlassian-rs

TMP_DIR="$(mktemp -d)"
SERVER_LOG="$TMP_DIR/server.log"
SERVER_PID=""

cleanup() {
    if [[ -n "$SERVER_PID" ]] && kill -0 "$SERVER_PID" 2>/dev/null; then
        kill "$SERVER_PID" 2>/dev/null || true
        wait "$SERVER_PID" 2>/dev/null || true
    fi
    rm -rf "$TMP_DIR"
}
trap cleanup EXIT

env \
    -u ENABLED_TOOLS \
    -u TOOLSETS \
    -u JIRA_URL \
    -u CONFLUENCE_URL \
    READ_ONLY_MODE=false \
    "$ROOT_DIR/target/debug/mcp-atlassian-rs" streamhttp \
        --host 127.0.0.1 \
        --port "$PORT" \
        --path "$MCP_PATH" \
        >"$SERVER_LOG" 2>&1 &
SERVER_PID="$!"

HEALTH_OK=false
for _ in $(seq 1 50); do
    if curl --fail --silent "http://127.0.0.1:$PORT/healthz" >"$TMP_DIR/healthz.json" 2>/dev/null; then
        HEALTH_OK=true
        break
    fi
    if ! kill -0 "$SERVER_PID" 2>/dev/null; then
        echo "streamable HTTP server exited before /healthz became ready" >&2
        cat "$SERVER_LOG" >&2 || true
        exit 1
    fi
    sleep 0.1
done

if [[ "$HEALTH_OK" != true ]]; then
    echo "timed out waiting for /healthz on 127.0.0.1:$PORT" >&2
    cat "$SERVER_LOG" >&2 || true
    exit 1
fi

python3 - "$TMP_DIR/healthz.json" <<'PY'
import json
import sys

with open(sys.argv[1], "r", encoding="utf-8") as handle:
    data = json.load(handle)
if data != {"status": "ok"}:
    raise SystemExit(f"unexpected /healthz body: {data!r}")
PY

post_mcp() {
    local body_file="$1"
    local header_file="$2"
    local output_file="$3"
    shift 3

    curl --fail --silent --show-error \
        --dump-header "$header_file" \
        --output "$output_file" \
        --request POST \
        --header "Content-Type: application/json" \
        --header "Accept: application/json, text/event-stream" \
        "$@" \
        --data-binary "@$body_file" \
        "http://127.0.0.1:$PORT$MCP_PATH"
}

cat >"$TMP_DIR/initialize.json" <<'JSON'
{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-03-26","capabilities":{},"clientInfo":{"name":"stage1-http-smoke","version":"0.1.0"}}}
JSON
post_mcp "$TMP_DIR/initialize.json" "$TMP_DIR/initialize.headers" "$TMP_DIR/initialize.body"

SESSION_ID="$(python3 - "$TMP_DIR/initialize.headers" <<'PY'
import sys

for line in open(sys.argv[1], "r", encoding="utf-8"):
    if line.lower().startswith("mcp-session-id:"):
        print(line.split(":", 1)[1].strip())
        break
else:
    raise SystemExit("Mcp-Session-Id header missing from initialize response")
PY
)"

python3 - "$TMP_DIR/initialize.body" 1 <<'PY'
import json
import sys

def sse_messages(path):
    with open(path, "r", encoding="utf-8") as handle:
        for line in handle:
            if line.startswith("data:"):
                data = line.split(":", 1)[1].strip()
                if data:
                    yield json.loads(data)

messages = list(sse_messages(sys.argv[1]))
expected_id = int(sys.argv[2])
if not any(message.get("id") == expected_id and "result" in message for message in messages):
    raise SystemExit(f"expected JSON-RPC result id {expected_id} in SSE body: {messages!r}")
PY

cat >"$TMP_DIR/initialized.json" <<'JSON'
{"jsonrpc":"2.0","method":"notifications/initialized","params":{}}
JSON
post_mcp \
    "$TMP_DIR/initialized.json" \
    "$TMP_DIR/initialized.headers" \
    "$TMP_DIR/initialized.body" \
    --header "Mcp-Session-Id: $SESSION_ID"

cat >"$TMP_DIR/tools-list.json" <<'JSON'
{"jsonrpc":"2.0","id":2,"method":"tools/list","params":{}}
JSON
post_mcp \
    "$TMP_DIR/tools-list.json" \
    "$TMP_DIR/tools-list.headers" \
    "$TMP_DIR/tools-list.body" \
    --header "Mcp-Session-Id: $SESSION_ID"

python3 - "$TMP_DIR/tools-list.body" <<'PY'
import json
import sys

messages = []
with open(sys.argv[1], "r", encoding="utf-8") as handle:
    for line in handle:
        if line.startswith("data:"):
            data = line.split(":", 1)[1].strip()
            if data:
                messages.append(json.loads(data))

for message in messages:
    if message.get("id") == 2:
        tools = message.get("result", {}).get("tools", [])
        names = {tool.get("name") for tool in tools}
        if "migration_status" in names:
            print("HTTP smoke passed: /healthz ok and migration_status is discoverable")
            break
        raise SystemExit(f"migration_status missing from HTTP tools/list: {message!r}")
else:
    raise SystemExit(f"tools/list response missing from SSE body: {messages!r}")
PY
