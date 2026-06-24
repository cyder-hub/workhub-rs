# Production Deployment

This guide describes the supported runtime shapes for `workhub-rs`.

## Deployment Checklist

- For local stdio users, install or update the release binary with the interactive installer.
- Choose one supported transport: `stdio` for local MCP clients or streamable HTTP for server deployments.
- Use `workhub cli ...` for production command-line automation against the same Jira, Confluence, and GitLab operation layer.
- Configure only the Jira, Confluence, and GitLab services you want exposed.
- Restrict exposed MCP tools with `MCP_TOOL_PROFILE`, `MCP_TOOLSETS`, `MCP_ENABLED_TOOLS`, or `MCP_DISABLED_TOOLS` when the MCP client should not see every configured tool. The resource CLI ignores these MCP visibility controls.
- Keep service credentials in a secret manager, shell environment, or orchestrator secret. Do not commit dotenv files with real credentials.
- Set `WORKHUB_LOG_DIR` to a durable writable path in service deployments, or rely on the platform log directory for local binaries.
- Check `GET /healthz` for streamable HTTP deployments.
- Review `SECURITY.md` before exposing the HTTP endpoint beyond localhost.

## Local Binary Installer

Linux and macOS users can install, update, or uninstall the default local binary with:

```bash
curl -fsSL https://github.com/cyder-hub/workhub-rs/releases/latest/download/install.sh | sh
```

Windows PowerShell users can run:

```powershell
irm https://github.com/cyder-hub/workhub-rs/releases/latest/download/install.ps1 | iex
```

The installer is interactive. It detects the platform, checks the default install path, reads the latest GitHub Release version, and then prompts for the relevant action. It manages only the default binary path and does not remove Workhub configuration files.

## Stdio

Use stdio when an MCP client starts the server process directly:

```bash
workhub stdio
```

Stdout is reserved for the MCP protocol. By default, stdio writes structured NDJSON to the platform log directory and does not emit console log summaries. Use `workhub logs path` to print the exact directory and active targets.

## Streamable HTTP

Use streamable HTTP for server deployments:

```bash
workhub streamhttp --host 0.0.0.0 --port 8000 --path /mcp
```

The health endpoint is:

```text
GET /healthz
```

The MCP endpoint path defaults to `/mcp` and can be set with `MCP_HTTP_PATH` or `--path`.

For service deployments, set `WORKHUB_LOG_DIR` to a persistent volume and keep the default `file,error_file,audit_file` targets enabled.

## Resource CLI

Use the CLI for one-shot automation and shell workflows:

```bash
workhub cli jira issue get ABC-1 --fields summary,status
workhub cli confluence page get --id 123456
workhub cli gitlab mr list group/project --state opened
```

The CLI uses the same service credentials, project/space filters, proxy, TLS, mTLS, redirect policy, and redaction behavior as MCP tool calls. It ignores MCP tool visibility controls such as `MCP_TOOL_PROFILE`, `MCP_TOOLSETS`, `MCP_ENABLED_TOOLS`, and `MCP_DISABLED_TOOLS`. It does not offer provider URL, token, password, proxy, custom-header, TLS, or mTLS command-line override flags.

Successful default output is compact text on stdout. `--json` emits result JSON on stdout. Errors are written to stderr and logs are written to the configured log files; stdout remains parseable for successful command results. CLI mode does not enable console logs by default. Use `workhub cli -v ...`, `workhub cli --verbose ...`, or explicitly include `console` in `WORKHUB_LOG_TARGETS` when you want compact log summaries on stderr.

See [cli.md](cli.md) for the full command reference.

## Docker And Compose

Build the local image:

```bash
docker build -t workhub-rs:local -f Dockerfile .
```

Run the image:

```bash
docker run --rm -p 8000:8000 workhub-rs:local
```

Run the image with a persistent log directory:

```bash
docker run --rm -e WORKHUB_LOG_DIR=/var/log/workhub -v workhub-logs:/var/log/workhub -p 8000:8000 workhub-rs:local
```

Run with compose:

```bash
docker compose up --build
```

The image runs as a non-root `app` user. The compose service includes a `/healthz` healthcheck and maps `${MCP_PORT:-8000}` on the host to container port `8000`.

Compose passes through runtime control variables plus Jira, Confluence, GitLab, shared `ATLASSIAN_*`, logging, and proxy variables. For example:

```bash
WORKHUB_LOG_PROFILE=support docker compose up --build
```

## Runtime Controls

| Variable | Deployment use |
| --- | --- |
| `MCP_TOOL_PROFILE` | Set `basic`, `developer`, `manager`, `full`, or `custom` for MCP discovery and MCP tool calls. Defaults to `basic`. With Jira, Confluence, and GitLab configured, profiles expose 24, 54, 98, 101, or 0 tools respectively. Unknown values fail startup. Ignored by `workhub cli ...`. |
| `MCP_TOOLSETS` | Add comma-separated toolset names to the selected profile. `all` enables every toolset. Unknown names fail startup. |
| `MCP_ENABLED_TOOLS` | Add comma-separated exact tool names. |
| `MCP_DISABLED_TOOLS` | Remove comma-separated exact tool names. Takes precedence over profile/toolset inclusion. |
| `MCP_HTTP_HOST` / `MCP_HTTP_PORT` / `MCP_HTTP_PATH` | Configure streamable HTTP when CLI flags are not used. Ignored by stdio startup. |
| `MCP_PORT` | Compose-only host port mapping. Does not configure the Rust process itself. |
| `ENV_FILE` | Optional dotenv file loaded by `streamhttp` and `cli` startup. The explicit `--env-file <path>` argument takes precedence. For `cli`, the global CLI `.env` and strict `./.env` are lower-priority fallbacks. Ignored by `stdio`. |
| `WORKHUB_LOG_PROFILE` | Select `production`, `support`, `development`, `quiet`, or `test`. Defaults to `production`. |
| `WORKHUB_LOG_DIR` | Override the platform log directory. Set this to a persistent writable volume in containers. |
| `WORKHUB_LOG_TARGETS` | Select `console`, `file`, `error_file`, and/or `audit_file`. Defaults are mode-specific: streamable HTTP uses all four targets; stdio, version, `logs`, and CLI commands omit `console` unless CLI `-v`/`--verbose` is used. |
| `WORKHUB_LOG_LEVEL` | Set the global minimum level. Defaults to `info`. |
| `WORKHUB_LOG_FILTER` | Advanced module filter for targeted troubleshooting. |
| `WORKHUB_LOG_PAYLOADS` | Select `none`, `metadata`, or `sanitized_args`. Defaults to `metadata`. |
| `WORKHUB_LOG_ROTATION`, `WORKHUB_LOG_MAX_BYTES`, `WORKHUB_LOG_RETENTION_FILES`, `WORKHUB_LOG_RETENTION_DAYS`, `WORKHUB_LOG_COMPRESSION` | Configure rotation, retention, and gzip compression. |
| `WORKHUB_LOG_BUNDLE_MAX_BYTES` | Configure the maximum included log bytes for `workhub logs bundle`. |

## Jira, Confluence, And GitLab Auth

Supported global auth:

- Jira Cloud: `JIRA_URL`, `JIRA_USERNAME`, `JIRA_API_TOKEN`.
- Jira Server/Data Center: `JIRA_URL`, `JIRA_PERSONAL_TOKEN`, or `JIRA_URL`, `JIRA_USERNAME`, `JIRA_PASSWORD`.
- Confluence Cloud: `CONFLUENCE_URL`, `CONFLUENCE_USERNAME`, `CONFLUENCE_API_TOKEN`.
- Confluence Server/Data Center: `CONFLUENCE_URL`, `CONFLUENCE_PERSONAL_TOKEN`, or `CONFLUENCE_URL`, `CONFLUENCE_USERNAME`, `CONFLUENCE_PASSWORD`.
- GitLab: `GITLAB_URL` plus `GITLAB_TOKEN` or `GITLAB_PERSONAL_TOKEN`.
- Shared auth fallbacks: `ATLASSIAN_USERNAME`, `ATLASSIAN_API_TOKEN`, `ATLASSIAN_PASSWORD`, `ATLASSIAN_PERSONAL_TOKEN`.

GitLab token precedence is `GITLAB_TOKEN`, then `GITLAB_PERSONAL_TOKEN`. Both are sent as `PRIVATE-TOKEN`. Use `read_api` for read-only tools and `api` for write, cleanup, approval, merge, and branch tools. GitLab username/password API auth is not supported.

`GITLAB_URL` should be the instance root, such as `https://gitlab.example.com`. A value ending in `/api/v4` is normalized back to the instance root. Set `GITLAB_PROJECTS_FILTER` to an exact comma-separated allowlist of numeric project IDs or full paths, such as `123,group/project`, to prevent project-scoped GitLab tools from reaching other projects.

Streamable HTTP uses the same process-wide service configuration as stdio. Incoming HTTP headers do not change Jira, Confluence, or GitLab upstream identity. Protect public HTTP deployments with a fronting gateway or network boundary appropriate for your environment.

## Network And TLS

Supported outbound network controls:

- Jira proxy: `JIRA_HTTP_PROXY`, `JIRA_HTTPS_PROXY`, `JIRA_NO_PROXY`.
- Confluence proxy: `CONFLUENCE_HTTP_PROXY`, `CONFLUENCE_HTTPS_PROXY`, `CONFLUENCE_NO_PROXY`.
- GitLab proxy: `GITLAB_HTTP_PROXY`, `GITLAB_HTTPS_PROXY`, `GITLAB_NO_PROXY`.
- Shared Atlassian proxy fallback: `ATLASSIAN_HTTP_PROXY`, `ATLASSIAN_HTTPS_PROXY`, `ATLASSIAN_NO_PROXY`.
- Global proxy fallback: `HTTP_PROXY`, `HTTPS_PROXY`, `NO_PROXY`.
- Jira custom outbound headers: `JIRA_CUSTOM_HEADERS`.
- Confluence custom outbound headers: `CONFLUENCE_CUSTOM_HEADERS`.
- GitLab custom outbound headers: `GITLAB_CUSTOM_HEADERS`.
- Shared custom outbound headers fallback: `ATLASSIAN_CUSTOM_HEADERS`.
- Jira mTLS: `JIRA_CLIENT_CERT`, `JIRA_CLIENT_KEY`.
- Confluence mTLS: `CONFLUENCE_CLIENT_CERT`, `CONFLUENCE_CLIENT_KEY`.
- GitLab mTLS: `GITLAB_CLIENT_CERT`, `GITLAB_CLIENT_KEY`.
- Shared mTLS fallback: `ATLASSIAN_CLIENT_CERT`, `ATLASSIAN_CLIENT_KEY`.
- TLS verification toggles: `JIRA_SSL_VERIFY`, `CONFLUENCE_SSL_VERIFY`, `GITLAB_SSL_VERIFY`, `ATLASSIAN_SSL_VERIFY`.
- Timeout controls: `JIRA_TIMEOUT`, `CONFLUENCE_TIMEOUT`, `GITLAB_TIMEOUT`, `ATLASSIAN_TIMEOUT`.

Reserved auth, cookie, host, content, proxy, connection, and GitLab token headers are rejected in custom outbound headers. GitLab does not use the shared `ATLASSIAN_*` fallback variables for proxy, custom headers, mTLS, TLS verification, or timeout.

## Log Operations

Use these commands on the same host/container that runs Workhub:

```bash
workhub logs path
workhub logs usage --since 24h
workhub logs bundle --since 24h --output workhub-logs.zip
```

`workhub logs path` prints JSON with the log directory, enabled targets, and recent files. `workhub logs usage` reports MCP tool and CLI command call counts, success/failure counts, incomplete calls, and duration summaries for a selected time window. `workhub logs bundle` creates a redacted ZIP with recent runtime, error, and audit logs plus `runtime-summary.json` and `manifest.json`.

## Security Behavior

- Secret-looking values are redacted from logs, development acceptance output, URL query values, and upstream error summaries.
- Support and development payload settings can include redacted argument metadata. They can still include business identifiers such as JQL, issue keys, page IDs, summaries, or descriptions, so use those profiles only while troubleshooting.
- Outbound upstream redirects are same-origin only and limited to 3 hops.

## Unsupported In The Current Rust Release

- OAuth Cloud 3LO authorization-code flow.
- OAuth proxy/DCR.
- OAuth refresh/token storage.
- Data Center OAuth authorization-code/refresh.
- SSE transport.
- SOCKS proxy.
- Helm chart.
- External registry publishing.
