# Security Policy

## Supported Versions

Security fixes are handled on the current `master` branch and on published release tags that remain in active use. This repository does not currently maintain separate long-term support branches.

## Security Boundary

The Rust server includes:

- Unified token, header, URL query, environment secret, and upstream error redaction.
- Streamable HTTP request-scoped Basic, Token, and Bearer auth.
- Header-provided Jira and Confluence service URL validation.
- Optional `MCP_ALLOWED_URL_DOMAINS` allowlisting.
- Same-origin outbound redirect protection.
- MCP session auth fingerprint enforcement.
- Explicit rejection for unsupported SOCKS proxy, system truststore injection, and encrypted mTLS key password envs.

The current Rust release does not implement full OAuth Cloud 3LO, OAuth proxy/DCR, OAuth token refresh/storage, Data Center OAuth authorization-code/refresh, SSE transport, Helm, or external registry publishing.

## Reporting A Vulnerability

Please report security vulnerabilities through GitHub private vulnerability reporting for this repository. Do not open a public issue for an unfixed vulnerability.

Include enough detail to reproduce and assess the issue:

- Affected commit or release tag.
- Impacted component, such as service code, Docker image, compose setup, CI, release workflow, or documentation.
- Reproduction steps or proof of concept.
- Expected and observed behavior.
- Any relevant logs with secrets removed.

Maintainers will triage reports privately and publish public details after a fix or mitigation is available.
