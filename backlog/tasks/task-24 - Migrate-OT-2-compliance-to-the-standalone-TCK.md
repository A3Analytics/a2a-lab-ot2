---
id: TASK-24
title: Migrate OT-2 compliance to the standalone TCK
status: Done
assignee:
  - '@me'
created_date: '2026-10-09 20:00'
updated_date: '2026-10-09 21:03'
labels:
  - compliance
  - migration
  - backlog-implementer
dependencies:
  - TASK-20
references:
  - .github/workflows/a2a-lab-compliance.yml
  - .mise/scripts/compliance.sh
  - tests/public_devkit_contract.rs
  - backlog/docs/technical/testing/doc-2 - Testing-a2a-lab-ot2.md
  - 'https://github.com/A3Analytics/a2a-lab-tck'
priority: high
type: enhancement
ordinal: 24000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Move OT-2's local and GitHub compliance evidence from the legacy devkit v0.1.0 action to one published immutable A3Analytics/a2a-lab-tck revision. Keep the application dependency on the public devkit independent from the TCK runner, preserve the consumer-owned workflow badge and machine-readable reports, and retain both implementation and suite provenance.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [x] #1 Cargo.toml and Cargo.lock continue to resolve the public devkit needed by a2a-lab-ot2 independently of the TCK, with no TCK dependency in the application crate and no sibling-only or private-token requirement
- [x] #2 mise run compliance fetches and verifies one published full a2a-lab-tck commit SHA by default, supports an explicit local TCK source override, preserves basic/full/default-on deterministic LLM controls, and retains the stable machine-readable report path
- [x] #3 The consumer-owned A2A-LAB Compliance workflow invokes A3Analytics/a2a-lab-tck at that same full commit SHA for both compliant and controlled-noncompliant runs, uploads available JSON evidence with if: always(), and preserves failure status
- [x] #4 Provenance records separately identify the OT-2 implementation commit, public devkit dependency revision, standalone TCK action/suite revision, profile 1.1.0, selected full suite, LLM-check state, and workflow outcome
- [x] #5 Contract tests reject drift between local and workflow TCK pins, reject legacy devkit action coordinates and private/sibling TCK access, and do not require the devkit code revision to equal the TCK suite revision
- [x] #6 README and the testing guide link to the standalone authoritative TCK profile and adoption guidance, retain the OT-2 repository-owned badge semantics, explain migration from devkit action v0.1.0, and preserve existing hardware/simulator/provider exclusions
- [x] #7 Default full, basic, explicit LLM opt-out, controlled noncompliance, focused contract tests, and mise run quality pass without Docker, hardware, external model credentials, OIDC, or SiLA
<!-- AC:END -->

## Implementation Plan

<!-- SECTION:PLAN:BEGIN -->
1. Separate the application devkit dependency pin from the standalone TCK source and immutable revision.
2. Migrate local compliance and the consumer-owned workflow to the standalone TCK while preserving reports, deterministic LLM controls, noncompliance evidence, and failure status.
3. Update contract tests, README, and testing guidance for official TCK terminology and independent provenance.
4. Verify focused contracts and all required compliance modes, then run mise run quality and complete task metadata.
<!-- SECTION:PLAN:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
- Preserved the application dependency boundary: Cargo.toml and Cargo.lock resolve public a2a-lab-dev-kit commit 9d5327868d96b3e800fd89f6debf434bcc12709d with no TCK crate dependency.
- Migrated the local runner and consumer-owned A2A-LAB Compliance workflow to official standalone A3Analytics/a2a-lab-tck v0.1.0 commit 1fd47d5c73e6fa3f7bd6505d75e43ee68cae33fd; both compliant and controlled-noncompliant Action invocations use that full SHA.
- Added separate implementation, devkit dependency, TCK Action/suite, profile 1.1.0, full-suite, LLM-check, and workflow-outcome provenance fields; retained always-uploaded JSON evidence and failure propagation.
- Strengthened contract tests for independent pins, local/workflow TCK pin parity, legacy devkit Action rejection, and private/sibling access rejection.
- Updated README and doc-2 with official A2A-LAB TCK terminology, immutable authoritative links, legacy devkit Action v0.1.0 migration, repository-owned badge semantics, historical hosted-evidence caveat, and preserved exclusions including the separate upstream A2A protocol TCK.
- Verification: focused public_devkit_contract 4/4 passed; default fetched full TCK passed with deterministic LLM check; local-override basic and full LLM-opt-out passed; controlled noncompliance exited 1 and retained compliant: false evidence; git diff --check and IDE diagnostics passed; mise run quality passed 80/80 tests.
- Hosted-workflow caveat: no standalone TCK GitHub run or artifact exists for these uncommitted changes because this task does not commit or push; the documented prior hosted artifact remains explicitly labeled legacy devkit Action evidence.
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
Migrated OT-2 compliance from the legacy devkit Action to the official standalone A2A-LAB TCK v0.1.0 at immutable commit 1fd47d5c73e6fa3f7bd6505d75e43ee68cae33fd while keeping the application devkit dependency independently pinned at 9d5327868d96b3e800fd89f6debf434bcc12709d. Updated local and hosted runners, separate provenance, drift/security contract tests, badge semantics, and migration documentation.

Verification: default full, basic, full LLM opt-out, and controlled-noncompliance modes behaved as required; focused contracts passed 4/4; mise run quality passed 80/80. No hosted standalone-TCK run was possible without committing/pushing, so the previous hosted artifact is documented only as legacy evidence.
<!-- SECTION:FINAL_SUMMARY:END -->
