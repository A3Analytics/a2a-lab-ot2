# AGENTS.md

Operating guide for AI agents working in this repository.

## What this repo is

`a2a-lab-ot2` — an example lab agent that wraps a persistent Opentrons OT-2 `robot-server` development simulator with the sibling `a2a-lab-dev-kit` crate. The `a2a-lab-ot2` executable talks to the OT-2 HTTP API. With no subcommand it also serves those operations over A2A and MCP. Robot-server routes from the pinned `v10.0.0` OpenAPI are classified in `src/opentrons/inventory.rs` and advertised through `list-tasks`, `list-log-sources`, and `list-metrics` (see doc-1).

## Tooling

Everything runs through `mise`. The `backlog` binary is provided by mise and is **not on PATH** — always invoke it as `mise exec -- backlog ...`.

- Install tools: `mise install`
- List tasks: `mise tasks`
- Run a task: `mise run <task>`
- Configured tools: Rust 1.98.1, Python 3.12, uv, cargo-nextest, and Backlog.md (`mise.toml`)
- Quality gate: `mise run quality`

Do not run `cargo`, `uv`, `make`, or `curl` from README/CI instructions. Use `mise run` or `mise exec --`. Internal `.mise/scripts/` may call pinned tools and `docker`.

Simulator pin: Opentrons git tag `v10.0.0` only. Never `edge`, `latest`, or a branch. The simulator runs as a Linux Docker image so robot-server has `journalctl` / journald.

### Devkit dependency

- Pin `a2a-lab-dev-kit` to a Git commit available from its upstream repository. Never leave a sibling-checkout `path` dependency or `[patch]` override in the checked-in Cargo configuration.
- Keep temporary sibling overrides in local, untracked Cargo configuration. Disable the override before final verification and make sure `Cargo.lock` records the pinned Git source.
- After changing the devkit pin, verify with `mise exec -- cargo metadata --locked --format-version 1` without local overrides. CI builds from a standalone OT2 checkout.

### Rust build targets and cache

- Reuse existing Cargo targets. Add integration test cases to an existing test file or to a module it imports from a subdirectory of `tests/`. Cargo auto-discovers top-level Rust test files as separate executables. Add a new Cargo target only when the user explicitly requests one.
- Use the existing `mise` tasks and their configured target directories. Native check, Clippy, and test tasks share `target/quality`. Keep the existing Docker and cross-compilation outputs separate. Avoid extra target directories or ad hoc changes to features, profiles, toolchains, and `RUSTFLAGS`.
- For focused Rust tests, run `mise run test -- --test <target> -E '<filter>'`. Run the full quality gate for final verification.
- Before a cold full build or after unexpected cache growth, check `du -sh target` and `df -h .`. If `target` exceeds 50 GB or free space drops below 50 GB, diagnose the growth before another full build.
- When cleaning, make sure builds and tests are idle. Preview scoped cleanup with `mise exec -- cargo clean --dry-run --profile dev --target-dir target/quality`. Preserve deployment binaries, simulator data, and reports.

## Cursor skills

Repo-specific agent skills live in `.cursor/skills/`:

- `backlog` — author atomic, testable backlog tasks
- `doc` — author backlog docs
- `work` — implement a task to done

Ignore `apps/web` examples in those skills. This repository is a Rust lab-agent example.

## Documentation policy

No loose markdown files anywhere in the repo except `README.md` and `AGENTS.md`. All other docs go through Backlog.md under `backlog/docs/`.

- `mise exec -- backlog doc list`
- `mise exec -- backlog doc view <id>`
- `mise exec -- backlog doc create "Title" -p <path> -t <type>`

Rules:

- Edit the generated file in `backlog/docs/`.
- Frontmatter must set `audience: technical` (internal) or `audience: public` (web).
- Routing: technical → `-t technical -p technical/<topic>`; other → `-t guide|reference|overview -p <path>`.
- Path is defined by directory structure, not frontmatter.
- No HTML comments in docs.

## Task policy

All task operations go through the Backlog.md CLI — **never hand-edit files** in `backlog/tasks/`. Direct edits break metadata and relationships. The entire `backlog/` directory is gitignored. Use `--plain` when reading for clean output.

Statuses (only these): `To Do`, `In Progress`, `Done`.

### Common commands

- Create: `mise exec -- backlog task create "Title" -d "Why" --ac "Testable outcome"`
- List / view: `mise exec -- backlog task list --plain` · `mise exec -- backlog task <id> --plain`
- Search: `mise exec -- backlog search "topic" --plain`
- Start: `mise exec -- backlog task edit <id> -s "In Progress" -a @me`
- Plan / notes / summary: `--plan "..."` · `--append-notes "..."` · `--final-summary "..."`
- Check criteria: `--check-ac 1 --check-ac 2` (one flag per index; no ranges or commas)
- Finish: `mise exec -- backlog task edit <id> -s Done`

For multi-line values use ANSI-C quoting: `--notes $'Line 1\nLine 2'`.

### Task quality

- Atomic (one PR), independent, and testable — acceptance criteria are observable outcomes, not implementation steps.
- Never reference a higher-numbered task.
- Description = the _why/what_; Implementation Plan (added only after starting) = the _how_.

### Definition of Done

All acceptance criteria checked · Implementation Notes written · Final Summary added · `mise run quality` passes · status set to `Done` via CLI.

Full CLI reference: `mise exec -- backlog --help`.

## Core rules

- Prefer editing existing files over creating new ones.
- Use `mise` tasks before raw binaries; never guess commands or paths.
- Keep code, comments, and responses terse.
- Don't install tools or dependencies without approval.
- Application behavior is not OS-specific. A capability comes from a library that is either present or absent. Do not gate features, configuration, or code paths on the operating system.
