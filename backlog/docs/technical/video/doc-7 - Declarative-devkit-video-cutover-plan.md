---
id: doc-7
title: Declarative devkit video cutover plan
type: specification
audience: technical
created_date: "2026-11-12 22:07"
---

# Declarative devkit video cutover plan

## Cutover target

OT-2 will configure video through the devkit's `DeclarativeVideoRuntime`.
Resolved `VideoConfig` supplies source declarations, `camera_id`, and the
`"auto"` format default. Camera, encoder, live store, DVR media, and the
semantic database are devkit modules. OT-2 supplies lab configuration: robot
URL, cameras, schedules, and provider selection.

## Separate SQLite files

The devkit `SqliteChunkDvrStore` owns media tracks, segments, recoverable
minute chunks, immutable H.264 epochs, and retention in `{dvr}/index.sqlite`.
`SemanticSqliteStore` owns manifests, indexing jobs, embeddings, and decision
tables in the file named by `semantic_indexing.limits.storage.sqlite_path`
(default `semantic.sqlite` beside the configuration file). Opening the DVR
does not create semantic tables. `DvrPersistHook` runs after the media
transaction commits and writes the semantic database on its own connection.

## Configuration and compatibility

`lab_config` maps integer or text `camera_id` values and optional
`format: "auto"` directly to devkit `VideoConfig`. Legacy
`--external-camera-index` and `--camera-index` flags are translated into the
same declarative source entries, so `serve` has one runtime path.

## Runtime integration and deletion

OT-2 starts `DeclarativeVideoRuntime`, passes its provider to the A2A service,
attaches `.with_capture_settings(runtime.settings_handle())` to
`A2aServer`, and serves stills from `runtime.still_cache()`. After tests
migrate to `VideoRuntimeInjections`, superseded capture, encoding, cadence,
epoch, and provider modules are deleted. The final gate is `mise run quality`
in both repositories.

## Backlog tasks

- OT2-1: TASK-85
- OT2-2: TASK-86
- OT2-3: TASK-87
- OT2-4: TASK-88
