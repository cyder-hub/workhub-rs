# mcp-atlassian-rs

Rust migration workspace for MCP Atlassian.

This repository is migrating the Python `mcp-atlassian` Jira and Confluence MCP server to a Rust-native implementation. The current Rust binary is a Stage 0 baseline: project identity, runtime skeleton, Docker/compose wiring, and migration tracking are in place. Jira and Confluence business tools are not available yet.

## Current Status

Implemented in the Rust root project:

- Package, binary, server name, Docker image, compose service, and CI image identity use `mcp-atlassian-rs`.
- MCP server runs over `stdio` and streamable HTTP at `/mcp`.
- Logging is configured to stderr so stdio MCP stdout remains protocol-only.
- The temporary MCP tool `migration_status` reports the migration state.
- Dockerfile, compose file, just commands, and CI metadata build the Rust binary.

Not implemented yet:

- Atlassian configuration, authentication, HTTP client, or API models.
- Jira and Confluence MCP tools.
- Tool filtering through `ENABLED_TOOLS` or `TOOLSETS`.
- Read-only write protection.
- `/healthz`.
- Real Jira or Confluence smoke tests.

## Requirements

- Rust 1.94 or newer
- just
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
cargo run -- streamhttp --host 127.0.0.1 --port 8000
```

When no command is provided, the binary defaults to `stdio`.

## MCP Tools

The Rust server currently exposes one temporary tool:

- `migration_status`: reports that the Rust project is in Stage 0 and that Jira/Confluence tools have not been migrated.

This tool is a migration aid. Later stages will replace or reclassify tool exposure after the shared runtime and tool registry are implemented.

## Commands

Run `just --list` to see the local command surface.

```bash
just dev           # run stdio transport
just dev-http      # run streamable HTTP transport on 127.0.0.1:8000
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

Stage 0 baseline checks:

```bash
cargo fmt --check
cargo check
cargo test
just check
docker compose -f docker-compose.yml config
docker build -t mcp-atlassian-rs:ci -f Dockerfile .
```

Real Jira and Confluence validation is intentionally deferred. The roadmap requires a real Jira gate before Confluence migration begins.

## License

Licensed under the MIT License. See [LICENSE](LICENSE).
