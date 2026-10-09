---
id: TASK-19
title: Integrate OT-2 with the public devkit and run compliance in GitHub Actions
status: Done
assignee:
  - '@me'
created_date: '2026-10-09 03:29'
updated_date: '2026-10-09 17:38'
labels:
  - compliance
  - backlog-implementer
dependencies:
  - TASK-18
references:
  - task-18
  - >-
    https://github.com/A3Analytics/a2a-lab-dev-kit-rs/commit/15e0c68492a050715e1f50423aee55a34353c00a
  - Cargo.toml
  - Cargo.lock
  - .mise/scripts/compliance.sh
  - .github/workflows/release.yml
priority: high
type: feature
ordinal: 19000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Make a2a-lab-ot2 build and verify compliance from the public A2A-LAB devkit without a sibling checkout or private repository credential. Cargo consumption, local compliance, release builds, and the consumer-owned compliance workflow must resolve the same immutable public devkit revision, 15e0c68492a050715e1f50423aee55a34353c00a, so builds and badge evidence are reproducible. The workflow remains the repository-owned provenance source for TASK-20's badge.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [x] #1 Cargo.toml resolves a2a-lab-dev-kit from https://github.com/A3Analytics/a2a-lab-dev-kit-rs at full revision 15e0c68492a050715e1f50423aee55a34353c00a with the required features, Cargo.lock records that exact Git commit, and the repository's locked build/check tasks succeed without ../a2a-lab-dev-kit-rs
- [x] #2 mise run compliance succeeds in a checkout with no sibling devkit and no devkit access token, runs the compliance runner from immutable public revision 15e0c68492a050715e1f50423aee55a34353c00a by default, and retains the machine-readable OT-2 report; any documented local source override is explicit and does not affect the default
- [x] #3 The release workflow builds with --locked from the public Git dependency and contains no devkit checkout step, DEVKIT_READ_TOKEN reference, or other private-devkit access mechanism
- [x] #4 A workflow named A2A-LAB Compliance runs on pull requests, pushes to the default branch, and manual dispatch; invokes A3Analytics/a2a-lab-dev-kit-rs at full action SHA 15e0c68492a050715e1f50423aee55a34353c00a against the deterministic OT-2 A2A and MCP fixture; and requires no Docker, hardware, cameras, model credentials, OIDC, or SiLA
- [x] #5 The compliance workflow uploads the machine-readable report with if: always(), fails when the action reports noncompliance, and makes the tested OT-2 commit, devkit action revision, suite revision, profile version, and per-case outcomes recoverable from the run; a controlled failing case verifies the check cannot remain green
- [x] #6 Automated checks reject drift between the Cargo dependency, local compliance runner, and GitHub Action immutable devkit revisions, reject reintroduction of private devkit credentials or sibling-only defaults, and mise run quality passes
<!-- AC:END -->

## Implementation Plan

<!-- SECTION:PLAN:BEGIN -->
1. Replace the sibling Cargo dependency with the public devkit Git source pinned to 15e0c68492a050715e1f50423aee55a34353c00a, regenerate Cargo.lock, and cover immutable-pin/locked-resolution behavior.
2. Make the local compliance task obtain and run the public devkit compliance runner at that same revision by default while retaining only an explicit source override where useful.
3. Add the consumer-owned A2A-LAB Compliance workflow at the same full action SHA, deterministic fixture lifecycle, always-uploaded report, and controlled noncompliance verification.
4. Remove the release workflow devkit checkout and DEVKIT_READ_TOKEN use so --locked release builds resolve the public Git dependency directly.
5. Add bounded consistency checks for all devkit pins and obsolete private/sibling assumptions, run focused checks and mise run quality, then complete TASK-19.
<!-- SECTION:PLAN:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
- Earlier local validation passed for the standard full suite and the mcp-metric-error controlled failure, and mise run quality passed with 73 tests; these results predate the public-source migration and must be rerun after implementation.
- The former publication blocker is resolved: https://github.com/A3Analytics/a2a-lab-dev-kit-rs has public main/HEAD revision 15e0c68492a050715e1f50423aee55a34353c00a, and that revision contains the repository-root composite action plus its compliance runner.
- Scope refinement keeps build dependency migration, local runner provenance, release cleanup, and badge-producing CI together because they share one immutable devkit source contract. Badge prose and evidence documentation remain in TASK-20.

- Migrated Cargo.toml and Cargo.lock to public Git source https://github.com/A3Analytics/a2a-lab-dev-kit-rs at exact revision 15e0c68492a050715e1f50423aee55a34353c00a; retained prior transitive lock versions.
- Changed mise run compliance to fetch and verify that public revision by default, with A2ALAB_DEV_KIT_ROOT retained only as an explicit local override. Default full-suite run passed without a devkit/token environment and retained target/compliance/a2a-lab-ot2.json (47 cases, 17 scenarios).
- Added the A2A-LAB Compliance workflow for pull requests, main pushes, and manual dispatch. It runs the public action at the same full SHA, records commit/action/suite/profile provenance, uploads reports with if: always(), and asserts a controlled mcp-metric-error action outcome is failure.
- Removed the release workflow devkit checkout and DEVKIT_READ_TOKEN while retaining cargo build --locked.
- Added public_devkit_contract integration checks that reject pin drift, sibling-only defaults, and private workflow access. Controlled local noncompliance exited 1 and retained three failing cases.
- Verification passed: bash -n .mise/scripts/compliance.sh; git diff --check; mise run check; focused public_devkit_contract tests (3/3); default mise run compliance; controlled noncompliance; IDE diagnostics; mise run quality (79/79 tests).
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
OT-2 now builds and runs compliance from the public A2A-LAB devkit pinned everywhere to 15e0c68492a050715e1f50423aee55a34353c00a. Release CI no longer uses private checkout credentials, and the new A2A-LAB Compliance workflow retains provenance and reports while proving controlled noncompliance fails.

Verified the standard full suite (47 cases, 17 scenarios), controlled failure behavior, locked compilation, immutable-pin contract tests, and mise run quality (79/79).
<!-- SECTION:FINAL_SUMMARY:END -->
