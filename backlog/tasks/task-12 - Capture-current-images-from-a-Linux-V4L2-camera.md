---
id: TASK-12
title: Capture current images from a Linux V4L2 camera
status: Done
assignee:
  - '@me'
created_date: '2026-10-08 22:23'
updated_date: '2026-10-08 22:58'
labels: []
dependencies:
  - TASK-10
references:
  - src/lib.rs
  - Cargo.toml
  - /Users/dylangustaveson/code/a2a-lab-dev-kit-rs/src/images/provider.rs
priority: high
type: feature
ordinal: 12000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
A second camera attached to the host at /dev/video4 must be available independently of robot-server. Add a Linux V4L2 current-frame source with configurable device selection and deterministic test seams, while keeping non-Linux development and the normal quality gate hardware-independent.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [x] #1 The provider advertises a stable external-camera source and captures its current frame directly from the configured Linux V4L2 device, whose default is /dev/video4, without routing through any Opentrons HTTP endpoint
- [x] #2 A successful capture returns byte-identical JPEG data with image/jpeg metadata, positive dimensions read from the frame, a UTC capture timestamp, and a unique image ID that participates in the shared catalog
- [x] #3 The crate still builds and its automated suite passes on non-Linux development hosts without opening a video device, while Linux capture behavior is testable through an injected or fixture-backed boundary
- [x] #4 Missing-device, permission-denied, busy-device, unsupported-format, timeout, empty-frame, malformed-frame, and configured payload-limit failures return stable unavailable or invalid errors that name the configured device and do not hang, panic, or retain a partial frame
- [x] #5 Configuration accepts an operator-selected device path while preserving /dev/video4 as the documented default, and tests prove that distinct configured paths reach only their selected capture boundary
- [x] #6 Focused tests cover a valid frame, dimension extraction, the default and overridden path, timeout, representative device errors, malformed bytes, and operation when the Opentrons source is simultaneously unavailable
<!-- AC:END -->

## Implementation Plan

<!-- SECTION:PLAN:BEGIN -->
1. Add an external-camera source whose default device is /dev/video4 and whose grab path is an injectable V4L2 boundary, separate from robot-server.
2. Read one JPEG frame from the selected device, enforce timeout and payload limits, and store it in the shared catalog with image/jpeg metadata and SOF dimensions.
3. Map missing, permission, busy, unsupported, timeout, empty, malformed, and oversize failures to stable errors that name the device and retain nothing. Test paths, dimensions, and an unavailable Opentrons source without opening a video device.
<!-- SECTION:PLAN:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
- ExternalCamera source id is external-camera. The default device is /dev/video4 and is not opened until grab. Operators can select another path. Capture does not call robot-server.
- DeviceGrab reads one JPEG frame from the V4L2 device node. Tests use an injected V4l2Grab boundary, so the suite does not open a video device. Non-JPEG device bytes are an unsupported-format error.
- Missing, permission, busy, unsupported, and timeout failures are unavailable and name the device. Empty, malformed, and oversize frames are invalid and are not retained. A blocked grab returns before the grab finishes.
- tests/v4l2_camera.rs covers both paths, dimensions, defaults, and a successful external capture while the Opentrons camera is unreachable. mise run quality passed (55 tests).
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
The host camera is a second live catalog source. It reads a JPEG directly from a Linux V4L2 device, defaulting to /dev/video4, and stores that frame in the shared catalog.

Key files: src/v4l2.rs, tests/v4l2_camera.rs. Verification: mise run quality (55 tests passed). Automated tests inject the grab boundary and do not open a video device.
<!-- SECTION:FINAL_SUMMARY:END -->
