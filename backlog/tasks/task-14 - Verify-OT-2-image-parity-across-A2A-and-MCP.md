---
id: TASK-14
title: Verify OT-2 image parity across A2A and MCP
status: Done
assignee:
  - '@me'
created_date: '2026-10-08 22:23'
updated_date: '2026-10-08 23:13'
labels: []
dependencies:
  - TASK-13
references:
  - tests/a2a.rs
  - tests/support/mod.rs
  - /Users/dylangustaveson/code/a2a-lab-dev-kit-rs/tests/image_parity.rs
  - /Users/dylangustaveson/code/a2a-lab-dev-kit-rs/tests/a2a_images.rs
  - /Users/dylangustaveson/code/a2a-lab-dev-kit-rs/tests/mcp_images.rs
priority: high
type: task
ordinal: 14000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
The OT-2 application exposes the same two camera sources through different A2A and MCP wire mechanisms. A bounded conformance matrix must detect application-level divergence in source catalogs, capture history, inline bytes, limits, and device failures while preserving the devkit's intentional protocol framing differences.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [x] #1 One shared fixture matrix drives the configured Opentrons and external-camera sources through both A2A and MCP for source listing, current capture, source-scoped listing, range-only search, text-only search, combined search, and specific-image retrieval
- [x] #2 Equivalent successful calls return the same ordered source and descriptor data and byte-identical inline images, including repeated captures from each source and empty and final pages
- [x] #3 The matrix covers unknown source and image IDs, half-open timestamp boundaries, retention eviction, Opentrons camera disabled and malformed HTTP responses, V4L2 missing, busy, and timed-out devices, and one source failing while the other succeeds
- [x] #4 Payloads immediately below, at, and above the configured decoded-byte maximum produce matching A2A and MCP results or stable SDK error categories
- [x] #5 Validation treats A2A hyphenated skills and artifacts versus MCP snake_case tools and JSON envelopes as intentional differences, confirms both sources use inline bytes rather than URI delivery, and reports the case plus both observed outputs for every unexplained difference
- [x] #6 The parity suite uses deterministic mock HTTP and V4L2 boundaries and runs in mise run quality without Docker, camera hardware, or model credentials
<!-- AC:END -->

## Implementation Plan

<!-- SECTION:PLAN:BEGIN -->
1. Drive opentrons-camera and external-camera through one A2A and MCP fixture for listing, capture, search, and image retrieval.
2. Compare normalized results and report both outputs when they differ. Treat hyphenated skills versus snake_case tools as intentional, and require inline bytes.
3. Cover pages, timestamp boundaries, retention, camera and V4L2 failures, and payloads below, at, and above the decoded-byte maximum with mock boundaries.
<!-- SECTION:PLAN:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
- tests/image_parity.rs runs one A2A and MCP fixture over opentrons-camera and external-camera. List, search, page, and get_image results are compared directly. Current capture is read back through both protocols by image id.
- The matrix covers empty and final pages, half-open timestamp bounds, retention eviction, unknown ids, payloads below, at, and above the decoded maximum, disabled and malformed robot-server responses, and V4L2 missing, busy, and timeout with the other source still usable.
- Hyphenated A2A image skills and snake_case MCP tools are the intentional framing difference. Image payloads are inline base64, not URIs. A mismatch prints the case and both outputs. mise run quality passed (65 tests).
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
A2A and MCP now see the same OT-2 and external camera results for listing, search, retrieval, limits, and device failures.

Key file: tests/image_parity.rs. Verification: mise run quality (65 tests passed). Differences are reported with both outputs, and protocol naming is treated as an intentional framing difference.
<!-- SECTION:FINAL_SUMMARY:END -->
