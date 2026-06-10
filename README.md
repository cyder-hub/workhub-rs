# mcp-workhub-rs

Rust-native MCP server for work systems. It currently exposes Jira, Confluence, and GitLab merge-request tools through stdio or streamable HTTP.

`mcp-workhub-rs` provides 88 business tools: 49 Jira tools, 24 Confluence tools, and 15 GitLab merge-request tools. Jira and Confluence have representative real acceptance coverage; GitLab support is currently local/mock validated only. See [docs/support-matrix.md](docs/support-matrix.md) for exact per-tool status.

## Quick Start

Build the binary from the repository:

```bash
cargo build --release
```

The binary will be available at:

```text
target/release/mcp-workhub-rs
```

Configure only the services you want to expose. `TOOL_PROFILE` defaults to `basic`, so the smallest setup only needs service credentials.

Jira Cloud:

```bash
export JIRA_URL="https://your-company.atlassian.net"
export JIRA_USERNAME="user@example.com"
export JIRA_API_TOKEN="<api-token>"
```

Jira Server/Data Center:

```bash
export JIRA_URL="https://jira.example.com"
export JIRA_PERSONAL_TOKEN="<personal-access-token>"
```

Confluence Cloud:

```bash
export CONFLUENCE_URL="https://your-company.atlassian.net/wiki"
export CONFLUENCE_USERNAME="user@example.com"
export CONFLUENCE_API_TOKEN="<api-token>"
```

Confluence Server/Data Center:

```bash
export CONFLUENCE_URL="https://confluence.example.com"
export CONFLUENCE_PERSONAL_TOKEN="<personal-access-token>"
```

GitLab:

```bash
export GITLAB_URL="https://gitlab.example.com"
export GITLAB_TOKEN="<personal-project-or-group-access-token>"
```

Run stdio locally:

```bash
cargo run -- stdio
```

Run streamable HTTP locally:

```bash
cargo run -- streamhttp --host 127.0.0.1 --port 8000 --path /mcp
```

The streamable HTTP MCP endpoint is `http://127.0.0.1:8000/mcp`; the health endpoint is `http://127.0.0.1:8000/healthz`.

## MCP stdio JSON

Most desktop MCP clients accept a `mcp.json` shape like this. Use an absolute path for `command`, and remove any service block you do not need.

```json
{
  "mcpServers": {
    "mcp-workhub": {
      "command": "/absolute/path/to/mcp-workhub-rs",
      "args": ["stdio"],
      "env": {
        "TOOL_PROFILE": "basic",
        "JIRA_URL": "https://your-company.atlassian.net",
        "JIRA_USERNAME": "user@example.com",
        "JIRA_API_TOKEN": "<jira-api-token>",
        "CONFLUENCE_URL": "https://your-company.atlassian.net/wiki",
        "CONFLUENCE_USERNAME": "user@example.com",
        "CONFLUENCE_API_TOKEN": "<confluence-api-token>",
        "GITLAB_URL": "https://gitlab.example.com",
        "GITLAB_TOKEN": "<gitlab-token>",
        "GITLAB_PROJECTS_FILTER": "group/project"
      }
    }
  }
}
```

For GitLab read-only use, choose a token with `read_api`. For GitLab create/update/note/discussion/approval/merge tools, use a token with `api`. `GITLAB_PROJECTS_FILTER` is optional but recommended for production because it restricts project-scoped GitLab tools before any upstream HTTP request is sent.

## Tool Access

Tool visibility is controlled by profiles, toolsets, exact enablement, exact disablement, and service availability. Unknown profiles or toolsets fail startup.

| Profile | Intended use |
| --- | --- |
| `basic` | Common Jira, Confluence, and GitLab reads plus limited safe writes such as Jira issue creation/comments. |
| `developer` | Adds workflow, Agile, attachment, development-info, Confluence version/attachment reads, and GitLab MR write/approval/merge tools. |
| `manager` | Adds most Jira project, sprint, worklog, link, JSM, SLA, Forms, Confluence write, analytics, and attachment upload tools. |
| `full` | All registered Jira, Confluence, and GitLab tools, including destructive Confluence delete toolsets. |
| `custom` | No profile baseline; use `TOOLSETS` and/or exact tool variables. |

Advanced controls:

- `TOOLSETS`: add comma-separated registered toolsets to the selected profile. `all` enables every toolset.
- `ENABLED_TOOLS`: add comma-separated exact MCP tool names.
- `DISABLED_TOOLS`: remove comma-separated exact MCP tool names. This takes precedence over every inclusion mechanism.

See [docs/configuration.md](docs/configuration.md) for the full configuration reference.

## Origin

This project was inspired by [sooperset/mcp-atlassian](https://github.com/sooperset/mcp-atlassian), a Python MCP server for Atlassian Jira and Confluence. The first Rust version was a migration/reference implementation of the Jira and Confluence surface from that project.

Since then, this project has diverged in a few practical ways: it uses a Rust-native RMCP runtime, typed provider clients, centralized tool metadata and profile filtering, stricter redaction/SSRF/redirect handling, request-scoped streamable HTTP auth, bounded attachment and diff responses, adjusted tool behavior based on real validation, and a GitLab merge-request extension surface.

## Documentation

- [Configuration](docs/configuration.md): service credentials, tool access, request auth, network/TLS, diagnostics, and content conversion notes.
- [Deployment](docs/deployment.md): stdio, streamable HTTP, Docker, compose, auth, security, and unsupported deployment capabilities.
- [Support matrix](docs/support-matrix.md): every Jira, Confluence, and GitLab tool with local/real acceptance status.
- [Development tools](docs/development-tools.md): `xtask`, local smoke checks, and real acceptance variables.
- [Backlog](docs/backlog.md): fixed future work such as OAuth flows, SSE, SOCKS proxy, Helm, and registry publishing.
- [Security policy](SECURITY.md): supported security boundary and vulnerability reporting.

## Architecture

The codebase is a Rust 1.94 / edition 2024 workspace:

- `src/main.rs`: CLI parsing, tracing, stdio, streamable HTTP, and `/healthz`.
- `src/mcp.rs` and `src/mcp/`: RMCP server glue, handlers, request-scoped auth, schema sanitization, and tool-call diagnostics.
- `src/jira/`, `src/confluence/`, `src/gitlab/`: provider-specific config, clients, tools, models, formatting, and tests.
- `src/upstream/`: provider-agnostic HTTP, auth, proxy, mTLS, custom headers, redaction, SSRF, redirect, and error helpers.
- `src/atlassian/`: Atlassian-specific compatibility and shared Jira/Confluence behavior.
- `src/tool_registry.rs` and `src/tool_registry/`: tool metadata, service availability, profile filtering, toolset filtering, enabled-tools inclusion, and disabled-tools exclusion.
- `xtask/`: development-only smoke and real acceptance automation.

## Development

Requirements:

- Rust 1.94 or newer
- just
- curl for manual HTTP checks
- Docker when validating container or compose behavior
- An MCP client or MCP inspector for manual transport checks

Common commands:

```bash
just dev                 # run stdio transport
just dev-http            # run streamable HTTP on 127.0.0.1:8000
just check               # fmt, check, and tests
just smoke               # all local mock smoke checks
just smoke-gitlab        # GitLab local mock smoke
just build               # release build
just docker-build        # local Docker image build
```

Real Jira/Confluence acceptance commands require disposable test objects and development-only credentials. See [docs/development-tools.md](docs/development-tools.md).

## Docker

Build the local image:

```bash
docker build -t mcp-workhub-rs:local -f Dockerfile .
```

Run the image:

```bash
docker run --rm -p 8000:8000 mcp-workhub-rs:local
```

Run with compose:

```bash
docker compose up --build
```

The image runs as a non-root `app` user and starts streamable HTTP on container port `8000`.

## Releases

Releases are tag-driven. A release tag must use the form `vX.Y.Z`, match `Cargo.toml` `package.version = "X.Y.Z"`, and have a matching `## X.Y.Z` entry in `CHANGELOG.md`.

The release workflow builds Linux, macOS, and Windows binaries named `mcp-workhub-rs-*` with matching `.sha256` checksum files. The current release process does not publish to crates.io, GHCR, Docker Hub, or any other external registry.

## License

Licensed under the MIT License. See [LICENSE](LICENSE).
