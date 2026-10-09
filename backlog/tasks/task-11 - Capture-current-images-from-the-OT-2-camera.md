---
id: TASK-11
title: Capture current images from the OT-2 camera
status: Done
assignee:
  - '@me'
created_date: '2026-10-08 22:23'
updated_date: '2026-10-08 22:53'
labels: []
dependencies:
  - TASK-10
references:
  - src/opentrons/client.rs
  - src/opentrons/provider.rs
  - src/opentrons/inventory.rs
  - backlog/docs/technical/ot2-http/doc-1 - OT-2-robot-server-HTTP-mapping.md
  - >-
    https://github.com/Opentrons/opentrons/blob/v10.0.0/robot-server/robot_server/service/legacy/routers/camera.py
  - >-
    https://github.com/Opentrons/opentrons/blob/v10.0.0/api/src/opentrons/system/camera.py
priority: high
type: feature
ordinal: 11000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
The Opentrons camera must become a live image source through the pinned v10.0.0 robot-server contract. Current-image capture must use the OT-2 still-picture response, preserve the distinction between camera status, preview, and stream routes, and fail safely when the on-board camera is disabled or unavailable.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [x] #1 The provider advertises one stable Opentrons camera source associated with asset opentrons-ot2 and obtains its current frame from the pinned v10.0.0 POST /camera/picture binary response with the required Opentrons-Version header
- [x] #2 The implementation does not use GET /camera/stream as a frame endpoint, does not enable or reconfigure the camera, and does not require the Flex-only live-stream service
- [x] #3 A successful JPEG response produces byte-identical image data with image/jpeg metadata, positive dimensions read from the image, a UTC capture timestamp, and a unique image ID that participates in the shared catalog
- [x] #4 Disabled-camera, missing-camera, robot-server transport, timeout, non-success HTTP, non-image content type, empty body, malformed JPEG, and configured payload-limit failures return stable actionable errors without crashing or retaining a partial frame
- [x] #5 Mock robot-server tests verify the exact method, path, version header, binary response handling, dimensions, and representative 422, 500, malformed-response, and unreachable-server failures; no physical robot is required for the quality gate
<!-- AC:END -->

## Implementation Plan

<!-- SECTION:PLAN:BEGIN -->
1. Add POST /camera/picture to the robot-server client, requiring the Opentrons-Version header and returning the raw body plus content type.
2. Parse JPEG dimensions and publish one opentrons-camera source (asset opentrons-ot2) through ImageCapture and the shared catalog. Do not call /camera/stream or change camera settings.
3. Cover a byte-identical JPEG, dimensions, disabled and missing camera, transport, timeout, non-image, empty, malformed, and payload-limit failures with a mock robot-server.
<!-- SECTION:PLAN:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
- OpentronsCamera captures POST /camera/picture with the Opentrons-Version header and registers source opentrons-camera with asset opentrons-ot2. JPEG bytes are stored unchanged as image/jpeg after reading SOF dimensions.
- The capture path does not call GET /camera/stream, POST /camera, or the live-stream service. Disabled (422), missing device (500), transport, timeout, non-image, empty, malformed, and oversize responses fail without retaining a frame.
- tests/opentrons_camera.rs drives a mock robot-server. mise run quality passed (49 tests).
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
The OT-2 on-board camera is a live catalog source. Current frames come from robot-server POST /camera/picture, including the Opentrons-Version header, and are stored as byte-identical JPEGs.

Key files: src/opentrons/camera.rs, src/opentrons/client.rs, src/images/jpeg.rs, tests/opentrons_camera.rs. Verification: mise run quality (49 tests passed).
<!-- SECTION:FINAL_SUMMARY:END -->
