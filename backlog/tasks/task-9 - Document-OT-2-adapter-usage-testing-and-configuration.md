---
id: TASK-9
title: 'Document OT-2 adapter usage, testing, and configuration'
status: Done
assignee:
  - '@me'
created_date: '2026-10-07 02:11'
updated_date: '2026-10-07 02:21'
labels:
  - ot2
dependencies: []
priority: medium
ordinal: 9000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Users need a concise introduction to how the OT-2 adapter implements a2a-lab, safe quickstarts for the pinned simulator and real hardware, and authoritative operational references for testing and command configuration. The documentation must clearly distinguish protocol exposure and security boundaries while preserving doc-1 as the canonical robot-server mapping specification.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [x] #1 README explains how OT-2 resources map to list_tasks, start_task, get_task_status, list_log_sources, query_logs, list_metrics, and query_metric; identifies agent-message as A2A-only; and links the testing, command/configuration, and HTTP-mapping technical docs.
- [x] #2 README provides runnable mise-based quickstarts for the pinned v10.0.0 simulator and compatible real OT-2 hardware. The hardware flow defaults to --readonly, contains no bundled mutation or intentional-failure example, and states that readonly inventory filtering is neither authorization nor a physical-safety control.
- [x] #3 README accurately summarizes the robot-server, A2A, MCP, and SiLA endpoints and explicitly states that OIDC protects A2A only, while MCP and SiLA remain outside that protection.
- [x] #4 A technical document under backlog/docs/technical/testing/ documents prerequisites; fmt-check, check, clippy, test, quality, and smoke; test coverage; Docker model and ports; smoke-test side effects and the smoke ok success signal; and troubleshooting. It distinguishes local quality checks from live simulator and model-provider validation.
- [x] #5 A technical document under backlog/docs/technical/cli/ documents every generated global option, environment variable, default and precedence rule, model provider and credential requirement, subcommand and argument, and observable output. It also covers fixed versus configurable listeners, OIDC's A2A-only boundary, SiLA's all-or-none PEM requirement and persisted paths, readonly semantics, synthetic journal fallback records, and startup/failure behavior.
- [x] #6 Doc-1 remains the canonical provider classification and start-task mapping specification; revisions remove or link out duplicated operational material without dropping mapping semantics introduced by task-7 or task-8.
- [x] #7 All documentation uses only repository-approved mise commands, contains no raw cargo, uv, or curl instructions, has valid Backlog frontmatter and working internal links, and matches generated top-level and subcommand help.
- [x] #8 mise exec -- backlog doc list lists both new technical documents and mise run quality passes. If live prerequisites are available, simulator setup, health, and smoke validation completes with smoke ok; otherwise the task notes why live validation was skipped.
<!-- AC:END -->

## Implementation Plan

<!-- SECTION:PLAN:BEGIN -->
1. Capture generated help and source-only credential variables.
2. Refocus README.md on adapter semantics, safe simulator and hardware quickstarts, endpoint boundaries, and links.
3. Create technical/testing and technical/cli docs through the Backlog CLI and complete them.
4. Link operational details out of doc-1 without dropping mapping semantics.
5. Verify help coverage, links, frontmatter, mise run quality, and live smoke if prerequisites exist.
<!-- SECTION:PLAN:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
- Refocused README.md on the seven a2a-lab operations, A2A-only agent-message, readonly limits, and simulator versus hardware quickstarts.
- Added doc-2 for local quality checks and live smoke, and doc-3 for the full command and configuration surface.
- Pointed doc-1 operational listener and authentication detail at doc-3 while keeping route classification and start-task mapping.
- mise run quality passed. mise run smoke printed smoke ok against the v10.0.0 simulator.
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
Documentation now explains how the OT-2 adapter implements a2a-lab, with separate simulator and readonly hardware quickstarts.

README.md covers the seven operations, agent-message, endpoint authentication, and links to doc-1, doc-2, and doc-3. doc-2 covers the quality gate and smoke test. doc-3 covers every CLI option, environment variable, subcommand, and output. doc-1 remains the HTTP mapping specification.

Verified with mise run quality and mise run smoke (smoke ok).
<!-- SECTION:FINAL_SUMMARY:END -->
