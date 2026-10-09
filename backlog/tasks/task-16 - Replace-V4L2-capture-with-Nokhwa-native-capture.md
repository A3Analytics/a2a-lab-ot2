---
id: TASK-16
title: Replace V4L2 capture with Nokhwa native capture
status: Done
assignee:
  - '@me'
created_date: '2026-10-09 02:58'
updated_date: '2026-10-09 03:07'
labels: []
dependencies: []
references:
  - Cargo.toml
  - src/v4l2.rs
  - src/images/serve.rs
  - src/bin/a2a-lab-ot2.rs
  - tests/v4l2_camera.rs
  - tests/image_serve.rs
  - tests/image_parity.rs
  - README.md
priority: high
type: enhancement
ordinal: 16000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
The external-camera source currently depends on a Linux-specific libc ioctl/mmap/poll implementation and V4L2-shaped configuration. Replace that boundary with Nokhwa's automatic native backend so the same generic camera-index configuration and capture contract work without application-level operating-system checks on Linux, macOS, and Windows. Preserve the external-camera protocol behavior, configurable description, injectable test seams, and hardware-independent quality gate. README camera-index discovery examples are part of the observable completion outcome and may be supplied by the documentation specialist after the code is implemented.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [x] #1 Cargo configuration uses Nokhwa with the input-native feature, and the application contains no custom libc ioctl/mmap/poll camera capture, no operating-system checks or cfg-gated external-camera behavior/configuration, and no runtime dependency on ffmpeg, GStreamer, Python, or another host-installed video executable
- [x] #2 One optional numeric external camera index enables external-camera through identically available command-line, environment, library configuration, help, validation, precedence, and startup-summary behavior on every platform; V4L2-specific device/path names are absent from the external-camera configuration surface, while the advertised external-camera description remains configurable through generically named settings with a platform-neutral default
- [x] #3 For each current-image request, the native backend requires exact MJPEG 3840x2160 at 30 fps, discards frames 1 through 29, and returns the untouched encoded bytes of frame 30; an unavailable exact format or capture failure produces an actionable stable error and retains no partial image
- [x] #4 The external camera keeps an injectable capture boundary, and deterministic tests verify the requested index and exact format, 30-frame settling sequence, byte-identical frame-30 result, generic description, timeout and representative backend/format failures, payload validation, and isolation when the Opentrons camera is unavailable without opening real camera hardware
- [x] #5 Existing A2A and MCP image parity coverage remains active for external-camera discovery, current capture, retrieval, inline byte identity, limits, and failures, with test fixtures renamed away from V4L2 concepts and no change to the external-camera source ID or protocol contract
- [x] #6 Before task closure, README examples show how to find and pass a camera index on Linux, macOS, and Windows, document the generic external-camera index and description settings, and state the exact MJPEG 3840x2160 at 30 fps and frame-30 behavior without presenting any platform as having a different application feature set
- [x] #7 mise run quality passes without camera hardware or host video executables, and mise run build-linux-x86_64 confirms the native-backend application build
<!-- AC:END -->

## Implementation Plan

<!-- SECTION:PLAN:BEGIN -->
1. Replace the custom V4L2 implementation and dependency with Nokhwa input-native capture behind the existing injectable boundary.
2. Migrate library and CLI configuration to a generic optional camera index and description while preserving source/protocol behavior.
3. Rename and extend deterministic fixtures to verify exact MJPEG format, 30-frame settling, errors, validation, and parity without hardware.
4. Run focused checks, quality, and the Linux x86-64 build; check verified non-README criteria and leave TASK-16 In Progress for documentation.
<!-- SECTION:PLAN:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
- Replaced the custom V4L2 module with src/camera.rs using nokhwa 0.10.11 input-native and raw MJPEG frame capture.
- Migrated external camera configuration to optional numeric --external-camera-index / A2ALAB_EXTERNAL_CAMERA_INDEX and generic description names; source ID remains external-camera.
- Added deterministic external-camera tests and migrated serve/parity/demo fixtures away from V4L2 device concepts. Focused camera tests passed (19 tests).
- mise run quality passed: 68 tests, 0 failures. IDE diagnostics report no errors.
- Initial Linux cross-build exposed missing libclang for Nokhwa v4l bindings; added clang and libclang-dev to the existing Docker build task. mise run build-linux-x86_64 then passed.
- No live camera was opened; exact hardware negotiation and capture remain hardware-dependent runtime behavior.
- README was intentionally not edited for TASK-16. Acceptance criterion #6 remains unchecked for the documentation writer/finalizer.

- Verified README against the implementation: generic index/description CLI and environment names, platform-neutral default, Nokhwa native backends, exact MJPEG 3840x2160 at 30 fps, frame-30 behavior, and Linux/macOS/Windows discovery examples are accurate.
- Final quality run initially hit a transient unrelated SQLite database-lock failure in tests/agent.rs. The isolated test passed on retry, then mise run quality passed all 68 tests.
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
Replaced the Linux-specific custom V4L2 camera boundary with Nokhwa input-native capture and a platform-neutral numeric camera index configuration. External capture now requires exact MJPEG 3840x2160 at 30 fps, returns untouched frame 30, preserves the external-camera protocol contract, and remains fully injectable for hardware-independent tests.

Migrated CLI, environment, library configuration, startup output, fixtures, parity coverage, build dependencies, and README guidance. Verified focused camera tests, mise run quality (68/68), and mise run build-linux-x86_64; live-hardware behavior remains dependent on a camera supporting the required exact format.
<!-- SECTION:FINAL_SUMMARY:END -->
