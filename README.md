# cyder-mcp-template

Rust-based development template for building Model Context Protocol (MCP) servers with `rmcp`.

The template provides a minimal Rust MCP server with one binary and two transports:

- `stdio` for local MCP clients that launch the server as a subprocess.
- `streamhttp` for streamable HTTP clients that connect to `/mcp`.

The starter implementation includes a small in-memory counter and echo tool. Replace those tools with your own application-specific capabilities.

## Use This Template

Create a new repository from this template with GitHub's **Use this template** button, then clone your new repository and rename the project before the first release. Search for these template names after each rename pass:

- `cyder-mcp-template`
- `cyder_mcp_template`

Rename checklist:

- `README.md`: update the title, project description, and MCP client examples.
- `Cargo.toml`: update `[package].name`, `default-run`, and `[[bin]].name`.
- `Cargo.lock`: regenerate it after changing the Rust package name.
- `src/mcp.rs`: update `SERVER_NAME` and the starter tool descriptions.
- `Dockerfile`: update the copied release binary path and `CMD` binary name.
- `docker-compose.yml`: update the `cyder-mcp-template:local` image name.
- `justfile`: update the default Docker image name.
- `.github/workflows/ci.yml`: update the Docker image tag `cyder-mcp-template:ci`.
- `.github/PULL_REQUEST_TEMPLATE.md` and `CONTRIBUTING.md`: update verification command examples.
- `.github/dependabot.yml`: update `target-branch` if the new repository does not use `main`.
- `LICENSE`: update the copyright holder if needed.

Shortest local verification path after renaming:

```bash
just check
docker compose -f docker-compose.yml config
docker build -t your-project:ci -f Dockerfile .
```

## Requirements

- Rust 1.94 or newer
- just
- Docker when you want container builds or local compose
- An MCP client or the MCP inspector for manual smoke testing

## Quick Start

Run over stdio:

```bash
just dev
```

Run over streamable HTTP:

```bash
just dev-http
```

The HTTP endpoint is:

```text
http://127.0.0.1:8000/mcp
```

## Commands

Run `just --list` to see the command surface.

```bash
just dev           # run stdio transport
just dev-http      # run streamable HTTP transport on 127.0.0.1:8000
just build         # build the release binary
just test          # run tests
just check         # fmt, check, and tests
just docker-build  # local Docker image build
```

Direct binary usage:

```bash
cargo run -- stdio
cargo run -- streamhttp --host 127.0.0.1 --port 8000
```

When no command is provided, the binary defaults to `stdio`.

## MCP Tools

The starter server exposes these example tools:

- `increment`: increment the in-memory counter by 1.
- `decrement`: decrement the in-memory counter by 1.
- `get_value`: read the current counter value.
- `echo`: return the caller-provided message.

## Inspector Smoke Tests

Stdio:

```bash
npx @modelcontextprotocol/inspector cargo run -- stdio
```

Streamable HTTP:

```bash
cargo run -- streamhttp --host 127.0.0.1 --port 8000
npx @modelcontextprotocol/inspector
```

Then connect the inspector to `http://127.0.0.1:8000/mcp`.

## Docker And Compose

Build the local image:

```bash
just docker-build
```

The equivalent direct Docker command is:

```bash
docker build -t cyder-mcp-template:local -f Dockerfile .
```

Run the image:

```bash
docker run --rm -p 8000:8000 cyder-mcp-template:local
```

The image runs:

```bash
cyder-mcp-template streamhttp --host 0.0.0.0 --port 8000
```

Run the app with compose:

```bash
docker compose up --build
```

Set `MCP_PORT` to change the host port used by compose.

## Automation

This template includes `.github/workflows/ci.yml`. The workflow runs on pull requests, pushes to `main` or `master`, and manual dispatch:

- `Service`: installs Rust 1.94, then runs Rust formatting, `cargo check`, and tests.
- `Docker`: waits for the service job, validates `docker-compose.yml`, and builds `cyder-mcp-template:ci`.

When renaming the template, update the workflow's Docker image tag together with the local Docker and compose names. If you use a different CI system, copy the same command set from the workflow.

## License

Licensed under the MIT License. See `LICENSE`.
