# Production Deployment

This guide describes the supported runtime shapes for `mcp-atlassian-rs`.

## Deployment Checklist

- Choose one supported transport: `stdio` for local MCP clients or streamable HTTP for server deployments.
- Configure only the Jira and Confluence services you want exposed.
- Set `READ_ONLY_MODE=true` for read-only deployments.
- Restrict exposed tools with `TOOLSETS` or `ENABLED_TOOLS` when the client should not see every configured tool.
- Set `MCP_ALLOWED_URL_DOMAINS` before accepting request-scoped Atlassian service URLs over streamable HTTP.
- Keep Atlassian credentials in a secret manager, shell environment, or orchestrator secret. Do not commit dotenv files with real credentials.
- Check `GET /healthz` for streamable HTTP deployments.
- Review `SECURITY.md` before exposing the HTTP endpoint beyond localhost.

## Stdio

Use stdio when an MCP client starts the server process directly:

```bash
mcp-atlassian-rs stdio
```

Logs are written to stderr. Stdout is reserved for the MCP protocol.

## Streamable HTTP

Use streamable HTTP for server deployments:

```bash
mcp-atlassian-rs streamhttp --host 0.0.0.0 --port 8000 --path /mcp
```

The health endpoint is:

```text
GET /healthz
```

The MCP endpoint path defaults to `/mcp` and can be set with `MCP_HTTP_PATH` or `--path`.

## Docker And Compose

Build the local image:

```bash
docker build -t mcp-atlassian-rs:local -f Dockerfile .
```

Run the image:

```bash
docker run --rm -p 8000:8000 mcp-atlassian-rs:local
```

Run with compose:

```bash
docker compose up --build
```

The image runs as a non-root `app` user. The compose service includes a `/healthz` healthcheck and maps `${MCP_PORT:-8000}` on the host to container port `8000`.

## Runtime Controls

| Variable | Deployment use |
| --- | --- |
| `READ_ONLY_MODE` | Set `true` to hide write tools from discovery and block direct write calls. |
| `TOOLSETS` | Set `all`, `default`, or comma-separated toolset names. Unknown-only values fail closed. |
| `ENABLED_TOOLS` | Set comma-separated tool names for exact tool allowlisting. |
| `MCP_HTTP_HOST` / `MCP_HTTP_PORT` / `MCP_HTTP_PATH` | Configure streamable HTTP when CLI flags are not used. |
| `IGNORE_HEADER_AUTH` | Set `true` to ignore request-scoped auth/service headers and use only global environment config. |
| `MCP_ALLOWED_URL_DOMAINS` | Restrict header-provided Jira/Confluence service URLs to exact domains or subdomains. |

## Jira And Confluence Auth

Supported global auth:

- Jira Cloud: `JIRA_URL`, `JIRA_USERNAME`, `JIRA_API_TOKEN`.
- Jira Server/Data Center: `JIRA_URL`, `JIRA_PERSONAL_TOKEN`.
- Confluence Cloud: `CONFLUENCE_URL`, `CONFLUENCE_USERNAME`, `CONFLUENCE_API_TOKEN`.
- Confluence Server/Data Center: `CONFLUENCE_URL`, `CONFLUENCE_PERSONAL_TOKEN`.
- BYOT access token: `ATLASSIAN_OAUTH_ACCESS_TOKEN`, or service-specific `JIRA_OAUTH_ACCESS_TOKEN` / `CONFLUENCE_OAUTH_ACCESS_TOKEN`.

Cloud BYOT access-token auth requires `ATLASSIAN_OAUTH_CLOUD_ID`. Jira Cloud BYOT uses `https://api.atlassian.com/ex/jira/{cloud_id}`. Confluence Cloud BYOT uses `https://api.atlassian.com/ex/confluence/{cloud_id}/wiki`.

Supported streamable HTTP request-scoped auth:

- `Authorization: Basic <base64(email:api_token)>`.
- `Authorization: Token <pat>`.
- `Authorization: Bearer <token>`.
- `X-Atlassian-Jira-Url` plus `X-Atlassian-Jira-Personal-Token`.
- `X-Atlassian-Confluence-Url` plus `X-Atlassian-Confluence-Personal-Token`.
- `X-Atlassian-Cloud-Id`.

Bearer tokens are interpreted as BYOT/OAuth access tokens when `X-Atlassian-Cloud-Id` is present or `ATLASSIAN_OAUTH_ENABLE=true`. Otherwise Bearer keeps the PAT-compatible behavior.

## Network And TLS

Supported outbound network controls:

- Jira proxy: `JIRA_HTTP_PROXY`, `JIRA_HTTPS_PROXY`, `JIRA_NO_PROXY`.
- Confluence proxy: `CONFLUENCE_HTTP_PROXY`, `CONFLUENCE_HTTPS_PROXY`, `CONFLUENCE_NO_PROXY`.
- Global proxy fallback: `HTTP_PROXY`, `HTTPS_PROXY`, `NO_PROXY`.
- Jira custom outbound headers: `JIRA_CUSTOM_HEADERS`.
- Confluence custom outbound headers: `CONFLUENCE_CUSTOM_HEADERS`.
- Jira mTLS: `JIRA_CLIENT_CERT`, `JIRA_CLIENT_KEY`.
- Confluence mTLS: `CONFLUENCE_CLIENT_CERT`, `CONFLUENCE_CLIENT_KEY`.
- TLS verification toggles: `JIRA_SSL_VERIFY`, `CONFLUENCE_SSL_VERIFY`.

Reserved auth, cookie, host, content, proxy, connection, and request-scoped Atlassian headers are rejected in custom outbound headers.

## Security Behavior

- Secret-looking values are redacted from logs, MCP debug output, compact acceptance errors, URL query values, and Atlassian error summaries.
- Header-provided service URLs are validated for scheme, hostname, blocked hostnames, IP ranges, DNS results, and optional allowed domains.
- Outbound Atlassian redirects are same-origin only and limited to 3 hops.
- Request-scoped auth applies only to the current streamable HTTP request or MCP session and does not mutate global environment config.
- `Mcp-Session-Id` is bound to the request auth fingerprint. Changing auth or token type within the same session is rejected.

## Unsupported In The Current Rust Release

- OAuth Cloud 3LO authorization-code flow.
- OAuth proxy/DCR.
- OAuth refresh/token storage.
- Data Center OAuth authorization-code/refresh.
- SSE transport.
- SOCKS proxy.
- System truststore injection through `MCP_ATLASSIAN_USE_SYSTEM_TRUSTSTORE`.
- Encrypted mTLS private key password envs.
- Helm chart.
- External registry publishing.
