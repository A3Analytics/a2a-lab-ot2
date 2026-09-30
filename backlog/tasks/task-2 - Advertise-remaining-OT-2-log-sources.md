---
id: TASK-2
title: Advertise remaining OT-2 log sources
status: Done
assignee:
  - '@me'
created_date: '2026-09-30 21:49'
updated_date: '2026-09-30 21:50'
labels:
  - ot2
dependencies: []
priority: medium
ordinal: 2000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Operators need every robot-server event stream (journals plus command and analysis streams) through list_sources and query_logs, including identifiers that are empty on the simulator.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [x] #1 list_sources includes run_commands, run_command_errors, protocol_analyses, stateless_commands, and every LogIdentifier journal from v10.0.0
- [x] #2 query_logs on an empty simulator journal returns an empty page rather than an error
- [x] #3 query_logs run_command_errors returns only failed commands
<!-- AC:END -->

## Implementation Plan

<!-- SECTION:PLAN:BEGIN -->
1. Classify or implement the slice in inventory.rs and provider.rs.
2. Cover with mock tests.
3. Run mise run quality.
<!-- SECTION:PLAN:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
list_sources now includes journals (LogIdentifier) plus run_commands, run_command_errors, protocol_analyses, and stateless_commands. Empty journals return an empty page.
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
list_sources now includes journals (LogIdentifier) plus run_commands, run_command_errors, protocol_analyses, and stateless_commands. Empty journals return an empty page.

mise run quality passed on the example crate.
<!-- SECTION:FINAL_SUMMARY:END -->
