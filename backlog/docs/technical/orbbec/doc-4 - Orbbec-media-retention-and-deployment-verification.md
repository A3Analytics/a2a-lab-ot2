---
id: doc-4
title: Orbbec media retention and deployment verification
type: specification
audience: technical
created_date: "2026-11-03 19:17"
---

# Orbbec media retention and deployment verification

## Recovery status

This page is the TASK-52 document recovered from Cursor local history on 2026-11-04. The shared-session and sync-retention source it names is not in commit `b5d29c673c2716c3b985de94e19e8ad4f06f9650` or in the `orbbec-frame-sets` worktree `a0f032a775f3c25a24c375c9a249b757829da549`. The behavior below is the preserved contract to restore. It is not a description of the tree at those commits. The recovered closing task list mis-labeled TASK-49 through TASK-51; the list at the bottom uses the backlog titles. See the [dual-format plan baseline](<../video/doc-5 - Dual-format-video-capture-implementation-plan.md>).

## Purpose

Operators and integrators need one place to read how Orbbec stills, live MJPEG, synchronized frame sets, and long-term sync history interact after the shared-session and retention work in TASK-48 through TASK-51. CLI flags and defaults stay in [doc-3](<../cli/doc-3 - a2a-lab-ot2-command-and-configuration-reference.md>). Automated coverage is summarized in [doc-2](<../testing/doc-2 - Testing-a2a-lab-ot2.md>).

## One shared Orbbec session

With `--orbbec-camera` (or `--camera-backend orbbec`), `serve` starts exactly one Orbbec SDK pipeline per process. That pump feeds:

- `orbbec-camera` still capture and `get-current-image`
- `orbbec-camera` live MJPEG follow and in-memory live playback
- frame-set discovery and one-shot capture tasks
- continuous synchronized history when retention is attached

Stills, live color, discovery, and frame sets therefore share one device open and one frame sequence. Hardware-free proof: `tests/orbbec_session.rs` (`shared_consumers_use_one_pipeline`).

### Capture rate and planes

Synchronized capture targets **5 FPS** (`--video-fps` default `5`, SDK `TARGET_FPS`, sync frame period 200 ms). A complete stereo set is color MJPEG plus depth and left/right infrared, with optional confidence when the device exposes it. Timestamps must align within 100 ms across color, depth, and both infrared planes before a set is stored.

When depth or infrared is missing, color MJPEG can still flow (RGB-only fallback). The session records that interval as a **stereo gap** (`tests/orbbec_session.rs`, `rgb_fallback_records_stereo_gap`). Continuous history stores only **complete** plane bundles for the current mode: color-only rows when stereo is unavailable, full stereo (plus confidence when present) when stereo is available.

## Retention layers

| Layer               | What is kept                                  | Default / limit                                                                                                         | Where                                                 | Notes                                                                                                                                     |
| ------------------- | --------------------------------------------- | ----------------------------------------------------------------------------------------------------------------------- | ----------------------------------------------------- | ----------------------------------------------------------------------------------------------------------------------------------------- |
| Live MJPEG memory   | Recent frames per enabled native video source | **5 s** (`--video-retention-secs`)                                                                                      | In-process ring                                       | Distinct from disk retention.                                                                                                             |
| External-camera DVR | Indexed MJPEG on disk                         | **1 day** on the deployed host (`A2ALAB_VIDEO_DISK_RETENTION_SECS` `86400`). Code default remains **3 days** (`259200`) | `--video-dvr-root` (default `.a2a-lab-ot2/video-dvr`) | Minute chunks plus SQLite index. Native 4K frames are about 400 KB, so three days does not fit the host disk.                             |
| Orbbec sync history | Every complete synchronized set               | **256 GiB** payload quota **or** host free space below **64 GiB** reserve                                               | `--orbbec-sync-db` and `--orbbec-sync-chunk-root`     | One physical copy per channel; color video **reads these slices** instead of writing a second MJPEG tree under `video-dvr/orbbec-camera`. |
| Frame-set catalog   | Recent captured set metadata for tasks/UI     | `--frame-set-retention` (default `32`)                                                                                  | `--frame-set-db`                                      | Separate from continuous sync bytes; retained sync ids use the `sync-` prefix.                                                            |
| Still images        | Operator-requested JPEG history               | **30 days** age plus `--image-retention` count                                                                          | `.a2a-lab-ot2/images.sqlite`                          | **Not** written by continuous Orbbec recording; only explicit still capture paths populate this file.                                     |

Eviction for Orbbec sync drops the **oldest complete minute** across **all channels in that minute** (color, depth, infrared, confidence together). Quota uses summed `payload_length` in `orbbec_sync_channels`; the reserve uses `df` on the chunk root volume (`src/orbbec_sync/disk.rs`).

## Operator-facing recovery

- **Process restart:** Re-open `serve` with the same `--orbbec-sync-db`, `--orbbec-sync-chunk-root`, and `--video-dvr-root`. `OrbbecSyncStore::open` removes channel rows whose payloads never finished (`payload_ready = 0`) and finalizes minute chunks. External DVR uses the same partial-chunk recovery as before (`tests/video_dvr.rs`).
- **Orbbec color clips and frame-at:** With sync-backed DVR, the video index points at color payload ranges inside sync minute files (`src/orbbec_sync/store_tests.rs`, `one_copy_color_and_dvr_index`). No duplicate MJPEG directory is created (`tests/orbbec_wiring.rs`).
- **Frame-set UI and tasks:** Live color is served from `/ot2/frame-sets/current/channels/color`. Stored depth, infrared, and confidence come from retained sync history (`sync-*` ids). Depth preview uses PNG; raw 16-bit depth remains available via the channel URL.
- **Rollback without data loss:** Stop the agent container (`mise run agent-stop`) but keep the mounted state directory (`A2ALAB_STATE_DIR`). Do not delete `images.sqlite`, `video-dvr/`, `orbbec-sync.sqlite`, `orbbec-sync-chunks/`, or `frame-sets.sqlite` when diagnosing device or SDK issues. See [doc-2](<../testing/doc-2 - Testing-a2a-lab-ot2.md>) agent rollback text.

## Configuration and startup validation

Enable Orbbec with `--orbbec-camera` and optional `--orbbec-serial`. Frame sets require the same enable path. Storage-related flags:

| Option                     | Environment                     | Default                           |
| -------------------------- | ------------------------------- | --------------------------------- |
| `--orbbec-sync-db`         | `A2ALAB_ORBBEC_SYNC_DB`         | `.a2a-lab-ot2/orbbec-sync.sqlite` |
| `--orbbec-sync-chunk-root` | `A2ALAB_ORBBEC_SYNC_CHUNK_ROOT` | `.a2a-lab-ot2/orbbec-sync-chunks` |
| `--frame-set-db`           | `A2ALAB_FRAME_SET_DB`           | `.a2a-lab-ot2/frame-sets.sqlite`  |
| `--frame-set-port`         | `A2ALAB_FRAME_SET_PORT`         | `31003`                           |
| `--video-dvr-root`         | `A2ALAB_VIDEO_DVR_ROOT`         | `.a2a-lab-ot2/video-dvr`          |

Before listeners bind, `serve` rejects:

- mismatched `--orbbec-serial` between image and video configuration when both are set
- `--orbbec-sync-db` equal to `--frame-set-db`
- `--orbbec-sync-chunk-root` equal to `--video-dvr-root`

Proof: `tests/orbbec_camera.rs` (`conflicting_orbbec_storage_paths_are_rejected_before_serve`) and `validate_orbbec_serve` in `src/bin/a2a-lab-ot2.rs`.

Relative paths resolve from the process working directory. In the agent container, mount `A2ALAB_STATE_DIR` at `/var/lib/a2a-lab-ot2` so all of the above survive restarts.

## Deployment verification checklist

Run these after enabling Orbbec on a host or in the agent image. Hardware-free steps run under `mise run quality`; device-backed steps are opt-in.

| Check                                | What to observe                                                                                                 | How to verify                                                                                                                                                                                               |
| ------------------------------------ | --------------------------------------------------------------------------------------------------------------- | ----------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| **One pipeline**                     | A single SDK pipeline start serves still, live, discovery, and sync                                             | `mise run quality` → `tests/orbbec_session.rs`                                                                                                                                                              |
| **Concurrent RGB and stereo access** | Live color, still capture, and retained depth/IR/confidence channels work together without a second device open | `tests/orbbec_session.rs`, `tests/frame_set_service.rs`, frame-set UI at `http://<host>:31003/ot2/frame-sets/ui` when `serve` prints `frame sets`                                                           |
| **Measured bytes per minute**        | Steady-state disk growth matches ~5 sets/s × channel payloads                                                   | On a device: note `du -sb` on `--orbbec-sync-chunk-root` before and after 60 s of streaming, or `SELECT SUM(payload_length) FROM orbbec_sync_channels WHERE payload_ready = 1` on `--orbbec-sync-db`        |
| **Restart recovery**                 | Indexes and chunks survive stop/start; partial writes are cleaned                                               | `tests/video_dvr.rs`, `tests/support/media_parity_matrix.rs` restart section; on device, restart `serve` or the agent container and confirm DVR timeline plus `sync-*` frame-set list entries still resolve |
| **Synchronized eviction**            | Oldest minute disappears as a unit when quota or free-space reserve triggers                                    | `src/orbbec_sync/store_tests.rs` (`eviction_drops_all_channels_in_oldest_minute`); on device, lower quota only in a scratch environment                                                                     |
| **External three-day cleanup**       | Only `external-camera` DVR rows older than retention expire                                                     | `tests/video_dvr.rs` retention and cleanup tests                                                                                                                                                            |
| **Dual-source soak**                 | External and Orbbec sources together: still/live reuse, live gateway, restart evidence                          | `A2ALAB_MEDIA_SMOKE=1 mise run agent-media-smoke` (container) or `A2ALAB_MEDIA_SMOKE=1 A2ALAB_MEDIA_SMOKE_NATIVE=1 mise run agent-media-smoke-native` when USB passthrough is unavailable                   |

Additional hardware-free wiring checks: `tests/orbbec_wiring.rs` (no duplicate Orbbec MJPEG under `video-dvr`), `tests/media_parity.rs` (five-second live window and restart timeline parity), `tests/agent_image.rs` (media-smoke helpers).

Opt-in live capture: `A2ALAB_ORBBEC_LIVE=1 mise run agent-orbbec-live` with `A2ALAB_ORBBEC_USB_DEVICES` set.

## Related tasks

Implementation: TASK-48 shared session, TASK-49 external minute-chunk DVR, TASK-50 synchronized retention, TASK-51 serve wiring. Documentation: TASK-52.
