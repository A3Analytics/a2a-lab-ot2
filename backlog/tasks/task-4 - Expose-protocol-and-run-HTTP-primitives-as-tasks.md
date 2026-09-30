---
id: TASK-4
title: Expose protocol and run HTTP primitives as tasks
status: Done
assignee:
  - '@me'
created_date: '2026-09-30 21:49'
updated_date: '2026-09-30 21:50'
labels:
  - ot2
dependencies: []
priority: high
ordinal: 4000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Agents should list, get, delete, analyze protocols and create/get/patch runs through start_task without dropping the run_serial_dilution composite.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [x] #1 list_tasks includes run_serial_dilution plus get_protocols, get_protocol, delete_protocol, get_runs, post_runs, get_run, and patch_run
- [x] #2 start_task get_protocols completes with JSON from GET /protocols
- [x] #3 start_task run_serial_dilution still uploads the bundled protocol, creates a run, and plays it
<!-- AC:END -->

## Implementation Plan

<!-- SECTION:PLAN:BEGIN -->
1. Classify or implement the slice in inventory.rs and provider.rs.
2. Cover with mock tests.
3. Run mise run quality.
<!-- SECTION:PLAN:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
Primitive protocol/run HTTP tasks dispatch through the generic client. run_serial_dilution remains a composite.
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
Primitive protocol/run HTTP tasks dispatch through the generic client. run_serial_dilution remains a composite.

mise run quality passed on the example crate.
<!-- SECTION:FINAL_SUMMARY:END -->
