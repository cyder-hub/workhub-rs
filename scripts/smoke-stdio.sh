#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT_DIR"

cargo build --quiet --bin mcp-atlassian-rs

python3 - "$ROOT_DIR/target/debug/mcp-atlassian-rs" <<'PY'
import json
import os
import select
import subprocess
import sys
import time

binary = sys.argv[1]

env = os.environ.copy()
for key in (
    "ENABLED_TOOLS",
    "TOOLSETS",
    "JIRA_URL",
    "CONFLUENCE_URL",
    "MCP_HTTP_HOST",
    "MCP_HTTP_PORT",
    "MCP_HTTP_PATH",
):
    env.pop(key, None)
env["READ_ONLY_MODE"] = "false"

proc = subprocess.Popen(
    [binary, "stdio"],
    stdin=subprocess.PIPE,
    stdout=subprocess.PIPE,
    stderr=subprocess.DEVNULL,
    text=True,
    bufsize=1,
    env=env,
)

def stop() -> None:
    if proc.stdin and not proc.stdin.closed:
        proc.stdin.close()
    try:
        proc.wait(timeout=2)
    except subprocess.TimeoutExpired:
        proc.terminate()
        try:
            proc.wait(timeout=2)
        except subprocess.TimeoutExpired:
            proc.kill()
            proc.wait(timeout=2)

def send(message: dict) -> None:
    assert proc.stdin is not None
    proc.stdin.write(json.dumps(message, separators=(",", ":")) + "\n")
    proc.stdin.flush()

def read_response(expected_id: int, timeout: float = 5.0) -> dict:
    assert proc.stdout is not None
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        ready, _, _ = select.select([proc.stdout], [], [], max(0.0, deadline - time.monotonic()))
        if not ready:
            continue
        line = proc.stdout.readline()
        if line == "":
            raise RuntimeError("stdio server closed before response")
        message = json.loads(line)
        if message.get("id") == expected_id:
            return message
    raise RuntimeError(f"timed out waiting for JSON-RPC response id {expected_id}")

try:
    send({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "initialize",
        "params": {
            "protocolVersion": "2025-03-26",
            "capabilities": {},
            "clientInfo": {"name": "stage1-stdio-smoke", "version": "0.1.0"},
        },
    })
    initialize = read_response(1)
    if "result" not in initialize:
        raise RuntimeError(f"initialize failed: {initialize}")

    send({"jsonrpc": "2.0", "method": "notifications/initialized", "params": {}})

    send({"jsonrpc": "2.0", "id": 2, "method": "tools/list", "params": {}})
    tool_list = read_response(2)
    tools = tool_list.get("result", {}).get("tools", [])
    names = {tool.get("name") for tool in tools}
    if "migration_status" not in names:
        raise RuntimeError(f"migration_status missing from stdio tools/list: {tool_list}")

    send({
        "jsonrpc": "2.0",
        "id": 3,
        "method": "tools/call",
        "params": {"name": "migration_status", "arguments": {}},
    })
    tool_call = read_response(3)
    if tool_call.get("result", {}).get("isError") is True:
        raise RuntimeError(f"migration_status call returned an error: {tool_call}")
    body = json.dumps(tool_call, sort_keys=True)
    if "Stage 1 shared MCP runtime and control plane is complete" not in body:
        raise RuntimeError(f"migration_status call did not return Stage 1 status text: {tool_call}")

    print("stdio smoke passed: migration_status is discoverable and callable")
finally:
    stop()
PY
