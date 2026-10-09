---
id: TASK-20
title: Document OT-2 compliance evidence and badge
status: Done
assignee:
  - '@me'
created_date: '2026-10-09 03:29'
updated_date: '2026-10-09 20:57'
labels:
  - compliance
  - documentation-writer
dependencies:
  - TASK-19
references:
  - task-19
  - >-
    https://github.com/A3Analytics/a2a-lab-dev-kit-rs/blob/15e0c68492a050715e1f50423aee55a34353c00a/backlog/docs/guide/compliance/doc-22%20-%20Adopt-A2A-LAB-compliance.md
  - >-
    https://github.com/A3Analytics/a2a-lab-dev-kit-rs/blob/15e0c68492a050715e1f50423aee55a34353c00a/backlog/docs/reference/compliance/doc-23%20-%20A2A-LAB-compliance-profile.md
  - README.md
  - backlog/docs/technical/testing/doc-2 - Testing-a2a-lab-ot2.md
modified_files:
  - README.md
  - backlog/docs/technical/testing/doc-2 - Testing-a2a-lab-ot2.md
priority: medium
type: docs
ordinal: 20000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Show users how a2a-lab-ot2 proves the A2A-LAB profile locally and in GitHub Actions, and display the repository-owned workflow badge without overstating the claim. The documentation must use TASK-19's public immutable devkit integration and distinguish deterministic compliance from live simulator and hardware checks. The public devkit adoption guide and compliance profile are authoritative sources for badge semantics.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [x] #1 README displays a standard GitHub Actions badge for the A2A-LAB Compliance workflow on the default branch and links to that workflow’s runs
- [x] #2 Badge text and adjacent prose say A2A-LAB compliant or compliance without using certification language and explain that green means the repository commit passed the identified profile and suite revision
- [x] #3 The testing guide provides the exact local compliance command, fixture boundaries, JSON report path, interpretation of pass, fail, and required-case skip outcomes, and a link to the uploaded CI artifact
- [x] #4 The testing guide clearly separates deterministic compliance from live smoke, the pinned robot-server simulator, real OT-2 hardware, camera availability, model-provider checks, OIDC, SiLA, and the official A2A protocol TCK
- [x] #5 Documentation links to the devkit compliance contract and adoption guide and does not imply that untested deployments or physical devices inherit the repository badge
- [x] #6 The testing guide identifies basic and full suite semantics, states that full includes basic and is the default, and documents the deterministic fake-model LLM check, its default-on behavior, and the explicit credential-free opt-out
<!-- AC:END -->

## Implementation Plan

<!-- SECTION:PLAN:BEGIN -->
1. Add the repository-owned A2A-LAB Compliance workflow badge and bounded interpretation to README.md.
2. Expand the canonical testing guide with exact basic/full/default/opt-out commands, deterministic fixture scope, report interpretation, CI artifact evidence, immutable devkit links, and explicit exclusions.
3. Verify links and commands against workflow/script/report behavior, run mise run quality, then record acceptance-criterion evidence and complete the task only if all criteria pass.
<!-- SECTION:PLAN:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
- Added the standard main-branch A2A-LAB Compliance badge and bounded green-status interpretation to README.md.
- Expanded doc-2 with exact default, basic, full, and credential-free LLM-opt-out commands; report outcomes; immutable profile/adoption links; CI evidence contents; and explicit live-system/TCK exclusions.
- Verified all four documented local command variants produce compliant reports with expected selected_suite, enabled_checks, and scenario sets.
- Verified hosted run https://github.com/A3Analytics/a2a-lab-ot2/actions/runs/37968981332 completed successfully on main for commit 9356cdd272c3ea5d5db2650e65c1c596bf83c29d.
- Downloaded and inspected artifact https://github.com/A3Analytics/a2a-lab-ot2/actions/runs/37968981332/artifacts/11634811818 (name a2a-lab-compliance-9356cdd272c3ea5d5db2650e65c1c596bf83c29d, SHA-256 216466d4cf9423131eb9eec1b42d933ed1a316ca12c67e4a0a2175eb9a22efbd, unexpired through 2027-01-07). It contains a2a-lab-ot2.json, controlled-noncompliance.json, and provenance.json.
- The primary report records compliant: true for profile 1.1.0, full suite, A2A and MCP; provenance records suite revision 15e0c68492a050715e1f50423aee55a34353c00a and action_outcome: success. The controlled report records compliant: false as expected.
- Restored the README explanation removed after the hosted run, linked the exact run and artifact from doc-2, and preserved the separate TASK-24 preparation edits.
- git diff --check, IDE diagnostics, and mise run quality passed (79/79 tests).
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
Documented and verified repository-owned local and hosted A2A-LAB compliance evidence. README carries the main-branch workflow badge and commit-scoped interpretation; doc-2 covers exact commands, suite/report semantics, exclusions, immutable contract links, and the successful run 37968981332 artifact 11634811818 for commit 9356cdd272c3ea5d5db2650e65c1c596bf83c29d.

Verification: hosted run completed successfully; artifact downloaded and inspected; primary report compliant true; controlled report compliant false; provenance profile 1.1.0/full/suite revision 15e0c68492a050715e1f50423aee55a34353c00a; git diff --check and mise run quality passed with 79/79 tests.
<!-- SECTION:FINAL_SUMMARY:END -->
