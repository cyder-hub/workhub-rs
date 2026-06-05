# mcp-atlassian-rs

Rust migration workspace for MCP Atlassian.

This repository is migrating the Python `mcp-atlassian` Jira and Confluence MCP server to a Rust-native implementation. The Rust binary currently has the shared MCP runtime/control plane, 49 Jira business tools, and 24 Confluence business tools implemented with local mock REST and MCP smoke coverage. Stage 5 integrated acceptance has also validated representative real Jira, real Confluence, dual-service MCP, release, Docker, and compose paths. Stage 6 added the mandatory production safety foundation for redaction, request-scoped streamable HTTP auth, SSRF/allowed-domain checks, and redirect protection.

## Current Status

Implemented in the Rust root project:

- Package, binary, server name, Docker image, compose service, and CI image identity use `mcp-atlassian-rs`.
- MCP server runs over `stdio` and streamable HTTP at `/mcp`.
- Logging is configured to stderr so stdio MCP stdout remains protocol-only.
- Runtime control-plane config parses `READ_ONLY_MODE`, `ENABLED_TOOLS`, `TOOLSETS`, `ATLASSIAN_OAUTH_CLOUD_ID`, `MCP_ALLOWED_URL_DOMAINS`, `IGNORE_HEADER_AUTH`, `MCP_HTTP_HOST`, `MCP_HTTP_PORT`, and `MCP_HTTP_PATH`.
- Jira config parses `JIRA_URL`, `JIRA_USERNAME`, `JIRA_API_TOKEN`, `JIRA_PERSONAL_TOKEN`, `JIRA_SSL_VERIFY`, `JIRA_PROJECTS_FILTER`, and `JIRA_TIMEOUT`.
- Confluence config parses `CONFLUENCE_URL`, `CONFLUENCE_USERNAME`, `CONFLUENCE_API_TOKEN`, `CONFLUENCE_PERSONAL_TOKEN`, `CONFLUENCE_SSL_VERIFY`, `CONFLUENCE_SPACES_FILTER`, and `CONFLUENCE_TIMEOUT`.
- Jira Cloud uses username/API token auth for `*.atlassian.net`; Jira Server/Data Center uses PAT auth.
- Confluence Cloud uses username/API token auth for `*.atlassian.net`; Confluence Server/Data Center uses PAT auth.
- Shared Atlassian HTTP/auth/error helpers and Jira models/client/tool handlers are implemented for the Stage 2 core tools and the completed Stage 3 Jira extensions.
- Tool registry metadata, service availability filtering, toolset filtering, enabled-tools filtering, and read-only write guards are in place for migrated tools.
- Stage 3 Jira extension tools are implemented for local mock validation: create/update/delete issue, batch create, changelog bulk fetch, projects, versions, users, watchers, worklog, links, attachment download, issue image retrieval, agile boards/sprints, service desk queues, Forms/ProForma, metrics/SLA, and development information.
- Stage 4 Confluence implementation has local mock coverage for config/auth/client/models and all 24 Confluence tools, including pages/comments/labels/users/history/diff/analytics/attachments.
- Streamable HTTP exposes `GET /healthz`.
- Stage 6 security is implemented for unified token/header/error redaction, request-scoped streamable HTTP auth, header-provided service URL SSRF checks, allowed domains, same-origin redirect policy, and MCP session auth fingerprint stability.
- Local stdio, streamable HTTP, and read-only smoke commands validate MCP initialization, Jira and Confluence tool discovery, mock read calls, `/healthz`, and write-tool blocking.
- Stage 5 local gate passed `cargo fmt --check`, `cargo check`, `cargo test`, local stdio/HTTP/Jira/Confluence smokes, release build, Docker build, compose config, and compose `/healthz` smoke.
- Stage 5 real acceptance passed Jira core read paths, Jira Agile board lookup, SLA read, development-info single/batch paths, Confluence page/comment/label/analytics/attachment representative paths, and dual-service MCP stdio/HTTP representative calls.
- The temporary MCP tool `migration_status` reports the migration state.

Deferred:

- `confluence_get_page_views` is Cloud-only. Confluence Server/Data Center returns a structured unavailable response; Stage 5 validated the Cloud representative path.
- Jira Service Management and Forms/ProForma remain objectively blocked in the Stage 5 test tenant: JSM service desk lookup returned 403, and the current Forms client path did not receive an effective Forms API response. These toolsets are implemented with local mock/product-dependency coverage but are not documented as real-accepted.
- `confluence_search_user` is implemented with local mock coverage. Stage 5 did not include a dedicated real user-search row, so it remains local-validated only.
- OAuth Cloud 3LO, BYOT, Data Center OAuth, SSE transport compatibility, proxy/custom outbound headers, mTLS, and broader compatibility auth/transport/network decisions remain Stage 7 work.
- Release pipeline, Helm, production deployment docs, parity audit, and long-term support matrix finalization remain Stage 8 work. Stage 5 only validated the local release binary, local Docker build, compose config, and a compose `/healthz` smoke.

## Stage 5 Gate Result

| Area | Stage 5 result |
| --- | --- |
| Local Rust regression | Passed: format, check, tests, local stdio/HTTP smoke, Jira read-only smoke, Confluence smoke, aggregate smoke. |
| Release/container | Passed: release build, Docker image build, compose config, compose startup, and `/healthz` smoke. |
| Real Jira core | Passed: issue read, JQL search, project issue search, field search/options, watchers read, and read-only write guards. |
| Real Jira product paths | Passed for Agile board lookup, SLA read, and development single/batch reads. JSM is blocked by 403 in the test tenant; Forms/ProForma is blocked by product/interface availability. |
| Real Confluence | Passed for search, page read, children/tree, comments, test-object create/update, add/reply comment, labels, Cloud page views, attachments list/download/content/images/upload/batch upload, and read-only delete/move/delete-attachment guards. `confluence_search_user` was not separately real-executed. |
| Dual-service MCP | Passed for stdio and streamable HTTP discovery and representative Jira/Confluence read calls, including `TOOLSETS=default` and `READ_ONLY_MODE=true` samples. |

## Requirements

- Rust 1.94 or newer
- just
- Python 3 when running local smoke scripts
- curl for manual HTTP checks
- Docker when validating container or compose behavior in later gates
- An MCP client or MCP inspector for manual transport checks

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

## Jira Configuration

Jira tools are discoverable only when Jira service configuration and authentication are complete.

Jira Cloud:

```bash
export JIRA_URL="https://your-company.atlassian.net"
export JIRA_USERNAME="user@example.com"
# Set JIRA_API_TOKEN in your local shell, secret manager, or uncommitted dotenv file.
export JIRA_API_TOKEN
cargo run -- stdio
```

Jira Server/Data Center:

```bash
export JIRA_URL="https://jira.example.com"
# Set JIRA_PERSONAL_TOKEN in your local shell, secret manager, or uncommitted dotenv file.
export JIRA_PERSONAL_TOKEN
cargo run -- stdio
```

Optional Jira variables:

| Variable | Default | Behavior |
| --- | --- | --- |
| `JIRA_SSL_VERIFY` | `true` | Set `false`, `0`, `no`, or `off` to disable TLS certificate verification for Jira requests. |
| `JIRA_PROJECTS_FILTER` | unset | Comma-separated project keys. Filters `jira_get_issue` by issue key prefix and injects a project filter into JQL search. |
| `JIRA_TIMEOUT` | `75` | Jira HTTP request timeout in seconds. Must be a positive integer. |
| `ATLASSIAN_OAUTH_CLOUD_ID` | unset | Optional Cloud ID used by Jira Forms/ProForma helpers. Missing values return a structured product-dependency response. |

OAuth, BYOT, and Data Center OAuth are not implemented. Stage 6 `Authorization: Bearer` is treated only as a PAT-compatible request credential, not as OAuth support.

## Confluence Configuration

Confluence tools are discoverable only when Confluence service configuration and authentication are complete.

Confluence Cloud:

```bash
export CONFLUENCE_URL="https://your-company.atlassian.net/wiki"
export CONFLUENCE_USERNAME="user@example.com"
# Set CONFLUENCE_API_TOKEN in your local shell, secret manager, or uncommitted dotenv file.
export CONFLUENCE_API_TOKEN
cargo run -- stdio
```

Confluence Server/Data Center:

```bash
export CONFLUENCE_URL="https://confluence.example.com"
# Set CONFLUENCE_PERSONAL_TOKEN in your local shell, secret manager, or uncommitted dotenv file.
export CONFLUENCE_PERSONAL_TOKEN
cargo run -- stdio
```

Optional Confluence variables:

| Variable | Default | Behavior |
| --- | --- | --- |
| `CONFLUENCE_SSL_VERIFY` | `true` | Set `false`, `0`, `no`, or `off` to disable TLS certificate verification for Confluence requests. |
| `CONFLUENCE_SPACES_FILTER` | unset | Comma-separated space keys. Applies to Confluence search when the tool call does not provide `spaces_filter`; an explicit empty `spaces_filter` disables the env filter. |
| `CONFLUENCE_TIMEOUT` | `75` | Confluence HTTP request timeout in seconds. Must be a positive integer. |

OAuth, BYOT, proxy/custom outbound headers, and mTLS are not implemented. Stage 6 request-scoped headers and SSRF protections are available only on the streamable HTTP MCP endpoint.

## Confluence Content Conversion Boundary

Stage 4 uses a deterministic minimal Markdown to Confluence storage conversion for local mock validation. It covers headings, paragraphs, unordered lists, simple inline links, fenced code blocks, line breaks, and HTML escaping.

The Rust implementation does not claim Python `md2conf` parity in Stage 4. Mermaid rendering, macro rendering, and full heading anchor parity remain outside the current local Confluence loop.

## Runtime Control Plane

| Variable | Default | Behavior |
| --- | --- | --- |
| `READ_ONLY_MODE` | `false` | Truthy values are `true`, `1`, `yes`, `y`, and `on`. Write tools are hidden from discovery and blocked on direct call when enabled. |
| `ENABLED_TOOLS` | unset | Comma-separated tool names. Empty or unset means no name filtering. |
| `TOOLSETS` | all toolsets | Supports `all`, `default`, or comma-separated toolset names. Unknown-only values fail closed. `migration_status` is not part of Jira or Confluence toolsets. |
| `MCP_ALLOWED_URL_DOMAINS` | unset | Optional comma-separated domain allowlist for header-provided Jira/Confluence service URLs. Exact domain and subdomain matches are accepted; URL values, IP literals, localhost, and metadata hostnames are rejected. |
| `IGNORE_HEADER_AUTH` | `false` | Truthy values are `true`, `1`, `yes`, `y`, and `on`. When enabled, streamable HTTP ignores all request-scoped auth/service headers and uses only global env service config. |
| `MCP_HTTP_HOST` | `127.0.0.1` | Streamable HTTP host when not overridden by CLI. |
| `MCP_HTTP_PORT` | `8000` | Streamable HTTP port when not overridden by CLI. |
| `MCP_HTTP_PATH` | `/mcp` | Streamable HTTP MCP path when not overridden by CLI. A missing leading slash is normalized. |

Supported transports are `stdio` and streamable HTTP. SSE is not implemented; Stage 7 must classify it as implemented, explicitly unsupported, or Stage 8 backlog.

The health endpoint is always:

```text
GET http://127.0.0.1:8000/healthz
```

## Stage 6 Security And Request Auth

Stage 6 security is active for the streamable HTTP MCP endpoint. It does not affect `stdio` global-env behavior except for shared redaction and outbound redirect policy.

Supported request-scoped headers:

| Header | Behavior |
| --- | --- |
| `Authorization: Basic <base64(email:api_token)>` | Overrides credentials for already configured Jira/Confluence services in the current HTTP request or MCP session. |
| `Authorization: Token <pat>` | Uses the token as a PAT-compatible credential for already configured services. |
| `Authorization: Bearer <token>` | Uses the token as a PAT-compatible credential only. This is not OAuth. |
| `X-Atlassian-Jira-Url` + `X-Atlassian-Jira-Personal-Token` | Creates or overrides the Jira config for the current request/session after URL validation. |
| `X-Atlassian-Confluence-Url` + `X-Atlassian-Confluence-Personal-Token` | Creates or overrides the Confluence config for the current request/session after URL validation. |
| `X-Atlassian-Cloud-Id` | Sets request-scoped Cloud ID context. It does not enable OAuth. |

Security behavior:

- Header-provided service URL/token values must be paired. Missing pairs are rejected.
- Header-provided service URLs must use `http` or `https`, have a hostname, and cannot target localhost, metadata hostnames, private, loopback, link-local, multicast, unspecified, documentation, or DNS-resolved non-global IP addresses.
- When `MCP_ALLOWED_URL_DOMAINS` is set, header-provided service URLs must match an allowed domain or subdomain.
- Outbound Atlassian HTTP redirects are limited to same-origin `http`/`https` redirects, maximum 3 hops.
- Request auth fingerprint is bound to `Mcp-Session-Id`; changing request auth within the same MCP session is rejected.
- Logs, MCP debug argument output, acceptance compact errors, HTTP status summaries, and URL query values are redacted for secret-looking header, token, cookie, password, key, signature, and env secret values.

## MCP Tools

The Rust server exposes these Stage 2 Jira core tools when Jira is configured:

| Tool | Access | Toolset |
| --- | --- | --- |
| `jira_get_issue` | read | `jira_issues` |
| `jira_search` | read | `jira_issues` |
| `jira_get_project_issues` | read | `jira_issues` |
| `jira_search_fields` | read | `jira_fields` |
| `jira_get_field_options` | read | `jira_fields` |
| `jira_add_comment` | write | `jira_comments` |
| `jira_edit_comment` | write | `jira_comments` |
| `jira_get_transitions` | read | `jira_transitions` |
| `jira_transition_issue` | write | `jira_transitions` |

The Rust server also exposes these Stage 3 Jira extension tools when Jira is configured. These are locally validated with mock Jira. Stage 5 real acceptance passed representative Jira core, Agile, SLA, and development paths; Jira Service Management and Forms/ProForma remain objectively blocked as described above.

| Tool | Access | Toolset |
| --- | --- | --- |
| `jira_create_issue` | write | `jira_issues` |
| `jira_batch_create_issues` | write | `jira_issues` |
| `jira_batch_get_changelogs` | read | `jira_issues` |
| `jira_update_issue` | write | `jira_issues` |
| `jira_delete_issue` | write | `jira_issues` |
| `jira_get_all_projects` | read | `jira_projects` |
| `jira_get_project_versions` | read | `jira_projects` |
| `jira_get_project_components` | read | `jira_projects` |
| `jira_create_version` | write | `jira_projects` |
| `jira_batch_create_versions` | write | `jira_projects` |
| `jira_get_user_profile` | read | `jira_users` |
| `jira_get_issue_watchers` | read | `jira_watchers` |
| `jira_add_watcher` | write | `jira_watchers` |
| `jira_remove_watcher` | write | `jira_watchers` |
| `jira_get_worklog` | read | `jira_worklog` |
| `jira_add_worklog` | write | `jira_worklog` |
| `jira_get_link_types` | read | `jira_links` |
| `jira_link_to_epic` | write | `jira_links` |
| `jira_create_issue_link` | write | `jira_links` |
| `jira_create_remote_issue_link` | write | `jira_links` |
| `jira_remove_issue_link` | write | `jira_links` |
| `jira_download_attachments` | read | `jira_attachments` |
| `jira_get_issue_images` | read | `jira_attachments` |
| `jira_get_agile_boards` | read | `jira_agile` |
| `jira_get_board_issues` | read | `jira_agile` |
| `jira_get_sprints_from_board` | read | `jira_agile` |
| `jira_get_sprint_issues` | read | `jira_agile` |
| `jira_create_sprint` | write | `jira_agile` |
| `jira_update_sprint` | write | `jira_agile` |
| `jira_add_issues_to_sprint` | write | `jira_agile` |
| `jira_get_service_desk_for_project` | read | `jira_service_desk` |
| `jira_get_service_desk_queues` | read | `jira_service_desk` |
| `jira_get_queue_issues` | read | `jira_service_desk` |
| `jira_get_issue_proforma_forms` | read | `jira_forms` |
| `jira_get_proforma_form_details` | read | `jira_forms` |
| `jira_update_proforma_form_answers` | write | `jira_forms` |
| `jira_get_issue_dates` | read | `jira_metrics` |
| `jira_get_issue_sla` | read | `jira_metrics` |
| `jira_get_issue_development_info` | read | `jira_development` |
| `jira_get_issues_development_info` | read | `jira_development` |

The Rust server also exposes these Stage 4 Confluence tools when Confluence is configured. These are locally validated with mock Confluence. Stage 5 real acceptance passed representative pages, comments, labels, analytics, and attachments paths on test objects; `confluence_search_user` remains local-validated only.

| Tool | Access | Toolset |
| --- | --- | --- |
| `confluence_search` | read | `confluence_pages` |
| `confluence_get_page` | read | `confluence_pages` |
| `confluence_get_page_children` | read | `confluence_pages` |
| `confluence_get_space_page_tree` | read | `confluence_pages` |
| `confluence_create_page` | write | `confluence_pages` |
| `confluence_update_page` | write | `confluence_pages` |
| `confluence_delete_page` | write | `confluence_pages` |
| `confluence_move_page` | write | `confluence_pages` |
| `confluence_get_comments` | read | `confluence_comments` |
| `confluence_add_comment` | write | `confluence_comments` |
| `confluence_reply_to_comment` | write | `confluence_comments` |
| `confluence_get_labels` | read | `confluence_labels` |
| `confluence_add_label` | write | `confluence_labels` |
| `confluence_search_user` | read | `confluence_users` |
| `confluence_get_page_history` | read | `confluence_pages` |
| `confluence_get_page_diff` | read | `confluence_pages` |
| `confluence_get_page_views` | read | `confluence_analytics` |
| `confluence_upload_attachment` | write | `confluence_attachments` |
| `confluence_upload_attachments` | write | `confluence_attachments` |
| `confluence_get_attachments` | read | `confluence_attachments` |
| `confluence_download_attachment` | read | `confluence_attachments` |
| `confluence_download_content_attachments` | read | `confluence_attachments` |
| `confluence_delete_attachment` | write | `confluence_attachments` |
| `confluence_get_page_images` | read | `confluence_attachments` |

`confluence_get_page_views` is Cloud-only. Attachment download/image tools return bounded structured content; the current inline content limit is 1 MiB per attachment. Attachment upload tools accept explicit local file paths readable by the server process and do not implement directory allowlists or remote URL upload in Stage 4.

The Rust server also exposes one migration utility tool:

- `migration_status`: reports the Rust migration state.

`migration_status` is not a Jira or Confluence business tool and is not counted as Atlassian tool parity.

## Commands

Run `just --list` to see the local command surface.

```bash
just dev           # run stdio transport
just dev-http      # run streamable HTTP transport on 127.0.0.1:8000
just smoke-stdio       # validate stdio MCP initialize, tools/list, and mock Jira jira_get_issue
just smoke-http        # validate /healthz, HTTP MCP tools/list, and mock Jira jira_get_issue
just smoke-jira        # validate read-only Jira write-tool hiding and blocking
just smoke-confluence  # validate mock Confluence stdio, HTTP, and read-only write blocking
just smoke             # run all local smoke checks
just acceptance-jira        # run real Jira acceptance using .env.dev by default
just acceptance-confluence  # run real Confluence acceptance using .env.dev by default
just acceptance-mcp         # run real dual-service MCP acceptance using .env.dev by default
just build             # build the release binary
just test              # run tests
just check             # fmt, check, and tests
just docker-build      # local Docker image build
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

Stage 5 validated `just docker-build`, compose config, compose startup, and `GET /healthz` with `MCP_PORT=18080`. Stage 8 still owns release-grade image tags, publishing, Helm, and long-term deployment documentation.

## Verification

Local checks:

```bash
cargo fmt --check
cargo check
cargo test
just check
just smoke-stdio
just smoke-http
just smoke-jira
just smoke-confluence
just smoke
just acceptance-jira
just acceptance-confluence
just acceptance-mcp
```

The smoke commands start local mock Jira and Confluence servers and do not require real Atlassian credentials.

Real acceptance wrappers read `.env.dev` by default; set `ACCEPTANCE_ENV_FILE` to use another dotenv file. Do not store real token values in committed files or task records. Real acceptance requires only test objects, not production business objects. Common test-object variables include `JIRA_READ_ISSUE`, `JIRA_PROJECT_KEY`, `JIRA_FIELD_ID`, `JIRA_FIELD_CONTEXT_ID`, `JIRA_SERVICE_DESK_ID`, `JIRA_QUEUE_ID`, `JIRA_FORM_ID`, `CONFLUENCE_SEARCH_QUERY`, `CONFLUENCE_PAGE_ID`, `CONFLUENCE_SPACE_KEY`, `CONFLUENCE_TEST_PAGE_PREFIX`, `CONFLUENCE_MUTATION_PAGE_ID`, `CONFLUENCE_COMMENT_ID`, `CONFLUENCE_ATTACHMENT_ID`, `CONFLUENCE_ATTACHMENT_FILE`, and `CONFLUENCE_LABEL_NAME`.

## License

Licensed under the MIT License. See [LICENSE](LICENSE).
