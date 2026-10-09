---
id: TASK-20
title: Document OT-2 compliance evidence and badge
status: In Progress
assignee:
  - '@me'
created_date: '2026-10-09 03:29'
updated_date: '2026-10-09 17:42'
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
- [ ] #3 The testing guide provides the exact local compliance command, fixture boundaries, JSON report path, interpretation of pass, fail, and required-case skip outcomes, and a link to the uploaded CI artifact
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
- Expanded doc-2 with verified default, basic, explicit full, and credential-free LLM-opt-out commands; report outcomes; immutable profile/adoption links; CI evidence contents; and explicit live-system/TCK exclusions.
- Verified all four documented command variants produce compliant reports with the expected selected_suite, enabled_checks, and scenario sets. Verified the immutable public profile and adoption sources, git diff --check, IDE diagnostics, and mise run quality (79/79 tests).
- Hosted evidence remains blocked on publication: the repository workflow-runs URL currently reports no matching runs, so no uploaded artifact URL can be observed or linked. AC #3 remains unchecked solely for that hosted-artifact link; the guide accurately links the workflow runs page and names the expected artifact and files without claiming a run.
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
Documented local and GitHub Actions A2A-LAB compliance evidence in README.md and doc-2, including the standard main badge, profile 1.1.0/basic/full/default-on fake-model semantics, opt-out, report interpretation, immutable suite revision, and non-certification boundaries. Local command variants and mise run quality pass. TASK-20 remains In Progress because no hosted workflow run or uploaded artifact exists yet; publish the workflow/docs on main, observe a run, and replace or supplement the runs-page link with the resulting artifact evidence before checking AC #3 and marking Done.
<!-- SECTION:FINAL_SUMMARY:END -->
