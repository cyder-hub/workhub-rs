---
name: workhub-rs
description: "User-facing business command guide for the installed workhub CLI. Use when a user wants to operate Jira, Confluence, or GitLab with `workhub cli ...`: choose the right business command, form arguments/body/file/stdin/JSON inputs, and explain default text or JSON returns. Do not use for installation, builds, server/MCP runtime, deployment, environment setup, credential configuration, or `workhub cli config` guidance."
---

# Workhub Business CLI Guide

Use this skill only for business operations through an already installed and usable `workhub` CLI.

It covers:

- Jira issues, projects, fields, Agile, service desk, users, attachments, worklogs, links, and related data.
- Confluence content search, pages, comments, labels, users, versions, analytics, and attachments.
- GitLab users, projects, merge requests, notes, discussions, approvals, merges, branches, and cleanup.

It does not cover installation, building, deployment, MCP server behavior, credentials, environment variables, global config files, or `workhub cli config`.

## Reference Map

Read only the file needed for the user's task:

1. Shared command syntax, input forms, output, errors, and safety: [references/conventions.md](references/conventions.md).
2. Jira business commands: [references/jira.md](references/jira.md).
3. Confluence business commands: [references/confluence.md](references/confluence.md).
4. GitLab business commands: [references/gitlab.md](references/gitlab.md).

## CLI Shape

Use this shape for business commands:

```bash
workhub cli [--json] [--pretty] <provider> <resource> <action> ...
```

Provider entry points:

```bash
workhub cli jira ...
workhub cli confluence ...
workhub cli gitlab ...
```

Use `--json` when the user needs automation-friendly output. Use `--pretty` only with `--json`.

Use built-in help only to verify exact flag spelling when needed:

```bash
workhub cli jira issue search --help
workhub cli confluence page create --help
workhub cli gitlab mr merge --help
```

## Command Selection

Choose Jira for issue tracking work: issue lookup/search/create/update/delete, comments, transitions, watchers, worklogs, attachments, project metadata, Agile boards/sprints, service desk queues, and Jira user/field lookup.

Choose Confluence for knowledge-base work: content search, page lookup/create/update/delete/move, page comments, page versions/diffs, labels, users, analytics, and attachments.

Choose GitLab for source-control review work: current user, project lookup, merge request listing/details/diffs/commits/pipelines, MR notes/discussions, approvals, merges, branch creation/deletion, and cleanup.

For write, delete, move, approval, and merge operations, first use a read command when the target ID, branch, page, issue, SHA, or current state is uncertain. Only suggest or run destructive or state-changing commands when the user clearly intends that action.
