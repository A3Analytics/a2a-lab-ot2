---
id: TASK-3
title: Advertise remaining OT-2 numeric gauges
status: Done
assignee:
  - '@me'
created_date: '2026-09-30 21:49'
updated_date: '2026-09-30 21:50'
labels:
  - ot2
dependencies: []
priority: medium
ordinal: 3000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Operators need live numeric gauges from robot-server GETs (door, lights, instruments, modules, disk) alongside the existing health and run-progress metrics.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [x] #1 list_metrics includes healthy, disk_available_mb, run_progress_percent, run_command_count, door_open, lights_on, pipette_count, instrument_count, and module_count
- [x] #2 query_metric door_open returns 0 or 1 from GET /robot/door/status
<!-- AC:END -->

## Implementation Plan

<!-- SECTION:PLAN:BEGIN -->
1. Classify or implement the slice in inventory.rs and provider.rs.
2. Cover with mock tests.
3. Run mise run quality.
<!-- SECTION:PLAN:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
list_metrics now includes door_open, lights_on, pipette/instrument/module counts, and disk_available_mb alongside health and run progress.
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
list_metrics now includes door_open, lights_on, pipette/instrument/module counts, and disk_available_mb alongside health and run progress.

mise run quality passed on the example crate.
<!-- SECTION:FINAL_SUMMARY:END -->
