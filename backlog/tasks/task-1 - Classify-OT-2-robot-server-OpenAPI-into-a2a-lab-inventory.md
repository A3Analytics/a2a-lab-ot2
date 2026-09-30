---
id: TASK-1
title: Classify OT-2 robot-server OpenAPI into a2a-lab inventory
status: Done
assignee:
  - '@me'
created_date: '2026-09-30 21:49'
updated_date: '2026-09-30 21:50'
labels:
  - ot2
dependencies: []
priority: high
ordinal: 1000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Callers need a single Rust table of every v10.0.0 OT-2 simulator HTTP operation so logs, metrics, and tasks stay aligned with robot-server instead of a hand-maintained subset.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [x] #1 src/opentrons/inventory.rs lists every GET /openapi.json path+method from the pinned OT-2 v10.0.0 simulator
- [x] #2 Each operation is Kind::Log, Kind::Metric, Kind::Task, or Kind::Skip (Flex-only / OT-2-unsupported)
- [x] #3 A unit test fails if an OpenAPI operation is unclassified
<!-- AC:END -->

## Implementation Plan

<!-- SECTION:PLAN:BEGIN -->
1. Classify or implement the slice in inventory.rs and provider.rs.
2. Cover with mock tests.
3. Run mise run quality.
<!-- SECTION:PLAN:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
Encoded GET /openapi.json from the v10.0.0 simulator into src/opentrons/inventory.rs with Kind::Log|Metric|Task|Skip. tests/inventory.rs asserts every OpenAPI pair is classified.
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
Encoded GET /openapi.json from the v10.0.0 simulator into src/opentrons/inventory.rs with Kind::Log|Metric|Task|Skip. tests/inventory.rs asserts every OpenAPI pair is classified.

mise run quality passed on the example crate.
<!-- SECTION:FINAL_SUMMARY:END -->
