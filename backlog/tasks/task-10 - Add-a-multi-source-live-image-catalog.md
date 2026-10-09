---
id: TASK-10
title: Add a multi-source live image catalog
status: Done
assignee:
  - '@me'
created_date: '2026-10-08 22:23'
updated_date: '2026-10-08 22:44'
labels: []
dependencies: []
references:
  - src/lib.rs
  - src/page.rs
  - /Users/dylangustaveson/code/a2a-lab-dev-kit-rs/src/images/provider.rs
  - /Users/dylangustaveson/code/a2a-lab-dev-kit-rs/src/images/model.rs
priority: high
type: feature
ordinal: 10000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
The OT-2 agent needs one provider-level image catalog that can expose multiple independently captured live cameras through the devkit ImageProvider contract. Add a process-local, bounded catalog over injectable capture backends so source discovery, current capture, metadata history, search, and retrieval have consistent semantics without coupling the catalog to Opentrons HTTP or Linux V4L2.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [x] #1 The provider lists every configured camera source in stable ID order with valid pagination, and unknown source IDs return not_found
- [x] #2 get_current_image invokes only the requested source, returns a newly captured validated Image, and makes that frame available to get_image by its returned ID
- [x] #3 list_images and search_images return metadata-only descriptors in deterministic captured-at-and-ID order, honor source, half-open time-range, nonblank text, and pagination filters, and never include pixel bytes
- [x] #4 A documented configurable retention bound keeps recent process-local captures per source; deterministic eviction removes old descriptors and causes their image IDs to return not_found without affecting other sources
- [x] #5 Capture failures are isolated to the requested source and surface as stable unavailable errors; concurrent current-image requests do not corrupt IDs, descriptors, or retained bytes
- [x] #6 Focused tests cover two injectable sources, repeated and concurrent captures, pagination, all search filters and timestamp boundaries, retention eviction, unknown IDs, and one source failing while the other remains usable
<!-- AC:END -->

## Implementation Plan

<!-- SECTION:PLAN:BEGIN -->
1. Add a process-local LiveImageCatalog over an injectable ImageCapture trait, with per-source retention and the devkit ImageProvider operations, so discovery and history stay independent of Opentrons HTTP and V4L2.
2. Validate each new frame before storing it, assign a unique image ID, and evict only the oldest frames of the captured source when the retention bound is exceeded.
3. Cover two injectable sources, repeated and concurrent captures, pagination, search filters and timestamp boundaries, retention eviction, unknown IDs, and one failing source with focused tests.
<!-- SECTION:PLAN:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
- Added LiveImageCatalog over an injectable ImageCapture trait. Current capture calls only the requested source, validates the frame, assigns img-{monotonic} IDs, and retains history in process.
- ImageCatalogConfig documents a per-source retention bound (default 32) and decoded-byte limit. Eviction drops the oldest other frames of that source by captured_at then image ID. The frame just stored stays readable. Other sources are untouched.
- List and search return ImageDescriptor pages only, ordered by captured_at then ID, with source, half-open range, nonblank caption text, and cursor pagination.
- Capture errors are returned unchanged and store nothing. tests/image_catalog.rs covers two sources, repeats, concurrency, pagination, search boundaries, retention, unknown IDs, and one failing source.
- Adapted A2aServer::new to Arc<dyn A2aLabApi> and extended the existing A2A skill and MCP tool expectations with the five image operations now advertised by the devkit service. mise run quality passed (fmt, check, clippy, 43 tests).
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
Added a process-local multi-source image catalog that captures through injectable backends and serves the devkit ImageProvider contract.

Key files: src/images/catalog.rs, tests/image_catalog.rs. Verification: mise run quality (43 tests passed). The frame just captured is always retained; older frames of that source are evicted by captured_at then ID.
<!-- SECTION:FINAL_SUMMARY:END -->
