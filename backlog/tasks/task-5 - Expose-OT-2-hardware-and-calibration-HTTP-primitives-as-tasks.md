---
id: TASK-5
title: Expose OT-2 hardware and calibration HTTP primitives as tasks
status: Done
assignee:
  - '@me'
created_date: '2026-09-30 21:49'
updated_date: '2026-09-30 21:50'
labels:
  - ot2
dependencies: []
priority: medium
ordinal: 5000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Agents should drive modules, instruments, motors, home/identify, and OT-2 calibration sessions through the same start_task surface as protocol runs.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [x] #1 list_tasks includes get_modules, get_instruments, get_pipettes, post_robot_home, post_identify, get_sessions, and pipette/tip/labware calibration ids
- [x] #2 start_task get_modules completes against the mock robot-server
- [x] #3 Flex estop, deck configuration, and subsystem ids are absent from list_tasks
<!-- AC:END -->

## Implementation Plan

<!-- SECTION:PLAN:BEGIN -->
1. Classify or implement the slice in inventory.rs and provider.rs.
2. Cover with mock tests.
3. Run mise run quality.
<!-- SECTION:PLAN:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
Modules, instruments, motors, home/identify, sessions, and OT-2 calibrations are inventory tasks. Flex estop/deck/subsystems are Kind::Skip.
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
Modules, instruments, motors, home/identify, sessions, and OT-2 calibrations are inventory tasks. Flex estop/deck/subsystems are Kind::Skip.

mise run quality passed on the example crate.
<!-- SECTION:FINAL_SUMMARY:END -->
