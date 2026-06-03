# mcp-atlassian-rs

Rust migration workspace for MCP Atlassian.

This repository is migrating the Python `mcp-atlassian` Jira and Confluence MCP server to a Rust-native implementation. The current Rust binary has completed the Stage 1 shared MCP runtime and control plane. Jira and Confluence business tools are not available yet.

## Current Status

Implemented in the Rust root project:

- Package, binary, server name, Docker image, compose service, and CI image identity use `mcp-atlassian-rs`.
- MCP server runs over `stdio` and streamable HTTP at `/mcp`.
- Logging is configured to stderr so stdio MCP stdout remains protocol-only.
- Runtime control-plane config parses `READ_ONLY_MODE`, `ENABLED_TOOLS`, `TOOLSETS`, `JIRA_URL`, `CONFLUENCE_URL`, `MCP_HTTP_HOST`, `MCP_HTTP_PORT`, and `MCP_HTTP_PATH`.
- Tool registry metadata, service availability filtering, toolset filtering, enabled-tools filtering, and read-only write guards are in place for migrated tools.
- Streamable HTTP exposes `GET /healthz`.
- Local stdio and streamable HTTP smoke commands validate MCP initialization and `migration_status` discovery.
- The temporary MCP tool `migration_status` reports the migration state.
- Dockerfile, compose file, just commands, and CI metadata build the Rust binary.

Not implemented yet:

- Atlassian configuration, authentication, HTTP client, or API models.
- Jira and Confluence MCP tools.
- Real Jira or Confluence smoke tests.

## Requirements

- Rust 1.94 or newer
- just
- curl and Python 3 when running local smoke scripts
- Docker when validating container or compose behavior
- An MCP client or MCP inspector for manual transport checks in later stages

## Quick Start

Run over stdio:

```bash
just dev
```

Run over streamable HTTP:

```bash
just dev-http
```

The streamable HTTP endpoint is:

```text
http://127.0.0.1:8000/mcp
```

Direct binary usage:

```bash
cargo run -- stdio
cargo run -- streamhttp --host 127.0.0.1 --port 8000 --path /mcp
```

When no command is provided, the binary defaults to `stdio`.

## Runtime Control Plane

Stage 1 supports these environment variables:

| Variable | Default | Behavior |
| --- | --- | --- |
| `READ_ONLY_MODE` | `false` | Truthy values are `true`, `1`, `yes`, `y`, and `on`. Future write tools are hidden from discovery and blocked on direct call when enabled. |
| `ENABLED_TOOLS` | unset | Comma-separated tool names. Empty or unset means no name filtering. |
| `TOOLSETS` | all toolsets | Supports `all`, `default`, or comma-separated toolset names. Unknown-only values fail closed. `migration_status` is not part of Jira or Confluence toolsets. |
| `JIRA_URL` | unset | Stage 1 only uses non-empty presence for Jira service availability filtering. |
| `CONFLUENCE_URL` | unset | Stage 1 only uses non-empty presence for Confluence service availability filtering. |
| `MCP_HTTP_HOST` | `127.0.0.1` | Streamable HTTP host when not overridden by CLI. |
| `MCP_HTTP_PORT` | `8000` | Streamable HTTP port when not overridden by CLI. |
| `MCP_HTTP_PATH` | `/mcp` | Streamable HTTP MCP path when not overridden by CLI. A missing leading slash is normalized. |

The health endpoint is always:

```text
GET http://127.0.0.1:8000/healthz
```

## MCP Tools

The Rust server currently exposes one temporary tool:

- `migration_status`: reports that the Rust project is a migration baseline and that Jira/Confluence tools have not been migrated.

This tool is a migration aid. It is not a Jira or Confluence business tool and is not counted as Atlassian tool parity.

## Commands

Run `just --list` to see the local command surface.

```bash
just dev           # run stdio transport
just dev-http      # run streamable HTTP transport on 127.0.0.1:8000
just smoke-stdio   # validate stdio MCP initialize, tools/list, and migration_status call
just smoke-http    # validate /healthz and HTTP MCP tools/list
just smoke         # run both smoke checks
just build         # build the release binary
just test          # run tests
just check         # fmt, check, and tests
just docker-build  # local Docker image build
```

## Docker And Compose

Build the local image:

```bash
just docker-build
```

Equivalent direct Docker command:

```bash
docker build -t mcp-atlassian-rs:local -f Dockerfile .
```

Run the image:

```bash
docker run --rm -p 8000:8000 mcp-atlassian-rs:local
```

The image runs:

```bash
mcp-atlassian-rs streamhttp --host 0.0.0.0 --port 8000
```

Run with compose:

```bash
docker compose up --build
```

Set `MCP_PORT` to change the host port used by compose.

## Verification

Stage 1 baseline checks:

```bash
cargo fmt --check
cargo check
cargo test
just smoke
just check
```

Real Jira and Confluence validation is intentionally deferred until business tools and clients are migrated.

## License

Licensed under the MIT License. See [LICENSE](LICENSE).
