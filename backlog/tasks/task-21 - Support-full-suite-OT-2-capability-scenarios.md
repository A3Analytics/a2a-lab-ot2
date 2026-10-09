---
id: TASK-21
title: Support full-suite OT-2 capability scenarios
status: Done
assignee:
  - '@me'
created_date: '2026-10-09 15:59'
updated_date: '2026-10-09 16:18'
labels:
  - compliance
  - backlog-implementer
dependencies:
  - TASK-18
references:
  - task-18
  - src/fixture.rs
  - tests/compliance_fixture.rs
priority: high
type: feature
ordinal: 21000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Extend the deterministic OT-2 fixture so profile 1.1.0 full-suite scenarios can discover and reuse real values across the existing OpentronsLab, A2A, and MCP paths. Keep the fixture hardware-independent, repeatable, and self-cleaning, and limit assertions to capability linkage rather than exhaustive provider correctness.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [x] #1 The fixture advertises at least one queryable log source, metric, executable task, and image source through both A2A and MCP, and values returned by discovery can be consumed by the corresponding query or list operation
- [x] #2 Starting a discovered task returns a run identifier that can be read through either interface until a deterministic terminal state, without relying on the predeclared run fixture
- [x] #3 Listing or searching a discovered image source returns an image identifier retrievable through either interface, and current-image retrieval returns an image linked to the requested source without requiring equality to an earlier capture
- [x] #4 The devkit's bounded cross-interface matrix succeeds against the fixture, including both interfaces as identifier producers and consumers across logs, metrics, tasks, and images
- [x] #5 Repeated full-suite runs start from isolated state and fixture shutdown removes listeners, temporary state, generated runs, and generated images owned by the fixture on success or failure
- [x] #6 Focused tests prove linked success paths, empty or broken-link diagnostics through a controlled variant, repeatability, cleanup, and no Docker, robot, camera device, cloud service, or external filesystem state
<!-- AC:END -->

## Implementation Plan

<!-- SECTION:PLAN:BEGIN -->
1. Extend the in-process fixture state and controlled variants so discovered log, metric, task, and image identifiers remain linked across A2A and MCP while generated runs/images are isolated per fixture.
2. Add focused full-suite tests for both producer/consumer directions, deterministic terminal task runs, image linkage, broken-link diagnostics, repeatability, and listener/state cleanup.
3. Run focused checks and mise run quality, verify each acceptance criterion, record implementation notes/final summary, and mark Done only when all checks pass.
<!-- SECTION:PLAN:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
- Updated the OT-2 compliance command to run profile 1.1.0 full with the unrelated LLM check explicitly disabled; all 47 operation cases and 16 non-LLM linked scenarios complete with a compliant report.
- Added full-suite fixture coverage for A2A and MCP as producers and consumers across logs, metrics, tasks, and images. Discovered task runs reach deterministic terminal state and discovered image IDs/source relationships remain reusable cross-interface.
- Added the mcp-empty-log-sources controlled variant and verified step-specific empty-discovery diagnostics remain isolated to affected linked scenarios.
- Proved repeatability with two isolated full-suite fixture processes, explicit shutdown/drop release of both listeners, startup-failure behavior, and credential/external-dependency independence. Fixture-generated runs and images remain process-local and are dropped with the service.
- Verification passed: focused compliance_fixture tests (6/6), mise run compliance, controlled noncompliant command exit followed by a clean compliant rerun, IDE diagnostics, git diff --check, and mise run quality (74/74 tests).
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
Extended the deterministic OT-2 fixture and local compliance command for profile 1.1.0 full-suite linked scenarios without adding LLM behavior. All 16 log, metric, task, and image journeys pass for same-interface and cross-interface producer/consumer paths; controlled empty discovery yields step-specific failures.

The fixture remains hardware-independent and process-local, repeats from isolated state, and releases both listeners and generated state on shutdown/drop. Focused tests, the compliance command, and mise run quality (74/74 tests) pass.
<!-- SECTION:FINAL_SUMMARY:END -->
