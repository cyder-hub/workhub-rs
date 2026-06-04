#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT_DIR"

ENV_FILE="${ACCEPTANCE_ENV_FILE:-.env.dev}"

cargo build --quiet --bin mcp-atlassian-rs
"$ROOT_DIR/target/debug/mcp-atlassian-rs" acceptance jira --env-file "$ENV_FILE" --preflight
"$ROOT_DIR/target/debug/mcp-atlassian-rs" acceptance jira --env-file "$ENV_FILE" --run "$ROOT_DIR/target/debug/mcp-atlassian-rs"
