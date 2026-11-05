---
id: doc-5
title: Dual-format video capture implementation plan
type: technical
audience: technical
created_date: '2026-11-04 21:27'
---

# Dual-format video capture implementation plan

Deliver YUY2/YUYV and MJPEG capture with reusable Rust media code in the devkit and application/vendor wiring in OT-2. Orbbec color must explicitly normalize to YUY2. Preserve current JPEG consumers and 5 FPS H.264 delivery while retiring the FFmpeg subprocess after compatibility checks.

Status: planned, with 30 small To Do tasks across the two repositories. This document records desk research and a delivery backlog; it does not report implementation or benchmark results.

Companion: [the devkit reusable-media plan](../../../../../a2a-lab-dev-kit-rs/backlog/docs/technical/video/doc-23%20-%20Dual-format-video-capture-implementation-plan.md). Task IDs in prose are always qualified by the repository/column containing them.

## Inspected baseline and prerequisite discrepancy

Reconciled on 2026-11-04 and updated on 2026-11-05 after `main` received the older SDK stills. Current `main` (`fbafc2b` lineage, including `c7412c1`) has `src/orbbec.rs`, `src/orbbec_frames.rs`, `src/cameras/orbbec_set.rs`, and `tests/orbbec_camera.rs`. It still does not contain the shared persistent Orbbec owner, the continuous synchronized recorder, or their tests. TASK-48 through TASK-52 are Done only in the local gitignored backlog. Done metadata is not evidence that the shared session is on a branch. This task does not recreate that SDK pipeline. Downstream Orbbec session edits stay blocked until the missing source below is on a branch and the baseline tests named by those tasks pass. Those tests were not run here. `mise run quality` on this `main` passed 184 tests on 2026-11-05.

### Commits and worktrees checked

| Commit | Location | Shared session |
| --- | --- | --- |
| `b5d29c673c2716c3b985de94e19e8ad4f06f9650` | `main`, and this reconciliation branch at creation | Absent. Also absent: `src/orbbec.rs`, `src/orbbec_frames.rs`, `src/cameras/orbbec_set.rs`, `src/orbbec_sync/`, `tests/orbbec_session.rs`, `tests/orbbec_wiring.rs`, `tests/orbbec_camera.rs`, `migrations/006_orbbec_sync_history.sql`. Present: `migrations/005_video_dvr_chunks.sql` and the external minute-chunk writer in `src/videos/dvr/` (CRC32, per-minute chunks), introduced in `d1bfc62` and still on this commit. |
| `d1bfc623fd788296598ae29111e11a3bdfa1c4e7` | main history, "Refactor camera handling and remove Orbbec SDK dependencies" | Removes the SDK modules. Adds migration 005. Does not add `src/orbbec_session.rs`. |
| `a0f032a775f3c25a24c375c9a249b757829da549` | `/Users/dylangustaveson/code/a2a-lab-ot2-deploy`, branch `orbbec-frame-sets` | SDK color and frame-set code (`src/orbbec.rs`, `src/orbbec_frames.rs`, `src/cameras/orbbec_set.rs`, `tests/orbbec_camera.rs`) with MJPEG color. No `src/orbbec_session.rs`, `src/orbbec_sync/`, `tests/orbbec_session.rs`, `tests/orbbec_wiring.rs`, or migration 006. |
| `2460c410dcf6cc60195efb8597179a745d07ed8b` | `task-43-warm-camera-sessions` | Same SDK files as `a0f032a`. No shared session. |
| `de2bdf2f02866851585195c49fb6c42de7e9f4cb` | `task-44-indexed-dvr` | Same SDK files as `a0f032a`. No shared session. |
| `8a9c26aacc48d942944e0710be5a044a2a2f97f4` | `task-45-wire-serve` | Same SDK files as `a0f032a`. No shared session. |
| `c2ae21aa1f663418c9d36bcca54a78487bc2881d` | `task-46-media-parity` | Same SDK files as `a0f032a`. No shared session. |
| `495987ba7a5f6db3ec8335b2ac544f858d712a66` | `task-47-deploy-verify` | Same SDK files as `a0f032a`. No shared session. |

`git stash` is empty. `git log --all` and `git rev-list --all --objects` contain no `orbbec_session.rs`, `orbbec_sync`, `orbbec_wiring.rs`, or `006_orbbec_sync_history.sql`. Unreachable trees do not contain those paths. A filename search of the machine finds no `orbbec_session.rs`.

Cursor local history has the full TASK-52 retention document (restored as [doc-4](<../orbbec/doc-4 - Orbbec-media-retention-and-deployment-verification.md>)) plus editor copies of `src/orbbec.rs` and `tests/orbbec_wiring.rs`. It has no `orbbec_session.rs` and no `orbbec_sync` directory. Agent transcripts contain Write and StrReplace payloads for the missing files. Replaying those edits leaves unapplied patches in `src/orbbec_session.rs`, `src/orbbec_frames.rs`, `src/orbbec.rs`, the session tests, and `src/orbbec_sync/store_tests.rs`, so the log is not a consistent final tree and was not copied onto this branch.

### Artifacts that must be recovered

- `src/orbbec_session.rs` and `tests/orbbec_session.rs`. One SDK library load and one device pipeline shared by still capture, live video, source discovery, and frame-set capture. Concurrent consumers wait on one bounded frame sequence. Complete stereo uses device-advertised 5 FPS color, depth, and left/right infrared, with confidence optional. RGB remains available when full stereo cannot start, stereo is reported unavailable, and that interval is a gap. Shutdown is graceful. An optional continuous recorder publishes complete sets. TASK-48 names `shared_consumers_use_one_pipeline` and `rgb_fallback_records_stereo_gap`.
- Persistent-pipeline edits in `src/orbbec_frames.rs`: stored matched profiles, 5 FPS matching, and full-stereo versus color-only start. The older file is at `a0f032a`; these edits are not.
- `src/orbbec.rs`, `src/cameras/orbbec_set.rs`, and `src/bin/a2a-lab-ot2.rs` wiring so those consumers use the shared owner (`OrbbecSharedRuntime`) instead of opening separate pipelines. TASK-48 and TASK-51.
- `migrations/006_orbbec_sync_history.sql` and `src/orbbec_sync/` (`store`, `recorder`, `plane`, `disk`, and the store tests under `store_tests.rs` or `store/tests.rs`). Every complete 5 FPS set stores native color MJPEG, depth, left and right infrared, optional confidence, timestamps, and deduplicated calibration once. The Orbbec color DVR reads those slices. Oldest complete minutes evict together at the 256 GiB quota or the 64 GiB free-space reserve. Continuous recording does not write thirty-day stills. TASK-50. Its final summary also names `src/videos/dvr/store.rs`, `src/frame_sets/history.rs`, and `src/frame_sets/model.rs`.
- `tests/orbbec_wiring.rs`, Orbbec storage-conflict coverage in `tests/orbbec_camera.rs`, and serve wiring in `src/videos/catalog.rs`, `src/videos/serve.rs`, and `src/frame_set_service.rs`. Default live rate 5 FPS. The live pump consumes the shared session. Conflicting Orbbec serial or storage settings are rejected before listeners bind. TASK-51.
- Operator doc [doc-4](<../orbbec/doc-4 - Orbbec-media-retention-and-deployment-verification.md>), recovered from local history. TASK-52 also updated the testing guide (doc-2) and the command reference (doc-3). Those cross-links are not in the committed copies of doc-2 or doc-3.

Already on `b5d29c6`, and not a substitute for the shared session: migration 005 and `src/videos/dvr/` match the external-camera minute-chunk behavior in the TASK-49 record (checksums, offsets, ready rows, legacy `dvr_segments` still readable, startup truncation of incomplete tails). `tests/video_dvr.rs` was not re-run for this reconciliation.

### Preserved obligations

These obligations come from the TASK-48 through TASK-52 records and the recovered doc-4 text. They remain in force for the branch that next contains the recovered source.

One pipeline. Still capture, live video, source discovery, and frame-set capture share one SDK library load and one device pipeline. Concurrent consumers wait on one bounded frame sequence. The live pump reads that session. Orbbec color retention has one physical copy; the color DVR view reads the synchronized slices.

RGB-only fallback. When the full stereo profile cannot start, color MJPEG stays available, stereo is reported unavailable, and the affected interval is recorded as a gap. Continuous history stores complete bundles for the active mode: color-only rows while stereo is unavailable, and color plus depth plus left and right infrared when stereo is available. Confidence is stored when the device exposes it.

5 FPS synchronized channels. The default video rate is 5 FPS. A complete stereo set uses device-advertised 5 FPS color, depth, and left/right infrared. The recovered doc records a 200 ms sync period and a 100 ms alignment window across color, depth, and both infrared planes before a set is stored.

Restart gaps. External DVR startup recovery truncates an incomplete chunk tail and exposes a payload only after its index row is ready. Re-opening serve with the same sync database, sync chunk root, and video DVR root drops unfinished sync payloads (`payload_ready = 0`) and finalizes minute chunks. Stereo gaps stay recorded for intervals that did not produce a full set. External-camera failure, shutdown, three-day retention, and media parity stay independent of Orbbec availability.

Retention. Live memory is five seconds. The external-camera disk default is three days. The recovered doc also records a deployed-host override of one day through `A2ALAB_VIDEO_DISK_RETENTION_SECS`. The app queue capacity on this tree is 64. Orbbec sync history retains every complete synchronized set until the 256 GiB payload quota or the host free space falls below the 64 GiB reserve, then evicts the oldest complete minute across all channels together so retained channels keep identical coverage. Calibration is stored once. List, search, get, and channel operations use the index. Non-color images are encoded as PNG only for responses. Continuous recording does not write the thirty-day still-image history. Serve rejects a mismatched Orbbec serial, a sync database equal to the frame-set database, and a sync chunk root equal to the video DVR root before listeners bind.

TASK-33 through TASK-40 stay In Progress as recorded. This reconciliation references them and leaves their frame-set implementation in place. It does not change deployment or hardware configuration.

The current `src/videos/h264.rs` launches FFmpeg/libx264 with fixed 5 FPS, all-IDR baseline, no B frames, Unix FD-closing plumbing, and a streaming Annex B parser. Native external capture uses this path; current injected JPEG/webcam paths do not prove H.264 parity. The current DVR writes a literal `h264` track and overwrites track-level initialization; the provider also assumes that track ID. These are the concrete integration seams addressed by the new tasks.

Keep source IDs and the retention obligations above. Verify them against the recovered integration baseline before editing Orbbec session code.

## Library decision

The selected stack is the best fit from desk research for these requirements: retain existing capture support, avoid FFmpeg at runtime, keep conversion in Rust, use an established H.264 codec, and limit native build dependencies. This is an implementation choice, not a measured performance claim. Recheck APIs, release notes, license notices, and dependency advisories when each dependency-bearing task starts; pin the reviewed versions and lockfile then.

| Work | Selected library | Why this choice | Alternative and trigger |
| --- | --- | --- | --- |
| Webcam capture | `nokhwa` 0.10.11, optional `input-native` adapter | Already used by OT-2; exposes advertised modes and raw frame buffers through native capture backends. Preserve it rather than build per-OS capture. | Direct V4L2/Media Foundation/AVFoundation code adds ownership and portability work without a demonstrated need. |
| YUY2/I420/RGB/BGR conversion | `yuv` from awxkee/yuvutils-rs, candidate 0.8.20 | Pure Rust with stride-aware packed/planar APIs and SIMD implementations; avoids another C/C++ library. | `libyuv` through Rust FFI is the mature native alternative if fixture correctness or measured performance fails. No second converter by default. |
| JPEG decode and still encode | `image` 0.25.10 with `default-features = false`, `features = ["jpeg"]` | Established Rust image API; JPEG decoding uses zune-jpeg, and JPEG encoding and decode limits are available in one dependency. | Direct `zune-jpeg` APIs only if profiling establishes a material benefit or necessary camera-JPEG behavior unavailable through image. Avoid adding a second JPEG stack preemptively. |
| Software H.264 | `openh264` 0.9.9, bundled `source` feature | Rust API to Cisco OpenH264; persistent encoder and owned picture handling can replace the FFmpeg subprocess. | Retain existing x264 recordings for compatibility. FFmpeg/libx264 requires external/native packaging; experimental pure-Rust H.264 encoders are not the default for this application. |

OpenH264 is a C++ codec behind a Rust wrapper. It needs a C++ build toolchain and verified runtime/static linking; it is FFmpeg-free, not entirely Rust. Its suitability for current GNU/musl/Apple artifacts is a release gate, not an assumption. Hardware encoding is outside this first implementation.

### Verified API direction

- Use Nokhwa `Camera::compatible_camera_formats`, `frame`/`frame_raw`, and returned format information; avoid unconditional `decode_image`.
- Use `yuv::yuyv422_to_yuv420` for raw input to I420. Use `rgb_to_yuv422` / `bgr_to_yuv422` plus `yuv422_to_yuyv422` for packed Orbbec output. Use the library's YUYV-to-RGB conversion for still JPEG derivatives. The published package index listed yuv 0.8.20 during this review while the API page showed 0.8.19; confirm the chosen release signatures before pinning.
- Use `image::ImageReader` with JPEG format and strict limits for decoding, and `image::codecs::jpeg::JpegEncoder` for raw-source JPEG derivatives. No implicit decoder allocation without input/dimension limits.
- Use `openh264::encoder::Encoder` / `encode_at`, explicit `EncoderConfig`, and a validated I420 `YUVSource`. Copy borrowed output before the next encode. Output FPS, bitrate, VUI, frame-skip policy, and intra policy are explicit.

Sources checked 2026-11-04: [Nokhwa Camera](https://docs.rs/nokhwa/0.10.11/nokhwa/struct.Camera.html), [yuv package](https://docs.rs/crate/yuv/latest), [yuv APIs](https://docs.rs/yuv/latest/yuv/), [image JPEG features](https://docs.rs/crate/image/latest/features), [ImageReader](https://docs.rs/image/latest/image/struct.ImageReader.html), [JPEG encoder](https://docs.rs/image/latest/image/codecs/jpeg/struct.JpegEncoder.html), [OpenH264 wrapper](https://github.com/ralfbiedert/openh264-rs), [EncoderConfig](https://docs.rs/openh264/latest/openh264/encoder/struct.EncoderConfig.html), [bundled source feature](https://docs.rs/crate/openh264/latest/features).

## Ownership and data flow

| Devkit owns reusable behavior | OT-2 owns application behavior |
| --- | --- |
| Color frame/capability contracts, stride and allocation checks, profile/rate selection, optional Nokhwa capture adapter | Source identifiers, camera selection/configuration and diagnostics |
| `image` JPEG handling, `yuv` conversion, lazy JPEG derivative API, bounded fresh-frame selection | Orbbec SDK access, serial/profile enum mapping, shared session and synchronized channel association |
| Persistent OpenH264 adapter, complete-picture framing, codec-configuration identity, epoch-aware packaging contracts and fixtures | SQLite/chunk persistence/migration, retention policy, source fanout, endpoint registration and lifecycle |
| Optional Cargo features, reusable HLS/CMAF/MP4 packaging, portable fixture tests | Deployment artifacts, operator settings, hardware verification |

Raw `CaptureFormat::Yuy2` is distinct from delivery/storage `VideoCodec::H264` or `Mjpeg`. Proposed names describe responsibilities; final API names follow existing crate conventions. SDK types and OT-2 IDs must not leak into public generic contracts.

```text
Webcam/external MJPEG -> select fresh output frame -> image JPEG decode -> yuv I420 -> OpenH264
Webcam/external YUY2  -> select fresh output frame -> yuv I420 --------------------> OpenH264

Orbbec shared frame set:
  native YUYV -> pass through as YUY2
  MJPEG      -> image decode -> yuv -> YUY2
  RGB/BGR    -> yuv planar422 -> packed YUY2
  normalized color YUY2 -> yuv I420 -> OpenH264
  depth / left-right IR / confidence remain their numeric channel formats

Original JPEG, when present -> existing JPEG consumers unchanged
Raw color without JPEG     -> devkit lazy JPEG derivative -> existing JPEG consumers
Complete H.264 picture     -> app immutable DVR epoch -> shared HLS/CMAF/finite MP4
```

Orbbec color explicitly becomes YUY2 before entering the generic video path. Retain original camera JPEG alongside the normalized frame where available; do not encode it again for JPEG consumers. This conversion does not imply that the physical camera can send HD YUYV over USB2. Native YUYV is preferred only when it satisfies the requested resolution/rate; otherwise retain HD MJPEG capture and normalize in memory. Perform expensive work only for selected output frames or actual synchronized consumers, without rebuilding the shared session or converting every unused 30/60 FPS frame.

## Behavioral invariants

- Default output is 5 FPS; sensor FPS is negotiated independently. Prefer an advertised native 5 FPS mode, otherwise sample a suitable advertised higher rate by monotonic time. No sleeping consumer is allowed to accumulate stale frames.
- Preserve requested resolution. Explicit unsupported raw mode returns actual available modes; auto retains same-resolution MJPEG rather than silently choosing VGA YUY2.
- YUY2/YUYV packed order is `Y0 U0 Y1 V0`. Validate even width, supported encoder height/alignment, stride, bounds, and overflow. Preserve matrix/range metadata; use an explicit documented fallback with provenance if a backend lacks it. Match I420 range/matrix to encoder VUI.
- All representations retain source/frame identity, original UTC capture time, and device timestamps where available. Never replace capture time with decode or encode time. Preserve Orbbec calibration, device frame numbers, and synchronized association.
- Reuse one capture session per device and one existing shared Orbbec pipeline. Depth/IR/confidence retain their bit depth, units, bytes, and timing.
- Retain all-IDR/no-B-frame behavior for the first migration. Verify actual IDR NALs and independent decode; current CMAF sample flags and 8x playback rely on it. Longer GOPs are a separate optimization.
- SPS/PPS and declared frame properties are immutable per DVR epoch. Old x264 history must not be rebound to new OpenH264 initialization. No destructive DVR migration or invented recovery for already-lost initialization.
- Keep existing source IDs, JPEG consumers, metadata-only protocol responses, media URL security, retention windows, bounded queues, and honest gap reporting. Raw YUY2 capture is not a request to archive uncompressed video.

## Delivery gates and cross-repository order

Local CLI dependencies point only to lower-numbered tasks in their own repository. Cross-repository dependencies are explicit gates because Backlog IDs are repository-scoped; a bare task number from another repo is never a CLI dependency. A gate is satisfied by reviewed delivered code and passing checks, not by a document or task status alone. Before starting a downstream task, verify its required APIs in the pinned devkit revision.

| Gate | Devkit tasks that must be delivered | OT-2 consumers |
| --- | --- | --- |
| Capture gate | TASK-109, TASK-110, TASK-115 | TASK-54, TASK-55, TASK-56 |
| Pixel gate | TASK-111, TASK-112, TASK-113 | TASK-57, TASK-58 |
| Epoch contract gate | TASK-119 | TASK-59 |
| Packaging gate | TASK-120, TASK-121 | TASK-60 |
| Encoding gate | TASK-114, TASK-116, TASK-117, TASK-118 | TASK-61 |
| Media fixture gate | TASK-122 | TASK-63 |
| Portable library gate | TASK-123 | TASK-64 |
| Performance harness gate | TASK-124 | TASK-65 |

Start devkit contracts/pixel work and OT-2 baseline reconciliation independently. Each downstream app task may advance the devkit revision and lockfile to a reviewed commit containing its newly required helpers; never use a floating branch or assume an earlier capture-only pin contains the encoder. Complete the app epoch writer/provider before enabling the replacement encoder. Remove FFmpeg after the shared path works. Release-artifact and opt-in camera checks follow fixture parity. Operators decide separately when to deploy; this planning work does not change a running host.

## Task size and implementation handoff

Each row is one focused PR, with its own scope, observable acceptance criteria, and lower-numbered local prerequisites. Start with the task, this plan, the named source files, and focused tests; read additional files only for the boundary being changed. No task requires the entire prior research conversation or all transport implementations in context. Keep a typical change to one module/seam plus its tests; if a task reveals unrelated migrations or missing upstream work, record and split that work rather than absorbing it silently.

Do not populate implementation-plan fields before starting. The tables below are the delivery backlog; detailed implementation plans are added to individual tasks after moving them to In Progress. A dependency introduced by later findings may require splitting/reordering tasks so earlier tasks never reference higher-numbered tasks.

## OT-2 integration backlog

| Task | Focus | Local prerequisites |
| --- | --- | --- |
| [TASK-53](../../../tasks/task-53%20-%20Reconcile-the-recorded-shared-Orbbec-source-baseline.md) | Reconcile the recorded shared Orbbec source baseline | None |
| [TASK-54](../../../tasks/task-54%20-%20Expose-dual-format-capture-preferences-through-devkit-configuration.md) | Expose dual-format capture preferences through devkit configuration | TASK-53 |
| [TASK-55](../../../tasks/task-55%20-%20Route-webcam-and-external-capture-through-shared-raw-frame-adapters.md) | Route webcam and external capture through shared raw-frame adapters | TASK-54 |
| [TASK-56](../../../tasks/task-56%20-%20Accept-advertised-raw-and-MJPEG-color-profiles-in-the-Orbbec-session.md) | Accept advertised raw and MJPEG color profiles in the Orbbec session | TASK-54 |
| [TASK-57](../../../tasks/task-57%20-%20Normalize-every-supported-Orbbec-color-frame-to-YUY2.md) | Normalize every supported Orbbec color frame to YUY2 | TASK-56 |
| [TASK-58](../../../tasks/task-58%20-%20Keep-JPEG-image-consumers-working-with-raw-capture.md) | Keep JPEG image consumers working with raw capture | TASK-55, TASK-57 |
| [TASK-59](../../../tasks/task-59%20-%20Store-H.264-initialization-as-immutable-DVR-epochs.md) | Store H.264 initialization as immutable DVR epochs | TASK-53 |
| [TASK-60](../../../tasks/task-60%20-%20Select-retained-H.264-epochs-by-requested-time-range.md) | Select retained H.264 epochs by requested time range | TASK-59 |
| [TASK-61](../../../tasks/task-61%20-%20Use-the-shared-H.264-pipeline-for-every-local-color-source.md) | Use the shared H.264 pipeline for every local color source | TASK-58, TASK-60 |
| [TASK-62](../../../tasks/task-62%20-%20Remove-the-superseded-FFmpeg-encoder-subprocess-path.md) | Remove the superseded FFmpeg encoder subprocess path | TASK-61 |
| [TASK-63](../../../tasks/task-63%20-%20Verify-dual-format-media-parity-through-current-OT-2-contracts.md) | Verify dual-format media parity through current OT-2 contracts | TASK-62 |
| [TASK-64](../../../tasks/task-64%20-%20Verify-FFmpeg-free-encoding-in-OT-2-release-artifacts.md) | Verify FFmpeg-free encoding in OT-2 release artifacts | TASK-63 |
| [TASK-65](../../../tasks/task-65%20-%20Verify-YUY2-and-MJPEG-modes-on-chupacabra-at-5-FPS.md) | Verify YUY2 and MJPEG modes on chupacabra at 5 FPS | TASK-64 |
| [TASK-66](../../../tasks/task-66%20-%20Document-dual-format-capture-and-FFmpeg-free-operation.md) | Document dual-format capture and FFmpeg-free operation | TASK-65 |

## Devkit reusable-media task map

These IDs belong to the companion repository and are delivery references, not local CLI dependencies. See the companion plan for individual task links and local dependencies.

| Task | Focus |
| --- | --- |
| TASK-109 | Define backend-neutral captured color frames |
| TASK-110 | Select capture format and sensor rate from advertised profiles |
| TASK-111 | Convert packed YUY2 and RGB color with the yuv crate |
| TASK-112 | Decode bounded MJPEG frames into reusable color buffers |
| TASK-113 | Produce JPEG derivatives from raw color frames |
| TASK-114 | Select fresh output frames at a bounded target rate |
| TASK-115 | Add an optional Nokhwa adapter for undecoded webcam frames |
| TASK-116 | Encode I420 pictures through bundled OpenH264 |
| TASK-117 | Adapt OpenH264 pictures to existing DVR media samples |
| TASK-118 | Compose a reusable dual-format color encoding pipeline |
| TASK-119 | Define immutable H.264 configuration epochs for DVR stores |
| TASK-120 | Package HLS across sequential H.264 configuration epochs |
| TASK-121 | Preserve decoder configuration in finite H.264 exports |
| TASK-122 | Validate real dual-format media through DVR packaging |
| TASK-123 | Verify media feature isolation and portable codec builds |
| TASK-124 | Measure dual-format conversion and encoding at 5 FPS |

## Verification and rollout criteria

Use deterministic hardware-free tests for most tasks. Conversion fixtures check channel order, known color values/tolerances, padded rows, malformed/overflowing buffers, dimensions, and bounded JPEG decoding. Encoder fixtures must actually decode media, including first/final pictures and all-IDR trick-play subsets. A metadata-only mock is not enough to validate a new codec.

The fixture path covers JPEG stills/preview, HLS live/finite playback, timeline and seek, 0.5x through 8x playback, bounded MP4, reconnect, old/new DVR epochs, and metadata-only A2A/MCP plus enabled SiLA exports. Reuse existing security and transport tests. Historical arbitrary H.264-to-JPEG extraction is currently unsupported and is not silently claimed fixed by capture work.

Finite exports spanning same-dimension configuration changes must use the correct decoder configuration for every sample. Incompatible transitions, including unsupported resolution changes, must return a structured error before publication; do not publish corrupt MP4, silently truncate, or transcode history. If complete cross-resolution clip support is later required, scope it separately.

Target checks cover the current OT-2 release matrix: x86_64/aarch64 Linux GNU and musl, and aarch64 Apple Darwin. Both musl outputs must remain fully static. Verify runtime dependencies and decode a generated sample in the minimal container. Add separate library fixture coverage for Windows x86_64; this does not assert a Windows application release exists. Feature-disabled and browser WASM graphs must omit OpenH264 and native camera libraries.

Performance verification is future work, with an opt-in fixture runner and camera measurements. A 5 FPS target supplies 200 ms per output frame. Record p50/p95 processing latency, actual output/drop rate, warmup, source profile, resolution, memory, target, and hardware. Include normal MJPEG-to-I420 and the required Orbbec MJPEG-to-YUY2-to-I420 path; JPEG derivative cost is separate. An unmeasured platform is not a performance pass.

The recorded chupacabra inventory showed USB2 links and HD MJPEG profiles while raw YUYV was restricted to smaller modes on some cameras. Treat those observations as fixtures and repeat capability discovery during the opt-in hardware task; do not hard-code node numbers or assume the cabling/firmware has stayed unchanged. Test EMEET/HP HD MJPEG and available raw modes, plus Orbbec native raw and HD MJPEG normalization, concurrent consumers, single pipeline, synchronized channels, reconnect, recovery, and a bounded dual-source soak.

Every implementation task remains To Do until started. Its acceptance criteria, focused checks, implementation notes, final summary, and owning repository `mise run quality` are required before Done. Register new focused commands in `.mise/run.toml`; do not guess unsupported raw commands. Dependency/tool installation still follows repository approval rules. No dependencies, prototypes, host configuration, or deployments were performed while creating this plan.

## Boundaries

This delivery does not add RealSense/ESP32 drivers, replace the browser player, add AV1/H.265/hardware encoding, adopt longer GOPs, or rewrite existing synchronized frame-set storage. Their future adapters can reuse the color boundary. General remuxer replacement and arbitrary historical H.264-to-JPEG extraction need separate requirements.
