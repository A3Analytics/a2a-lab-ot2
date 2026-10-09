---
id: TASK-13
title: Configure and serve both OT-2 camera sources
status: Done
assignee:
  - '@me'
created_date: '2026-10-08 22:23'
updated_date: '2026-10-08 23:06'
labels: []
dependencies:
  - TASK-11
  - TASK-12
references:
  - src/bin/a2a-lab-ot2.rs
  - src/agent.rs
  - tests/a2a.rs
  - >-
    backlog/docs/technical/cli/doc-3 -
    a2a-lab-ot2-command-and-configuration-reference.md
  - /Users/dylangustaveson/code/a2a-lab-dev-kit-rs/src/service.rs
  - /Users/dylangustaveson/code/a2a-lab-dev-kit-rs/src/mcp/client.rs
priority: high
type: feature
ordinal: 13000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Serving a2a-lab-ot2 must attach both camera backends to A2aLabService so the devkit's five image operations are discoverable over MCP and A2A. Operators need explicit limits and device configuration, while an absent camera must not prevent the robot, logs, metrics, tasks, or the other camera from starting.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [x] #1 Serve mode registers both the Opentrons camera and external-camera sources with A2aLabService::with_images, and the A2A card and MCP tool list include all five devkit image operations in addition to the existing operations
- [x] #2 The external camera device is configurable by --v4l2-device and A2ALAB_V4L2_DEVICE with /dev/video4 as the default; help and startup output show the effective source configuration without opening the device
- [x] #3 The decoded image maximum is configurable by --max-image-bytes and A2ALAB_MAX_IMAGE_BYTES with the devkit 64 MiB default, rejects invalid values, and is applied consistently to A2aLabService and the internal McpLab connection used by A2A
- [x] #4 A configurable positive capture timeout and bounded per-source retention setting are applied to both sources and reject zero or overflow-prone values before listeners start
- [x] #5 Startup remains successful when either camera is missing, disabled, busy, or unsupported; list_image_sources still identifies both configured sources and only operations against the failing source return an actionable error
- [x] #6 Image reads remain available under --readonly because they are image operations rather than task inventory mutations, and existing task readonly behavior is unchanged
- [x] #7 Configuration and serve integration tests cover defaults, command-line-over-environment precedence, invalid limits, both-source discovery, one-source failure isolation, image payload enforcement, exact A2A skills, and exact MCP tools
<!-- AC:END -->

## Implementation Plan

<!-- SECTION:PLAN:BEGIN -->
1. Validate device, decoded-byte maximum, capture timeout, and per-source retention before listeners start, with CLI overriding the environment.
2. Register both camera sources on A2aLabService::with_images and apply the same image transport to that service and the A2A McpLab connection.
3. Print the effective source configuration without opening the V4L2 device, and test defaults, invalid limits, both-source discovery, failure isolation, payload limits, readonly, and exact A2A skills and MCP tools.
<!-- SECTION:PLAN:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
- Serve validates the V4L2 device, 64 MiB decoded-byte default, capture timeout, and per-source retention before listeners start. --v4l2-device and A2ALAB_V4L2_DEVICE default to /dev/video4. CLI overrides the environment. Zero and overflow-prone limits fail without contacting robot-server.
- Both opentrons-camera and external-camera are registered with A2aLabService::with_images. The same ImageTransportConfig is applied to the service and to McpLab::connect_with for A2A. Startup prints the effective sources without opening the device.
- A missing, disabled, busy, or unsupported camera does not remove the other source. --readonly still serves image reads and keeps write tasks hidden.
- tests/image_serve.rs covers help, precedence, invalid limits, both-source discovery, failure isolation, payload limits, readonly, exact A2A skills, and exact MCP tools. mise run quality passed (62 tests).
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
Serve mode exposes both the OT-2 camera and the host V4L2 camera through the devkit image operations, with explicit device, timeout, retention, and payload limits.

Key files: src/images/serve.rs, src/bin/a2a-lab-ot2.rs, tests/image_serve.rs. Verification: mise run quality (62 tests passed). Image limits are rejected before listeners start, and startup prints the camera configuration without opening the V4L2 device.
<!-- SECTION:FINAL_SUMMARY:END -->
