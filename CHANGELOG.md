# Changelog

## 0.1.1 - 2026-06-05

### Added
- feat: Support loading environment variables from an `.env` file via the `--env-file` argument or `ENV_FILE` environment variable when running in `streamhttp` mode.

### Fixed
- fix: Resolved Jira Cloud "unbounded JQL" rejection errors by automatically injecting `issuekey IS NOT EMPTY` when queries are unbounded or contain only an `ORDER BY` clause.
- fix: Corrected `ORDER BY` clause positioning when a Jira project filter is applied.
- fix: Prevented MCP schema validation errors (e.g. in Opencode) by automatically wrapping raw JSON array responses into an `items` record (`{ "items": [...] }`) for all tools.

### Chores
- chore(deps): Bumped GitHub Actions `upload-artifact` (v4 to v7) and `download-artifact` (v4 to v8).

## 0.1.0 - 2026-06-05

- Implemented the Rust MCP Atlassian server with 49 Jira business tools and 24 Confluence business tools.
- Added stdio and streamable HTTP transports, `/healthz`, tool filtering, toolset filtering, service availability filtering, and read-only guards.
- Added Jira and Confluence Cloud Basic/API-token auth, Server/Data Center PAT auth, BYOT access-token support, request-scoped streamable HTTP auth, SSRF and allowed-domain checks, same-origin redirect protection, proxy/custom outbound headers, and mTLS client cert/key support.
- Added local mock REST and MCP smoke coverage, plus Stage 5 real acceptance records for representative Jira and Confluence paths.
- Deferred full OAuth flows, SSE transport, SOCKS proxy, system truststore injection, Helm, external registry publishing, and blocked or local-only product acceptance items to the published backlog.
