# Changelog

## 0.1.0 - 2026-06-05

- Implemented the Rust MCP Atlassian server with 49 Jira business tools and 24 Confluence business tools.
- Added stdio and streamable HTTP transports, `/healthz`, tool filtering, toolset filtering, service availability filtering, and read-only guards.
- Added Jira and Confluence Cloud Basic/API-token auth, Server/Data Center PAT auth, BYOT access-token support, request-scoped streamable HTTP auth, SSRF and allowed-domain checks, same-origin redirect protection, proxy/custom outbound headers, and mTLS client cert/key support.
- Added local mock REST and MCP smoke coverage, plus Stage 5 real acceptance records for representative Jira and Confluence paths.
- Deferred full OAuth flows, SSE transport, SOCKS proxy, system truststore injection, Helm, external registry publishing, and blocked or local-only product acceptance items to the published backlog.
