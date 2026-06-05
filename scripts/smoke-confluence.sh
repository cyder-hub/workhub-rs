#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT_DIR"

MODE="${1:-all}"
case "$MODE" in
    all | stdio | http | read-only) ;;
    *)
        echo "usage: $0 [all|stdio|http|read-only]" >&2
        exit 2
        ;;
esac

cargo build --quiet --bin mcp-atlassian-rs

python3 - "$ROOT_DIR/target/debug/mcp-atlassian-rs" "$MODE" "${MCP_SMOKE_PORT:-}" "${MCP_SMOKE_PATH:-/mcp}" <<'PY'
import http.client
import json
import os
import select
import socket
import subprocess
import sys
import tempfile
import threading
import time
import urllib.parse
import urllib.request
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

binary = sys.argv[1]
mode = sys.argv[2]
requested_http_port = sys.argv[3]
requested_mcp_path = sys.argv[4]


class MockConfluenceHandler(BaseHTTPRequestHandler):
    expected_token = "test-confluence-smoke-token"
    requests = []
    lock = threading.Lock()

    def log_message(self, format, *args):
        return

    def record(self, body=None):
        with self.lock:
            self.requests.append(
                {
                    "method": self.command,
                    "path": self.path,
                    "body": body,
                }
            )

    def read_json_body(self):
        length = int(self.headers.get("Content-Length", "0") or "0")
        if length == 0:
            return None
        raw = self.rfile.read(length)
        return json.loads(raw.decode("utf-8"))

    def send_json(self, status, payload):
        raw = json.dumps(payload, separators=(",", ":")).encode("utf-8")
        self.send_response(status)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(raw)))
        self.end_headers()
        self.wfile.write(raw)

    def is_authorized(self):
        expected = f"Bearer {self.expected_token}"
        return self.headers.get("Authorization") == expected

    def require_authorized(self):
        if self.is_authorized():
            return True
        self.send_json(401, {"errorMessages": ["mock auth failed"]})
        return False

    def do_GET(self):
        self.record()
        if not self.require_authorized():
            return

        path = urllib.parse.urlsplit(self.path).path
        if path == "/rest/api/content/search":
            self.send_json(
                200,
                {
                    "results": [
                        {
                            "id": "123",
                            "title": "Roadmap",
                            "type": "page",
                            "content": {
                                "id": "123",
                                "title": "Roadmap",
                                "type": "page",
                                "space": {"key": "ENG", "name": "Engineering"},
                                "_links": {"webui": "/spaces/ENG/pages/123/Roadmap"},
                            },
                            "space": {"key": "ENG", "name": "Engineering"},
                            "url": "/spaces/ENG/pages/123/Roadmap",
                            "excerpt": "Smoke page",
                        }
                    ],
                    "start": 0,
                    "limit": 10,
                    "size": 1,
                    "_links": {},
                },
            )
            return
        if path == "/rest/api/content/123":
            self.send_json(
                200,
                {
                    "id": "123",
                    "title": "Roadmap",
                    "type": "page",
                    "status": "current",
                    "space": {"key": "ENG", "name": "Engineering"},
                    "body": {
                        "storage": {
                            "value": "<h1>Roadmap</h1><p>Smoke page</p>",
                            "representation": "storage",
                        }
                    },
                    "version": {"number": 3},
                    "ancestors": [{"id": "100", "title": "Home"}],
                    "metadata": {"labels": {"results": [{"name": "smoke"}]}},
                    "_links": {"webui": "/spaces/ENG/pages/123/Roadmap"},
                },
            )
            return

        self.send_json(404, {"errorMessages": ["mock path not found"]})

    def do_POST(self):
        body = self.read_json_body()
        self.record(body)
        if not self.require_authorized():
            return

        self.send_json(404, {"errorMessages": ["mock write path not expected"]})


def free_port():
    with socket.socket(socket.AF_INET, socket.SOCK_STREAM) as sock:
        sock.bind(("127.0.0.1", 0))
        return sock.getsockname()[1]


def start_mock_confluence():
    with MockConfluenceHandler.lock:
        MockConfluenceHandler.requests = []
    server = ThreadingHTTPServer(("127.0.0.1", 0), MockConfluenceHandler)
    thread = threading.Thread(target=server.serve_forever, daemon=True)
    thread.start()
    return server, f"http://127.0.0.1:{server.server_address[1]}"


def clean_env(confluence_url, read_only):
    env = os.environ.copy()
    for key in (
        "ENABLED_TOOLS",
        "TOOLSETS",
        "JIRA_URL",
        "JIRA_USERNAME",
        "JIRA_API_TOKEN",
        "JIRA_PERSONAL_TOKEN",
        "JIRA_SSL_VERIFY",
        "JIRA_PROJECTS_FILTER",
        "JIRA_TIMEOUT",
        "CONFLUENCE_URL",
        "CONFLUENCE_USERNAME",
        "CONFLUENCE_API_TOKEN",
        "CONFLUENCE_PERSONAL_TOKEN",
        "CONFLUENCE_SSL_VERIFY",
        "CONFLUENCE_SPACES_FILTER",
        "CONFLUENCE_TIMEOUT",
        "MCP_HTTP_HOST",
        "MCP_HTTP_PORT",
        "MCP_HTTP_PATH",
        "ATLASSIAN_OAUTH_CLOUD_ID",
    ):
        env.pop(key, None)
    env["CONFLUENCE_URL"] = confluence_url
    env["CONFLUENCE_PERSONAL_TOKEN"] = MockConfluenceHandler.expected_token
    env["CONFLUENCE_SSL_VERIFY"] = "false"
    env["READ_ONLY_MODE"] = "true" if read_only else "false"
    env["TOOLSETS"] = "default"
    return env


def stop_process(proc):
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


def send_stdio(proc, message):
    assert proc.stdin is not None
    proc.stdin.write(json.dumps(message, separators=(",", ":")) + "\n")
    proc.stdin.flush()


def read_stdio_response(proc, expected_id, timeout=5.0):
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


def initialize_stdio(proc, name):
    send_stdio(
        proc,
        {
            "jsonrpc": "2.0",
            "id": 1,
            "method": "initialize",
            "params": {
                "protocolVersion": "2025-03-26",
                "capabilities": {},
                "clientInfo": {"name": name, "version": "0.1.0"},
            },
        },
    )
    initialize = read_stdio_response(proc, 1)
    if "result" not in initialize:
        raise RuntimeError(f"initialize failed: {initialize}")
    send_stdio(proc, {"jsonrpc": "2.0", "method": "notifications/initialized", "params": {}})


def list_stdio_tools(proc):
    send_stdio(proc, {"jsonrpc": "2.0", "id": 2, "method": "tools/list", "params": {}})
    tool_list = read_stdio_response(proc, 2)
    tools = tool_list.get("result", {}).get("tools", [])
    return {tool.get("name") for tool in tools}


def call_stdio_tool(proc, request_id, name, arguments):
    send_stdio(
        proc,
        {
            "jsonrpc": "2.0",
            "id": request_id,
            "method": "tools/call",
            "params": {"name": name, "arguments": arguments},
        },
    )
    return read_stdio_response(proc, request_id)


def assert_tool_success(response, tool_name):
    if "error" in response:
        raise RuntimeError(f"{tool_name} returned JSON-RPC error: {response}")
    result = response.get("result", {})
    if result.get("isError") is True:
        raise RuntimeError(f"{tool_name} returned tool error: {response}")
    structured = result.get("structuredContent")
    if structured is None:
        raise RuntimeError(f"{tool_name} returned no structuredContent: {response}")
    return structured


def assert_search_result(response):
    structured = assert_tool_success(response, "confluence_search")
    results = structured.get("results") or []
    if not results or results[0].get("title") != "Roadmap":
        raise RuntimeError(f"confluence_search did not return mock page: {response}")


def assert_page_result(response):
    structured = assert_tool_success(response, "confluence_get_page")
    metadata = structured.get("metadata") or {}
    if metadata.get("id") != "123" or metadata.get("title") != "Roadmap":
        raise RuntimeError(f"confluence_get_page did not return mock page: {response}")


def run_stdio(confluence_url):
    proc = subprocess.Popen(
        [binary, "stdio"],
        stdin=subprocess.PIPE,
        stdout=subprocess.PIPE,
        stderr=subprocess.DEVNULL,
        text=True,
        bufsize=1,
        env=clean_env(confluence_url, read_only=False),
    )
    try:
        initialize_stdio(proc, "stage4-confluence-stdio-smoke")
        names = list_stdio_tools(proc)
        required_tools = {
            "confluence_search",
            "confluence_get_page",
            "confluence_create_page",
        }
        missing = required_tools - names
        if missing:
            raise RuntimeError(f"stdio tools/list missing {sorted(missing)}: {sorted(names)}")
        response = call_stdio_tool(
            proc,
            3,
            "confluence_search",
            {"query": "project docs", "limit": 10, "spaces_filter": "ENG"},
        )
        assert_search_result(response)
        response = call_stdio_tool(
            proc,
            4,
            "confluence_get_page",
            {"page_id": "123", "include_metadata": True, "convert_to_markdown": True},
        )
        assert_page_result(response)
        print("Confluence stdio smoke passed: search and get_page work with mock Confluence")
    finally:
        stop_process(proc)


def normalize_mcp_path(path):
    path = path.strip() or "/mcp"
    if not path.startswith("/"):
        path = "/" + path
    return path


def parse_sse_messages(body):
    stripped = body.lstrip()
    if stripped.startswith("{"):
        return [json.loads(stripped)]

    messages = []
    for line in body.splitlines():
        if line.startswith("data:"):
            payload = line.split(":", 1)[1].strip()
            if payload:
                messages.append(json.loads(payload))
    return messages


def post_mcp(port, path, message, session_id=None):
    body = json.dumps(message, separators=(",", ":")).encode("utf-8")
    headers = {
        "Content-Type": "application/json",
        "Accept": "application/json, text/event-stream",
    }
    if session_id:
        headers["Mcp-Session-Id"] = session_id

    connection = http.client.HTTPConnection("127.0.0.1", port, timeout=5)
    try:
        connection.request("POST", path, body=body, headers=headers)
        response = connection.getresponse()
        response_body = response.read().decode("utf-8")
        response_headers = {key.lower(): value for key, value in response.getheaders()}
        if response.status >= 400:
            raise RuntimeError(f"MCP HTTP {response.status}: {response_body}")
        return response.status, response_headers, response_body
    finally:
        connection.close()


def expect_rpc_result(body, expected_id):
    messages = parse_sse_messages(body)
    for message in messages:
        if message.get("id") == expected_id:
            if "result" not in message:
                raise RuntimeError(f"expected JSON-RPC result id {expected_id}: {message}")
            return message
    raise RuntimeError(f"expected JSON-RPC result id {expected_id} in body: {body!r}")


def wait_health(port, log_path):
    url = f"http://127.0.0.1:{port}/healthz"
    deadline = time.monotonic() + 5
    while time.monotonic() < deadline:
        try:
            with urllib.request.urlopen(url, timeout=0.5) as response:
                payload = json.loads(response.read().decode("utf-8"))
            if payload == {"status": "ok"}:
                return
            raise RuntimeError(f"unexpected /healthz body: {payload!r}")
        except Exception:
            time.sleep(0.1)
    with open(log_path, "r", encoding="utf-8") as handle:
        log = handle.read()
    raise RuntimeError(f"timed out waiting for /healthz on {url}\n{log}")


def run_http(confluence_url):
    port = int(requested_http_port) if requested_http_port else free_port()
    mcp_path = normalize_mcp_path(requested_mcp_path)
    with tempfile.TemporaryDirectory() as tmp_dir:
        log_path = os.path.join(tmp_dir, "server.log")
        with open(log_path, "w", encoding="utf-8") as log:
            proc = subprocess.Popen(
                [
                    binary,
                    "streamhttp",
                    "--host",
                    "127.0.0.1",
                    "--port",
                    str(port),
                    "--path",
                    mcp_path,
                ],
                stdout=log,
                stderr=log,
                env=clean_env(confluence_url, read_only=False),
            )
            try:
                wait_health(port, log_path)
                _, headers, body = post_mcp(
                    port,
                    mcp_path,
                    {
                        "jsonrpc": "2.0",
                        "id": 1,
                        "method": "initialize",
                        "params": {
                            "protocolVersion": "2025-03-26",
                            "capabilities": {},
                            "clientInfo": {"name": "stage4-confluence-http-smoke", "version": "0.1.0"},
                        },
                    },
                )
                expect_rpc_result(body, 1)
                session_id = headers.get("mcp-session-id")
                if not session_id:
                    raise RuntimeError("Mcp-Session-Id header missing from initialize response")

                post_mcp(
                    port,
                    mcp_path,
                    {"jsonrpc": "2.0", "method": "notifications/initialized", "params": {}},
                    session_id=session_id,
                )
                _, _, body = post_mcp(
                    port,
                    mcp_path,
                    {"jsonrpc": "2.0", "id": 2, "method": "tools/list", "params": {}},
                    session_id=session_id,
                )
                tools_message = expect_rpc_result(body, 2)
                names = {
                    tool.get("name")
                    for tool in tools_message.get("result", {}).get("tools", [])
                }
                required_tools = {
                    "confluence_search",
                    "confluence_get_page",
                    "confluence_create_page",
                }
                missing = required_tools - names
                if missing:
                    raise RuntimeError(f"HTTP tools/list missing {sorted(missing)}: {sorted(names)}")
                _, _, body = post_mcp(
                    port,
                    mcp_path,
                    {
                        "jsonrpc": "2.0",
                        "id": 3,
                        "method": "tools/call",
                        "params": {
                            "name": "confluence_search",
                            "arguments": {"query": "project docs", "limit": 10, "spaces_filter": "ENG"},
                        },
                    },
                    session_id=session_id,
                )
                search_message = expect_rpc_result(body, 3)
                assert_search_result(search_message)
                _, _, body = post_mcp(
                    port,
                    mcp_path,
                    {
                        "jsonrpc": "2.0",
                        "id": 4,
                        "method": "tools/call",
                        "params": {
                            "name": "confluence_get_page",
                            "arguments": {
                                "page_id": "123",
                                "include_metadata": True,
                                "convert_to_markdown": True,
                            },
                        },
                    },
                    session_id=session_id,
                )
                page_message = expect_rpc_result(body, 4)
                assert_page_result(page_message)
                print("Confluence HTTP smoke passed: /healthz ok and search/get_page work with mock Confluence")
            finally:
                proc.terminate()
                try:
                    proc.wait(timeout=2)
                except subprocess.TimeoutExpired:
                    proc.kill()
                    proc.wait(timeout=2)


def run_read_only(confluence_url):
    proc = subprocess.Popen(
        [binary, "stdio"],
        stdin=subprocess.PIPE,
        stdout=subprocess.PIPE,
        stderr=subprocess.DEVNULL,
        text=True,
        bufsize=1,
        env=clean_env(confluence_url, read_only=True),
    )
    try:
        initialize_stdio(proc, "stage4-confluence-read-only-smoke")
        names = list_stdio_tools(proc)
        if "confluence_search" not in names:
            raise RuntimeError(f"confluence_search missing in read-only mode: {sorted(names)}")
        if "confluence_get_page" not in names:
            raise RuntimeError(f"confluence_get_page missing in read-only mode: {sorted(names)}")
        if "confluence_create_page" in names:
            raise RuntimeError(f"confluence_create_page should be hidden in read-only mode: {sorted(names)}")
        response = call_stdio_tool(
            proc,
            3,
            "confluence_create_page",
            {
                "space_key": "ENG",
                "title": "Blocked smoke page",
                "content": "blocked by read-only smoke",
                "content_format": "markdown",
            },
        )
        error = response.get("error", {})
        if "read-only" not in error.get("message", ""):
            raise RuntimeError(f"confluence_create_page was not blocked by read-only guard: {response}")
        with MockConfluenceHandler.lock:
            write_requests = [
                request
                for request in MockConfluenceHandler.requests
                if request["method"] == "POST" and request["path"].split("?", 1)[0] == "/rest/api/content"
            ]
        if write_requests:
            raise RuntimeError(f"read-only Confluence write tool reached mock Confluence: {write_requests!r}")
        print("Confluence read-only smoke passed: reads stay visible and confluence_create_page is blocked before HTTP")
    finally:
        stop_process(proc)


mock_server, mock_url = start_mock_confluence()
try:
    if mode in ("all", "stdio"):
        run_stdio(mock_url)
    if mode in ("all", "http"):
        run_http(mock_url)
    if mode in ("all", "read-only"):
        run_read_only(mock_url)
finally:
    mock_server.shutdown()
    mock_server.server_close()
PY
