# Contributing

This repository is a GitHub template for a Rust MCP server. Contributions should improve the template itself: correctness, documentation, local developer workflow, CI, Docker, and generic example resources.

## Local Setup

Install the versions listed in `README.md`, then use `just --list` to inspect the local command surface.

## Verification

Run the checks that match your change. For most code, dependency, CI, or Docker changes, run the full local path:

```bash
cargo fmt --check
cargo check
cargo test
docker compose -f docker-compose.yml config
docker build -t cyder-mcp-template:ci -f Dockerfile .
```

The shorter project shortcut is:

```bash
just check
```

`just check` covers Rust formatting, compilation checks, and tests. The checked-in GitHub Actions workflow mirrors the direct service, compose, and Docker build checks above. Run the direct Docker commands when you change `Dockerfile`, `docker-compose.yml`, `.dockerignore`, transport commands, or release packaging.

## Pull Requests

Create a branch from the current `main` branch and keep the pull request focused on one change. Include a concise summary, the verification commands you ran, and any follow-up work that remains.

Before opening a pull request:

- Keep generated and local files out of the commit, including `target/`, `.env`, logs, or credentials.
- Do not commit real credentials, tokens, private endpoints, or machine-specific config.
- Keep the main README in English.
- Do not add product claims for features the template does not implement, such as authentication, authorization, persistence, deployment automation, or a frontend.
- Update README and template rename guidance when changing `cyder-mcp-template`, `cyder_mcp_template`, Docker image names, transport behavior, or automation commands.

## Code Style

Use `cargo fmt` for Rust formatting. Prefer the project patterns already present in `src/`, `Dockerfile`, and `docker-compose.yml`.

## Dependency Updates

Dependency update pull requests should include the relevant lockfile changes and should not bundle unrelated refactors. Before merging dependency updates, run the release validation set:

```bash
cargo fmt --manifest-path Cargo.toml --check
cargo check --manifest-path Cargo.toml
cargo test --manifest-path Cargo.toml
docker compose -f docker-compose.yml config
docker build -t cyder-mcp-template:ci -f Dockerfile .
```

The direct commands above match the publishing checklist.
