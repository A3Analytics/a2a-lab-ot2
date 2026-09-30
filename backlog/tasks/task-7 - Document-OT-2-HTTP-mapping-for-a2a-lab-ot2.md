---
id: TASK-7
title: Document OT-2 HTTP mapping for a2a-lab-ot2
status: Done
assignee:
  - '@me'
created_date: '2026-09-30 21:49'
updated_date: '2026-09-30 21:50'
labels:
  - ot2
dependencies: []
priority: medium
ordinal: 7000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Humans and agents need a technical description of how robot-server OpenAPI operations map onto the seven a2a-lab operations without adding loose markdown.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [x] #1 A backlog technical doc under technical/ describes the log/metric/task/skip mapping and start-task input conventions
- [x] #2 README states that list-tasks includes primitive HTTP operations and that Flex-only routes are omitted
- [x] #3 mise run quality passes
<!-- AC:END -->

## Implementation Plan

<!-- SECTION:PLAN:BEGIN -->
1. Classify or implement the slice in inventory.rs and provider.rs.
2. Cover with mock tests.
3. Run mise run quality.
<!-- SECTION:PLAN:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
doc-1 under technical/ot2-http describes the mapping. README and AGENTS.md mention primitive HTTP coverage. mise run quality passed.
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
doc-1 under technical/ot2-http describes the mapping. README and AGENTS.md mention primitive HTTP coverage. mise run quality passed.

mise run quality passed on the example crate.
<!-- SECTION:FINAL_SUMMARY:END -->
