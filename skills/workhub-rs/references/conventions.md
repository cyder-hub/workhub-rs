# CLI Conventions

This file covers shared usage and return behavior for `workhub cli jira ...`, `workhub cli confluence ...`, and `workhub cli gitlab ...`.

It intentionally excludes setup, credentials, environment loading, MCP runtime, and config commands.

## Command Shape

```bash
workhub cli [--json] [--pretty] <provider> <resource> <action> ...
```

Global output flags:

| Flag | Use | Return behavior |
| --- | --- | --- |
| `--json` | Emit machine-readable output. | Success JSON goes to stdout. Error JSON goes to stderr. |
| `--pretty` | Indent JSON for reading. | Requires `--json`; same data as compact JSON. |

## Output

Default success output is compact text:

```text
is_last: true

issues:
key     id      summary     status   project assignee
ABC-1   10001   First       Done     ABC     Ada
ABC-2   10002   Second      To Do    ABC     -
```

Rules:

- Object fields render as `key: value`.
- Array fields such as `issues`, `values`, `results`, or `items` render as table sections.
- Empty strings, nulls, and missing table cells render as `-`.
- Nested objects prefer readable identifiers such as `name`, `title`, `key`, `username`, or `id`.
- Mutations usually return a short operation summary in text mode and the underlying result object with `--json`.

Use JSON for automation:

```bash
workhub cli --json jira issue search --jql 'project = ABC' --limit 10
workhub cli --json --pretty confluence page get --id 123456 --markdown true
```

## Errors

Failures leave stdout empty and write stderr. Exit codes:

| Code | Meaning |
| --- | --- |
| `0` | Success. |
| `2` | Usage error, missing input, invalid argument, unreadable file input, or invalid JSON input. |
| `3` | Service unavailable or project/space filter rejection. |
| `4` | Upstream HTTP, transport, decode, or unexpected response-shape error. |
| `5` | Business structured error or output rendering failure. |

When a command returns a structured result with top-level `"success": false`, treat it as a business failure: stdout is empty, the same structured payload is written to stderr, and the process exits with code `5`. Automation must check both the process exit code and JSON business fields such as `success`, `partial_success`, `failed`, and `error`.

State-changing commands use a mutation envelope when implemented: `success`, `message`, `data`, and `warnings`; batch mutations also use `partial_success`, `summary`, and `failed`. No-content upstream responses use `{}` in `data`, not bare `null`. For compatibility or cleanup failures, inspect structured error categories such as `permission_denied`, `not_found`, and `unsupported_or_auth_required`.

Destructive cleanup commands may require an explicit confirmation flag equal to the target, such as `--confirm-id`, `--confirm-iid`, or `--confirm-branch`. Treat a missing or mismatched confirmation value as a hard stop; do not retry by guessing. When a delete result includes `cleanup_hint`, use the suggested read/list command or GitLab UI/API verification to verify the outcome before reporting cleanup complete.

With `--json`, stderr has this shape:

```json
{
  "success": false,
  "error": {
    "category": "invalid_input",
    "message": "fields JSON must be an object"
  }
}
```

## Input Forms

Text body flags use one source at a time:

```bash
workhub cli jira issue comment add ABC-1 --body 'Short note'
workhub cli jira issue comment add ABC-1 --body-file ./comment.md
printf 'Short note\n' | workhub cli jira issue comment add ABC-1 --body-stdin
```

JSON inline/file pairs use one source at a time:

```bash
workhub cli jira issue update ABC-1 --fields-json '{"summary":"New summary"}'
workhub cli jira issue update ABC-1 --fields-file ./jira-fields.json
```

CSV flags use comma-separated values:

```bash
workhub cli gitlab mr list group/project --labels backend,urgent --per-page 20
workhub cli jira agile sprint add-issues 42 --issues ABC-1,ABC-2
```

Boolean flags have two forms:

- Presence flags: `--include-content`, `--values-only`, `--delete-subtasks`, `--include-raw-dates`.
- Explicit booleans: `--minor-edit true`, `--resolved false`, `--released true`.

## Safety

Prefer read commands before state-changing commands when identifiers or state are uncertain:

- Use Jira issue/project/transition/list commands before updating, deleting, linking, or transitioning issues.
- Use Confluence page/attachment lookup commands before updating, moving, deleting, or downloading content.
- Use GitLab MR/project lookup commands before approving, resolving discussions, or merging. Confirm the MR state, target branch, and head SHA before merge.
