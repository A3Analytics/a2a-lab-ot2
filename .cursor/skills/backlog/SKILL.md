---
name: backlog
description: >-
  Author atomic, testable, AI-implementable backlog.md tasks in this
  mise + Backlog.md + Cursor template repo. Uses the Backlog.md CLI (via
  `mise exec --`) for all task operations — metadata and content. Use when the
  user says /backlog, works with backlog tasks, references task IDs, or the repo
  has `backlog/config.yml`.
---

# Backlog.md Task Authoring

Author atomic, testable, AI-implementable tasks for this repo's `backlog.md` workflow.

## Repo conventions

- CLI is **not** on PATH — always run via `mise exec -- backlog ...`.
- `task_prefix: task` → IDs are `task-N` (see `backlog/config.yml`).
- Statuses (only these): `To Do`, `In Progress`, `Done`.
- `labels: []` — free-form; reuse existing labels before inventing new ones.
- `auto_commit: false` — commit task changes intentionally.
- Docs go through the `doc` skill, never as loose markdown (see `AGENTS.md`).

## Golden rule

**Never hand-edit files in `backlog/tasks/`.** Every create, edit, and status change goes through the CLI so metadata, Git tracking, and relationships stay in sync. Read with `--plain` for clean output.

## Create a task

Put content in at creation time via CLI flags:

```bash
mise exec -- backlog task create "Add health-check endpoint" \
  -d "Expose a liveness probe so deploys can gate on readiness." \
  --ac "GET /healthz returns 200 with {status:'ok'}" \
  --ac "Endpoint is unauthenticated and excluded from logging" \
  -l api --priority medium
```

Add an implementation plan only *after* starting work:

```bash
mise exec -- backlog task edit <id> --plan $'1. Add route\n2. Wire handler\n3. Verify'
```

## Section responsibilities

- **Description** (`-d`): the *why* and *what*. No implementation steps.
- **Acceptance Criteria** (`--ac`): observable, testable outcomes.
- **Implementation Plan** (`--plan`): the *how*. Added after moving to In Progress.
- **Implementation Notes** (`--append-notes`): progress log, appended as you work.
- **Final Summary** (`--final-summary`): PR-style description, added at the end.

## Quality rules

Tasks must be:

- **Atomic** — a single PR's worth of work.
- **Independent** — never depend on a higher-numbered task.
- **Testable** — AC are verifiable outcomes (endpoint returns X, command exits 0), not steps.
- **AI-implementable** — clear enough for another agent to execute unattended.

Never:

- Hand-edit task markdown or frontmatter.
- Put implementation steps in Description or AC.
- Depend on a future/higher task ID.
- Mark `Done` before all AC are checked and a Final Summary is written.

## Common commands

All prefixed with `mise exec --`.

- Create: `mise exec -- backlog task create "Title" -d "..." --ac "..." -l label --priority high`
- List: `mise exec -- backlog task list --plain`
- View: `mise exec -- backlog task <id> --plain`
- Search: `mise exec -- backlog search "topic" --plain`
- Status: `mise exec -- backlog task edit <id> -s "In Progress"`
- Assign / label: `mise exec -- backlog task edit <id> -a @me -l api`
- Dependency: `mise exec -- backlog task edit <id> --dep task-1`
- Check AC: `mise exec -- backlog task edit <id> --check-ac 1 --check-ac 2` (one flag per index; no ranges/commas)
- Board UI: `mise run backlog-browser`

Multi-line values use ANSI-C quoting: `--notes $'Line 1\nLine 2'`.

## Workflow

1. Start: `mise exec -- backlog task edit <id> -s "In Progress" -a @me`
2. Add the plan, then implement against it (see the `work` skill).
3. Check AC as they pass; append notes as you go.
4. Add a Final Summary, then set `-s Done`.

## Related

- Implementation loop: [.cursor/skills/work/SKILL.md](../work/SKILL.md)
- Documentation authoring: [.cursor/skills/doc/SKILL.md](../doc/SKILL.md)
