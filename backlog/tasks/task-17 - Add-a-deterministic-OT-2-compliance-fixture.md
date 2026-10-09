---
id: TASK-17
title: Add a deterministic OT-2 compliance fixture
status: Done
assignee:
  - '@me'
created_date: '2026-10-09 03:29'
updated_date: '2026-10-09 03:41'
labels:
  - compliance
  - backlog-implementer
dependencies: []
references:
  - >-
    ../a2a-lab-dev-kit-rs/backlog/tasks/task-24 -
    Define-the-A2A-LAB-compliance-contract.md
  - tests/support/mod.rs
  - tests/a2a.rs
priority: high
type: feature
ordinal: 17000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Provide an a2a-lab-ot2 fixture mode that satisfies the devkit compliance adaptor contract from a2a-lab-dev-kit TASK-24 while exercising the real OT-2 provider and A2A/MCP adapters. The fixture replaces robot-server and camera hardware with deterministic in-process substitutes and bypasses model-provider, OIDC, SiLA, Docker, and physical-device startup. Existing tests/support/mod.rs and injectable camera tests are authoritative starting points. External prerequisite: a2a-lab-dev-kit TASK-24.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [x] #1 A fixture command starts the OT-2 implementation on configurable or dynamically allocated A2A and MCP ports and provides a machine-readable readiness signal and clean shutdown behavior
- [x] #2 The fixture uses the real OpentronsLab, A2aLabService, A2aServer, and McpServer paths with deterministic robot-server and image inputs; it does not require Docker, a physical OT-2, a camera device, model credentials, OIDC, or SiLA
- [x] #3 The fixture declaration names stable log, metric, task, run, image-source, image, pagination, time-boundary, invalid-input, and missing-id cases required by the A2A-LAB profile
- [x] #4 Task lifecycle and image cases are repeatable across runs and do not mutate external robot-server, filesystem, camera, or cloud state outside fixture-owned temporary data
- [x] #5 Focused tests prove readiness, both protocol endpoints, deterministic fixture reset or isolation, dependency-free startup, and actionable startup failure messages
<!-- AC:END -->

## Implementation Plan

<!-- SECTION:PLAN:BEGIN -->
1. Add a reusable fixture server that seeds an in-process robot-server and deterministic image catalog while constructing the real OpentronsLab and A2A-LAB protocol servers.
2. Add a fixture CLI command with configurable or ephemeral A2A/MCP ports, JSON readiness output, signal shutdown, and contextual startup errors.
3. Add focused tests for declaration validity, endpoint readiness, isolation, dependency-free startup, shutdown, and bind failures.
4. Run focused checks and mise run quality; verify each acceptance criterion and complete task metadata.
<!-- SECTION:PLAN:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
- Added a reusable FixtureServer and fixture CLI command with configurable or ephemeral A2A/MCP ports, validated JSON readiness, and explicit shutdown.
- Seeded an in-process OT-2 robot-server substitute and deterministic two-source image catalog while using OpentronsLab, A2aLabService, A2aServer, and McpServer.
- Declared stable log, metric, task/run, image source/image, alternate pagination, broad time-boundary, invalid-input profile, and missing identifiers for compliance profile 1.0.0.
- Added four focused tests covering both endpoints, stable restart isolation, port release, dependency-free CLI startup, and actionable occupied-port errors.
- Verification passed: mise exec -- cargo nextest run --test compliance_fixture (4/4), mise run clippy, IDE diagnostics, and mise run quality (72/72 tests).
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
Added a deterministic, hardware-independent OT-2 compliance fixture exposed through `a2a-lab-ot2 fixture`. It serves the real OT-2 provider and A2A/MCP adapter stack on configurable or ephemeral ports, emits a validated machine-readable readiness declaration, and shuts down fixture-owned listeners cleanly.

Key files: src/fixture.rs, src/bin/a2a-lab-ot2.rs, src/lib.rs, Cargo.toml, and tests/compliance_fixture.rs. Focused fixture tests and the full `mise run quality` gate pass (72/72 tests). TASK-18 can consume the readiness JSON and devkit profile; no live robot, Docker, camera, model, OIDC, SiLA, filesystem, or cloud dependency is required.
<!-- SECTION:FINAL_SUMMARY:END -->
