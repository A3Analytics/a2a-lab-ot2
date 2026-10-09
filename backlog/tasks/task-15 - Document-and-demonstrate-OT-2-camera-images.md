---
id: TASK-15
title: Document and demonstrate OT-2 camera images
status: Done
assignee:
  - '@me'
created_date: '2026-10-08 22:23'
updated_date: '2026-10-08 23:19'
labels: []
dependencies:
  - TASK-14
references:
  - README.md
  - backlog/docs/technical/ot2-http/doc-1 - OT-2-robot-server-HTTP-mapping.md
  - backlog/docs/technical/testing/doc-2 - Testing-a2a-lab-ot2.md
  - >-
    backlog/docs/technical/cli/doc-3 -
    a2a-lab-ot2-command-and-configuration-reference.md
  - .mise/run.toml
  - >-
    /Users/dylangustaveson/code/a2a-lab-dev-kit-rs/backlog/docs/reference/primitives/doc-17
    - A2A-LAB-primitives.md
priority: medium
type: docs
ordinal: 15000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Operators need an accurate guide and an executable way to discover and retrieve images from the two camera sources. Documentation must distinguish robot-server access to the OT-2 on-board camera from direct host V4L2 access, state simulator and hardware limitations, and describe the devkit's inline transport and process-local history semantics.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [x] #1 README and the existing Backlog-managed HTTP mapping, CLI/configuration, and testing docs list the five image operations and identify the Opentrons camera and external-camera source IDs without creating a loose documentation file
- [x] #2 The docs state that the OT-2 on-board camera is captured through pinned v10.0.0 POST /camera/picture, that GET /camera/stream is not used and OT-2 live streaming is unsupported, and that camera enablement is operator-managed rather than changed by this agent
- [x] #3 The docs state that the additional camera is captured directly from the host Linux V4L2 device, defaults to /dev/video4, is independently configurable, and never passes through robot-server
- [x] #4 Configuration documentation covers device path, timeout, retention, the 64 MiB decoded-byte default and override, readonly behavior, startup without cameras, inline base64 cost, process-local frame retention, and actionable missing, disabled, busy, permission, timeout, malformed-frame, and oversized-frame errors
- [x] #5 A mise-backed executable demo lists both sources and can request a current image from either selected source, reports descriptor metadata, and saves byte-identical image data to an operator-selected path without embedding binary data in terminal output
- [x] #6 The demo and docs explicitly state that the pinned simulator can advertise the Opentrons source but does not emulate a usable on-board camera, and provide separate expected outcomes for deterministic tests, the simulator, a real OT-2, and a Linux host with /dev/video4
- [x] #7 mise run quality passes with documentation checks, the demo's hardware-independent tests, and the completed image parity suite
<!-- AC:END -->

## Implementation Plan

<!-- SECTION:PLAN:BEGIN -->
1. Add list-image-sources and get-current-image commands plus a mise images demo that prints metadata and writes the JPEG to a chosen path.
2. Update the README and the existing HTTP, CLI, and testing docs with both source ids, the five image operations, capture limits, and the distinct test, simulator, OT-2, and V4L2 outcomes.
3. Cover the demo with a mock robot-server test that checks metadata text and byte-identical output, then run mise run quality.
<!-- SECTION:PLAN:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
- README, doc-1, doc-2, and doc-3 name list_image_sources, list_images, search_images, get_image, and get_current_image, plus opentrons-camera and external-camera.
- Docs state that the on-board camera uses pinned v10.0.0 POST /camera/picture, that GET /camera/stream is not a frame source, and that enablement stays with the operator. The host camera reads /dev/video4 directly and is configurable.
- mise run images lists both sources or saves one JPEG. The command prints descriptor metadata and does not print image bytes. tests/image_demo.rs covers that path with a mock robot-server.
- Docs separate deterministic tests, the pinned simulator, a real OT-2, and a Linux host with /dev/video4. mise run quality passed (67 tests).
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
Operators can discover both cameras and save a current JPEG, and the HTTP, CLI, and testing docs describe the capture path, limits, and environment-specific outcomes.

Key files: README.md, backlog docs doc-1, doc-2, and doc-3, .mise/scripts/images.sh, tests/image_demo.rs. Verification: mise run quality (67 tests passed).
<!-- SECTION:FINAL_SUMMARY:END -->
