---
name: doc
description: >-
  Author backlog.md documentation in this mise + Backlog.md + Cursor template
  repo. Creates docs via the Backlog.md CLI (structure) and markdown (content),
  with path-based layout under `backlog/docs/` and the required `audience`
  frontmatter. Always invokes the CLI via `mise exec --`. Use when the user says
  /doc, backlog docs, doc create, or asks to write docs in this repo.
---

# Backlog.md Documentation Authoring

Author documentation for this repo. Per `AGENTS.md`, no loose markdown anywhere — all docs live under `backlog/docs/` and are created via the CLI.

## Repo conventions

- CLI is **not** on PATH — always run via `mise exec -- backlog ...`.
- Frontmatter **requires** `audience: technical` (internal) or `audience: public` (web).
- Path is defined by directory structure, **not** frontmatter.
- No HTML comments anywhere in doc bodies.
- IDs are `doc-N` (assigned by the CLI).

## Separation of concerns

- **CLI**: structure only — id, title, type, path.
- **Markdown**: all body content.

## Create a doc

### 1. Stub via CLI

Routing (per `AGENTS.md`):

- Technical docs: `-t technical -p technical/<topic>`
- Other docs: `-t guide|reference|overview -p <path>`

```bash
mise exec -- backlog doc create "Architecture Overview" -p technical/architecture -t technical
```

This writes `backlog/docs/technical/architecture/doc-<N> - Architecture-Overview.md` with minimal frontmatter.

### 2. Edit the generated file

Add `audience` to frontmatter and write plain markdown:

```markdown
---
id: doc-<N>
title: Architecture Overview
type: technical
audience: technical
---

# Architecture Overview

## Purpose

Plain markdown. One H1 matching the title, then H2/H3 sections.
```

- `type` ∈ `technical | guide | reference | overview`.
- `audience` is required.
- Do not add `path` or fields not already used by existing docs.

## Quality rules

Docs must:

- Have a clear title and a `type` that matches the path.
- Sit at a path consistent with the existing tree.
- Use a single H1 (matching `title`), then H2/H3.
- Cross-reference related docs (`doc-N`) and tasks (`task-N`) where useful.

Never:

- Add HTML comments (those are task-only markers).
- Put the path in frontmatter.
- Create markdown files outside `backlog/docs/`.
- Mix unrelated topics — split into sibling docs at the same path.

## Common commands

All prefixed with `mise exec --`.

- Create: `mise exec -- backlog doc create "Title" -p <path> -t <type>`
- List: `mise exec -- backlog doc list`
- View: `mise exec -- backlog doc view <id>`
- Board UI: `mise run backlog-browser`

## Workflow

1. Decide path and type; check `mise exec -- backlog doc list` to avoid duplication.
2. Stub: `mise exec -- backlog doc create "<Title>" -p <path> -t <type>`.
3. Edit the generated file: add `audience`, write the body.
4. Cross-link related `doc-N` and any `task-N` that consumes the doc.

## Related

- Task authoring: [.cursor/skills/backlog/SKILL.md](../backlog/SKILL.md)
- Implementation loop: [.cursor/skills/work/SKILL.md](../work/SKILL.md)
