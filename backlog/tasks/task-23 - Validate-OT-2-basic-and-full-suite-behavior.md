---
id: TASK-23
title: Validate OT-2 basic and full suite behavior
status: Done
assignee:
  - '@me'
created_date: '2026-10-09 15:59'
updated_date: '2026-10-09 16:31'
labels:
  - compliance
  - backlog-implementer
dependencies:
  - TASK-22
references:
  - task-22
  - .mise/scripts/compliance.sh
  - tests/compliance_fixture.rs
priority: high
type: task
ordinal: 23000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Update the local OT-2 compliance integration to validate profile 1.1.0 basic and full behavior as a bounded consistency check across A2A and MCP. The default run must include linked scenarios and deterministic LLM-enabled behavior, while explicit selections and opt-outs remain visible in retained evidence. Publishing or pinning the devkit action remains outside this task.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [x] #1 mise run compliance defaults to profile 1.1.0 full with LLM checks enabled, writes the stable JSON report path, exits zero for the standard fixture, and cleans up all fixture-owned processes and temporary state
- [x] #2 A basic run executes the preserved operation-level matrix without full scenario or LLM cases, and its report identifies basic as the selected suite
- [x] #3 A full run reports every required basic case plus logs, metrics, tasks, images, bounded cross-interface handoffs, and the deterministic A2A agent-message to MCP-tool scenario
- [x] #4 An explicit LLM opt-out requires no model credentials, records the opt-out, and can remain compliant; the default-on controlled LLM failure exits nonzero and retains actionable report diagnostics
- [x] #5 Controlled broken-link and A2A/MCP mismatch variants fail the named scenario or case without making intentional protocol envelopes, unrelated ordering, image capture differences, or hashes compliance requirements
- [x] #6 Focused tests and the repository quality gate pass without Docker, robot or camera hardware, external model services, OIDC, SiLA, or a remotely published devkit action
<!-- AC:END -->

## Implementation Plan

<!-- SECTION:PLAN:BEGIN -->
1. Expose validated profile 1.1.0 suite and LLM controls in the local compliance wrapper while retaining a stable report and guaranteed fixture cleanup.
2. Add process-level tests for default full/default-on LLM, explicit basic and LLM opt-out, controlled mismatch/broken-link/LLM failures, diagnostics, and cleanup.
3. Run focused checks plus mise run quality, verify each acceptance criterion, and record completion evidence.
<!-- SECTION:PLAN:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
- Added validated A2ALAB_COMPLIANCE_SUITE (basic/full, default full) and A2ALAB_COMPLIANCE_LLM_CHECK (true/false, default true) controls to the local wrapper; every run replaces and retains its selected stable report.
- Added focused basic-suite evidence and strengthened controlled mismatch, broken-link, and ungrounded-agent assertions so unrelated required cases/scenarios remain passing.
- End-to-end mise run compliance verification passed for default full/default-on LLM, basic, credential-free full opt-out, and all three controlled failures. Reports recorded profile 1.1.0 selection/checks and actionable isolated diagnostics; expected success/failure exit codes and an empty fixture TMPDIR after every run proved trap cleanup.

- Verification passed without external dependencies: focused compliance_fixture tests 8/8, bash syntax, git diff --check, IDE diagnostics, six end-to-end wrapper modes, and mise run quality with 76/76 tests.
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
Exposed strict local basic/full and LLM-check controls while preserving profile 1.1.0 full plus deterministic LLM as the default. Added focused basic evidence and isolated controlled-failure assertions for mismatch, broken-link, and agent-message behavior.

Verified stable retained reports, success/noncompliance exit codes, credential-free opt-out, and fixture temporary-state cleanup across six end-to-end mise runs. Focused tests passed 8/8 and mise run quality passed 76/76 without Docker, hardware, external services, OIDC, SiLA, or a published action.
<!-- SECTION:FINAL_SUMMARY:END -->
