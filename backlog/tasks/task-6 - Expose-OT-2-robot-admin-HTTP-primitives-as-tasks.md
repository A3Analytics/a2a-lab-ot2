---
id: TASK-6
title: Expose OT-2 robot admin HTTP primitives as tasks
status: Done
assignee:
  - '@me'
created_date: '2026-09-30 21:49'
updated_date: '2026-09-30 21:50'
labels:
  - ot2
dependencies: []
priority: medium
ordinal: 6000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Agents should reach networking, settings, camera, data files, client data, labware offsets, error recovery, system time, and door through start_task.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [x] #1 list_tasks includes get_networking_status, get_settings, get_camera, get_data_files, get_client_data, get_labware_offsets, get_error_recovery_settings, get_system_time, and get_door_status
- [x] #2 start_task get_door_status and get_settings complete with JSON text in TaskRun.message
- [x] #3 Auth-scoped routes such as put_system_time remain advertised even if robot-server returns 403
<!-- AC:END -->

## Implementation Plan

<!-- SECTION:PLAN:BEGIN -->
1. Classify or implement the slice in inventory.rs and provider.rs.
2. Cover with mock tests.
3. Run mise run quality.
<!-- SECTION:PLAN:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
Networking, settings, camera, data files, client data, offsets, error recovery, system time, and door are inventory tasks. Auth failures stay SdkError.
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
Networking, settings, camera, data files, client data, offsets, error recovery, system time, and door are inventory tasks. Auth failures stay SdkError.

mise run quality passed on the example crate.
<!-- SECTION:FINAL_SUMMARY:END -->
