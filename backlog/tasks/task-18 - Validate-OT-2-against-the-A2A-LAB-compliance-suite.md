---
id: TASK-18
title: Validate OT-2 against the A2A-LAB compliance suite
status: Done
assignee:
  - '@me'
created_date: '2026-10-09 03:29'
updated_date: '2026-10-09 04:07'
labels:
  - compliance
  - backlog-implementer
dependencies:
  - TASK-17
references:
  - task-17
  - >-
    ../a2a-lab-dev-kit-rs/backlog/tasks/task-25 -
    Build-the-reusable-A2A-LAB-compliance-runner.md
priority: high
type: task
ordinal: 18000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Integrate the reusable runner from a2a-lab-dev-kit TASK-25 as a local OT-2 quality command and validate the reference implementation’s shared A2A-LAB behavior across A2A and MCP. This is the bounded consistency check for the two protocol adapters: it compares success, boundary, and failure behavior while allowing only the envelope differences named by the devkit contract. External prerequisite: a2a-lab-dev-kit TASK-25.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [x] #1 A mise task starts the deterministic OT-2 fixture, runs the devkit compliance command against both endpoints, writes the JSON report to a documented target path, and cleans up the fixture on success or failure
- [x] #2 The matrix covers representative success cases for logs, metrics, tasks, and images; pagination and half-open time boundaries; invalid limits or inputs; unknown source, metric, task, run, and image ids; and task terminal-state behavior
- [x] #3 The report identifies every compared A2A/MCP case and contains the normalized outputs for any unexplained difference; intentional protocol-envelope differences match the devkit contract
- [x] #4 The local compliance command exits zero with an overall compliant result for the deterministic OT-2 fixture and exits nonzero when a controlled fixture variant introduces a result or error-code mismatch
- [x] #5 The compliance command remains outside live smoke and does not start the pinned Docker simulator or contact physical hardware, cameras, model providers, OIDC, or SiLA
<!-- AC:END -->

## Implementation Plan

<!-- SECTION:PLAN:BEGIN -->
1. Add a local compliance command that starts the deterministic fixture, extracts its declaration, runs the sibling devkit runner, writes a stable report, and always cleans up.
2. Add a controlled MCP-only mismatch fixture variant and focused integration coverage for compliant and noncompliant command exits and report diagnostics.
3. Document the command/report boundary, verify all acceptance criteria, and run mise run quality.
<!-- SECTION:PLAN:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
- Added `mise run compliance`, backed by `.mise/scripts/compliance.sh`, to build and start the deterministic fixture, extract its declaration, invoke the sibling devkit `a2a-lab-compliance` command, write `target/compliance/a2a-lab-ot2.json`, and clean up through an EXIT trap.
- Stabilized fixture metric timestamps and current-image identities across A2A/MCP calls and seeded same-source image pagination while retaining the real OpentronsLab, A2aLabService, A2aServer, McpServer, and LiveImageCatalog paths.
- Added the controlled `mcp-metric-error` fixture variant and focused coverage proving the standard fixture is compliant while the mismatch report names the case and includes normalized A2A/MCP outputs.
- Verified `mise run compliance` exits 0 with 35 required cases passing across the 47-case profile; the controlled mismatch exits 1 and writes a noncompliant report; fixture cleanup leaves no running process in either path.
- Verification passed: focused compliance fixture tests (5/5), IDE diagnostics, and `mise run quality` (73/73 tests). The command remains separate from `smoke` and uses no Docker, hardware, camera, model, OIDC, or SiLA dependency.
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
Integrated the reusable A2A-LAB compliance runner as `mise run compliance`, with a documented report at `target/compliance/a2a-lab-ot2.json` and guaranteed fixture cleanup. The deterministic OT-2 fixture now passes all 35 required cases across A2A and MCP, while the controlled MCP metric mismatch exits nonzero and records normalized protocol evidence.

Focused tests and the full `mise run quality` gate pass (73/73 tests). TASK-19 can invoke the same fixture command or fixture CLI and upload the report; no TASK-19 or TASK-20 workflow/badge changes were made.
<!-- SECTION:FINAL_SUMMARY:END -->
