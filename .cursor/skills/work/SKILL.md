---
name: work
description: >-
  Implement a backlog.md task to done in this mise + Backlog.md + Cursor repo:
  load the task via the CLI, research and plan, implement in tight loops,
  verify against the acceptance criteria and whatever quality tasks the project
  defines, then mark it Done. Use when the user says work, start a task,
  implement a backlog task, or works through task IDs with acceptance criteria.
---

# Work

Implement a backlog task in this repo until all acceptance criteria pass.

Repo context (see `AGENTS.md`):

- mise + Backlog.md + Cursor template. Application code is added downstream — this template ships with none.
- Tooling runs through `mise`; the `backlog` CLI is **not** on PATH → always `mise exec -- backlog ...`.
- Verification means: run whatever quality/build/test tasks the project defines, plus explicit behavior checks against each acceptance criterion.

## Command discovery

- List tasks: `mise tasks` (a fresh template only has `backlog-browser`).
- Inspect a task: `mise task <name>`.
- Run: `mise run <task>`.
- Never guess a command or path — confirm a `mise` task or script exists before relying on it.

## Step 1: Load the task

```bash
mise exec -- backlog task <id> --plain
```

Read the description and every acceptance criterion. Review any references or docs on the task. If anything is ambiguous, stop and ask.

Set it in progress and assign yourself:

```bash
mise exec -- backlog task edit <id> -s "In Progress" -a @me
```

## Step 2: Research & plan

- Read referenced files and adjacent code; follow existing patterns.
- Confirm dependencies and tools already exist before proposing new ones.
- Review related docs: `mise exec -- backlog doc list`.

Record the plan on the task, then implement against it:

```bash
mise exec -- backlog task edit <id> --plan $'1. Change X because...\n2. Change Y...\n3. Verify AC'
```

## Step 3: Implement loop

Repeat until every acceptance criterion is satisfied:

1. Make one focused change.
2. Verify it — run the relevant `mise` quality/build/test task (fix and repeat until clean) and check behavior against the AC.
3. Check the AC that now passes: `mise exec -- backlog task edit <id> --check-ac <n>`.
4. Append a progress note: `mise exec -- backlog task edit <id> --append-notes $'- did X'`.

After each iteration state: what you did, which AC now pass, and what's next — or **All AC pass, ready to complete**.

## Step 4: Complete

When all AC pass and all defined quality checks are clean:

```bash
mise exec -- backlog task edit <id> --final-summary $'What changed and why.\n\nKey files, tests run, risks.'
mise exec -- backlog task edit <id> -s Done
```

## Rules

- Research before coding; never guess commands or paths.
- Never proceed past a failing quality/build/test check.
- Use existing `mise` tasks before reaching for raw binaries.
- Do not install tools or dependencies without approval.
- State progress after each iteration.
- Do not mark `Done` until every AC is verifiable and all defined checks pass.
- Keep code, comments, and responses terse.

## Related

- Task authoring: [.cursor/skills/backlog/SKILL.md](../backlog/SKILL.md)
- Documentation authoring: [.cursor/skills/doc/SKILL.md](../doc/SKILL.md)
