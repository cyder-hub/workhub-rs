# Changelog

## Unreleased

## 0.5.0 - 2026-06-17

### Added
- Added Jira MCP tools and CLI commands for deleting issue comments, updating and deleting worklogs, listing issue links, listing remote issue links, and deleting remote issue links.
- Added Confluence MCP tools and CLI commands for updating and deleting page comments and removing content labels.
- Added GitLab MCP tools and CLI commands for closing and deleting merge requests, updating and deleting merge request notes, listing merge request discussions, and creating or deleting branches.

### Changed
- State-changing CLI commands now use a consistent mutation response envelope with `success`, `message`, `data`, and `warnings`; batch-style responses also include `partial_success`, `summary`, and `failed` where applicable.
- No-content mutation successes now return an empty object in `data` instead of `null`.
- Removed `--validate-only` from Jira batch issue creation.
- Confluence page and attachment delete commands now require a matching confirmation id.

### Fixed
- CLI commands returning a structured `{"success": false}` payload now write the failure payload to stderr and exit with code `5`.
- Jira bulk issue creation now rejects the removed `validate_only` argument instead of ignoring it.
- Confluence page comment update and delete now verify that the comment belongs to the requested page before making the change.

## 0.4.5 - 2026-06-16

### Changed
- Linux/macOS and Windows installers now resolve the latest version through the GitHub release redirect and download assets from the latest-release download URL, avoiding the GitHub REST API.

## 0.4.4 - 2026-06-16

### Fixed
- Fixed Windows installer detection of the local application data directory and CPU architecture in Windows PowerShell.
- Fixed Windows installer error handling for empty latest-release tags and empty checksum downloads.

## 0.4.3 - 2026-06-16

### Added
- Added interactive Linux/macOS and Windows installers for installing, updating, or uninstalling the default local `workhub` binary from GitHub Releases.

## 0.4.2 - 2026-06-16

### Added
- Added interactive prompt controls to `workhub cli config setup`, including confirmations, text inputs, hidden secret inputs, and menu-based auth-method selection.
- Added Jira, Confluence, and shared Atlassian Server/Data Center setup choices for either personal-token auth or username/password auth.

### Fixed
- Invalid setup input, including malformed service URLs, now stays on the current prompt until corrected instead of exiting the wizard.
- Existing setup choices now show whether each auth method is configured or partially configured.

## 0.4.1 - 2026-06-15

### Added
- Added global CLI configuration commands: `workhub cli config path`, `show`, `setup`, `set`, and `unset`.
- Added per-platform global CLI config files under the user's application config directory.

### Changed
- `workhub cli` now loads environment values from explicit `--env-file`, then `ENV_FILE`, then the global CLI config file, then the current directory `.env`.
- `streamhttp` dotenv loading now searches the current directory and parent directories for a default `.env`.

### Fixed
- `workhub cli config show` now redacts secret values before printing configuration.
- `workhub cli config setup` now rejects service URLs that runtime startup would reject, including non-HTTP(S), hostless, and malformed Jira, Confluence, and GitLab URLs.

## 0.4.0 - 2026-06-12

### Added
- Added the production `workhub cli` command surface for resource-oriented Jira, Confluence, and GitLab operations.
- Added CLI text output by default, compact JSON with `--json`, pretty JSON with `--pretty`, stdout-only success output, stderr-only errors, and stable exit-code categories.
- Added CLI dotenv loading through `--env-file`, `ENV_FILE`, or default `.env` discovery.

### Changed
- Renamed the package and release binary from `mcp-workhub-rs` to `workhub-rs` and `workhub`.
- Renamed MCP tool visibility environment variables to `MCP_TOOL_PROFILE`, `MCP_TOOLSETS`, `MCP_ENABLED_TOOLS`, and `MCP_DISABLED_TOOLS`.
- `workhub cli` uses service configuration and project/space filters but ignores MCP tool visibility controls.
- Streamable HTTP now uses process-wide service configuration only; request-scoped Jira and Confluence credential or service URL overrides were removed.
- Jira and Confluence OAuth/BYOT access-token environment authentication was removed; use API-token, PAT, or username/password auth instead.
- GitLab OAuth access-token environment authentication was removed; use `GITLAB_TOKEN` or `GITLAB_PERSONAL_TOKEN` instead.

### Fixed
- Successful env-file-backed CLI commands no longer print dotenv diagnostics to stdout.
- CLI startup configuration failures now use the CLI error contract, including JSON formatting and config exit code `3`.

## 0.3.0 - 2026-06-10

### Added
- Added `mcp-workhub-rs -v` to print only the package version and exit.

### Changed
- Renamed the released package, binary, and runtime identity from `mcp-atlassian-rs` to `mcp-workhub-rs`, including Docker image, Compose service, and release artifact names.
- Updated the default tracing target namespace from `mcp_atlassian_rs` to `mcp_workhub_rs`; custom `RUST_LOG` filters should use the new module path.

## 0.2.0 - 2026-06-10

### Added
- Added GitLab merge request MCP tools for project lookup, merge request reads, commits, bounded diffs, pipelines, create/update, notes, discussions, approvals, and SHA-gated merge.
- Added GitLab configuration through `GITLAB_URL`, token variables, project allowlisting, proxy, TLS, custom headers, and mTLS settings.
- Added profile-based MCP tool access through `TOOL_PROFILE`, `TOOLSETS`, `ENABLED_TOOLS`, and `DISABLED_TOOLS`.
- Added Jira and Confluence Server/Data Center username/password Basic Auth via `JIRA_PASSWORD`, `CONFLUENCE_PASSWORD`, and shared `ATLASSIAN_PASSWORD` fallbacks.
- Added shared Atlassian fallback variables for username/API-token and personal-token credentials.

### Fixed
- Corrected advertised MCP output schemas.

### Changed
- Replaced `READ_ONLY_MODE` with profile and exact-tool access controls. The default `basic` profile exposes a smaller safe tool set, while `DISABLED_TOOLS` blocks direct calls even when a tool is otherwise enabled.

## 0.1.2 - 2026-06-08

### Added
- feat: Added `MCP_TOOL_CALL_DEBUG` to enable MCP tool-call diagnostics when `RUST_LOG` is unset, including tool names, elapsed time, failures, and redacted arguments.
- docs: Added repository contributor guidelines in `AGENTS.md`.

### Fixed
- fix(jira): Fixed Server/Data Center search expand array payload handling.
- fix(jira): Resolved `currentUser()` and `me` Jira user profile lookups.

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

- Implemented the Rust MCP server with 49 Jira business tools and 24 Confluence business tools.
- Added stdio and streamable HTTP transports, `/healthz`, tool filtering, toolset filtering, service availability filtering, and write-tool guards.
- Added Jira and Confluence Cloud Basic/API-token auth, Server/Data Center PAT auth, username/password fallback for private instances, same-origin redirect protection, proxy/custom outbound headers, and mTLS client cert/key support.
- Added local mock REST and MCP smoke coverage, plus real acceptance records for representative Jira and Confluence paths.
- Deferred full OAuth flows, SSE transport, SOCKS proxy, system truststore injection, Helm, external registry publishing, and blocked or local-only product acceptance items to the published backlog.
