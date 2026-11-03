---
id: doc-2
title: Testing a2a-lab-ot2
type: guide
audience: technical
created_date: "2026-10-07 02:15"
---

# Testing a2a-lab-ot2

## Purpose

Run the local quality gate, deterministic A2A-LAB compliance profile, and optional live smoke check. Local checks use controlled software fixtures. Smoke drives the pinned `v10.0.0` robot-server container and a Bedrock conversation.

Install tools with `mise install` before these paths.

## Local quality checks

These tasks do not need Docker, a robot, or model credentials.

| Task                 | What a passing run means                                                                  |
| -------------------- | ----------------------------------------------------------------------------------------- |
| `mise run fmt-check` | Rust sources match the formatter.                                                         |
| `mise run check`     | The workspace type-checks with warnings denied.                                           |
| `mise run clippy`    | Clippy is clean for all targets, with warnings denied.                                    |
| `mise run test`      | The workspace test suite passes.                                                          |
| `mise run quality`   | Formatting, the warning-denied check, clippy, and the test suite all pass, in that order. |

```bash
mise run test
mise run quality
```

`mise run quality` is the gate for a change. `mise run test` is the faster loop while a failure is still inside the suite. `mise run fmt` rewrites sources; `fmt-check` only reports drift.

## Run the deterministic A2A-LAB TCK

Run the default profile from the repository root:

```bash
mise run compliance
```

The task builds and starts the deterministic OT-2 fixture, runs profile `1.1.0` over its A2A and MCP endpoints, writes `target/compliance/a2a-lab-ot2.json`, and stops the fixture. By default, it fetches and verifies standalone `A3Analytics/a2a-lab-tck` release `v0.1.0` at immutable suite revision `1fd47d5c73e6fa3f7bd6505d75e43ee68cae33fd`. `A2ALAB_TCK_ROOT` is an explicit local-source override; it does not change the default. The application separately pins the public devkit library at commit `64ea97d3edcac8f51ba802dbed7fc4a595aec763`; it does not depend on the TCK crate.

Choose a suite or disable the LLM check with these commands:

```bash
# Basic: operation-level cases only.
A2ALAB_COMPLIANCE_SUITE=basic mise run compliance

# Full: basic cases plus linked capability scenarios and the deterministic LLM check.
A2ALAB_COMPLIANCE_SUITE=full mise run compliance

# Full without the LLM check.
A2ALAB_COMPLIANCE_SUITE=full A2ALAB_COMPLIANCE_LLM_CHECK=false mise run compliance
```

`full` includes every `basic` case and is the default. The full suite also runs linked capability scenarios. Its LLM check is on by default and uses the fixture's fake model to exercise `agent-message` without Bedrock, OpenAI, Anthropic, AWS, or other model-provider credentials. Set `A2ALAB_COMPLIANCE_LLM_CHECK=false` for the explicit credential-free opt-out. The `basic` suite does not run linked or LLM scenarios.

Interpret `target/compliance/a2a-lab-ot2.json` as follows:

- `selected_suite` and `enabled_checks` record the effective basic, full, and LLM-check scope.
- `outcome: "pass"` means the case or scenario met its assertions.
- `outcome: "fail"` on a required case or scenario makes `compliant` false and the command exits nonzero.
- `outcome: "skip"` is expected only when `required` is false, such as the unavailable alternative to a required fixture capability. A required skip makes the report noncompliant.
- `compliant: true` means both interfaces were tested and every required enabled case and scenario passed. It does not extend the result beyond the tested fixture, commit, profile, and suite revision.

To exercise a controlled failure:

```bash
A2ALAB_COMPLIANCE_FIXTURE_VARIANT=mcp-metric-error mise run compliance
```

The authoritative [A2A-LAB TCK profile](https://github.com/A3Analytics/a2a-lab-tck/blob/1fd47d5c73e6fa3f7bd6505d75e43ee68cae33fd/backlog/docs/reference/compliance/doc-1%20-%20A2A-LAB-TCK-profile.md) defines required cases, suites, report fields, and pass criteria. The standalone TCK [adoption guidance](https://github.com/A3Analytics/a2a-lab-tck/blob/1fd47d5c73e6fa3f7bd6505d75e43ee68cae33fd/README.md#github-action) defines fixture, Action, evidence, and consumer-owned badge semantics. Both links are pinned to the immutable commit behind release `v0.1.0`.

The standalone TCK replaces the legacy devkit Action `v0.1.0` as the compliance runner. Its Action interface and report contract remain compatible, but TCK provenance is now separate from the application's devkit dependency provenance.

## Inspect GitHub Actions evidence

The repository-owned [A2A-LAB Compliance workflow runs](https://github.com/A3Analytics/a2a-lab-ot2/actions/workflows/a2a-lab-compliance.yml?query=branch%3Amain), which invoke the official A2A-LAB TCK, are the hosted evidence source. For a completed run, open its summary and download the `a2a-lab-compliance-<commit-sha>` artifact. It contains:

- `a2a-lab-ot2.json`, including the per-case and per-scenario outcomes;
- `controlled-noncompliance.json`, showing the workflow's controlled failing check; and
- `provenance.json`, separately identifying the tested implementation commit, public devkit dependency, immutable TCK Action and suite revisions, profile, selected suite, LLM-check state, and workflow outcome.

The successful [hosted run for commit `9356cdd272c3ea5d5db2650e65c1c596bf83c29d`](https://github.com/A3Analytics/a2a-lab-ot2/actions/runs/37968981332) uploaded [`a2a-lab-compliance-9356cdd272c3ea5d5db2650e65c1c596bf83c29d`](https://github.com/A3Analytics/a2a-lab-ot2/actions/runs/37968981332/artifacts/11634811818). It is retained historical evidence from the legacy devkit Action: its provenance records profile `1.1.0`, the full suite, devkit suite revision `15e0c68492a050715e1f50423aee55a34353c00a`, and `action_outcome: "success"`; its primary report records `compliant: true`. It does not claim a standalone TCK result. New revisions require their own standalone TCK run and artifact.

The workflow uploads available reports even when compliance fails. A setup failure can occur before a report is created. A green badge means the matching `main` workflow succeeded for its recorded commit and configuration. It is A2A-LAB compliance evidence, not certification.

## Compliance boundaries

Deterministic compliance covers only the selected A2A-LAB profile behavior exposed by the software fixture over A2A and MCP. It does not run or validate:

- `mise run smoke` or a live model-provider conversation;
- the pinned `v10.0.0` robot-server Docker simulator;
- a physical OT-2 or an untested deployment;
- on-board or external camera availability;
- Bedrock, OpenAI, Anthropic, or other live model-provider behavior;
- OpenID Connect (OIDC) authentication;
- the packaged web interface at `/ui/` (the compliance fixture stays headless and does not serve it);
- Standardization in Lab Automation (SiLA); or
- the separate upstream A2A protocol Technology Compatibility Kit (TCK).

Those systems need their own checks. They do not inherit a compliance result or badge from the deterministic fixture.

## TASK-42 foundation reconciliation (2026-11-03)

This section records how the main application checkout was merged with the
`orbbec-frame-sets` reference worktree before TASK-42 source edits landed.

| Path | Role |
| ---- | ---- |
| `/Users/dylangustaveson/code/a2a-lab-ot2` | Main checkout on `main` at `98c0cc7` before TASK-42; carries external live video, `src/opentrons/stream.rs`, and robot-server stream regression tests. |
| `/Users/dylangustaveson/code/a2a-lab-ot2-deploy` | Linked worktree on `orbbec-frame-sets` at `a0f032a` plus local edits; carries Orbbec capture, portable synchronized frame sets, agent-image scripts, and frame-set service wiring. |

### Overlapping areas

| Area | Main (`98c0cc7`) | Deploy (`a0f032a` + local) | Retained behavior |
| ---- | ---------------- | -------------------------- | ----------------- |
| Live video (`src/videos/`, CLI `--video-*`) | External Nokhwa MJPEG catalog, dev-kit `MemoryVideos` wiring, `tests/external_video.rs` | Older or absent in branch tip; local tree reintroduces video flags without stream tests | **Main:** external live video path, 5 s live memory and 3-day disk retention defaults, no `POST /camera/picture` polling for video |
| OT-2 stream contract (`src/opentrons/stream.rs`, `tests/opentrons_camera_stream.rs`) | Documents and tests pinned v10.0.0 `GET /camera/stream` as non-ingestible metadata | Branch removes stream module and stream tests | **Main:** stream fixtures, rejection helper, and hardware-free stream tests |
| Still images (`src/images/`, `src/camera.rs`) | Opentrons + external V4L2 sources, 30-day still retention | Adds `orbbec-camera`, frame-set history, matching still/live reuse hooks | **Both:** main two-source image catalog **plus** deploy Orbbec still path and frame-set SQLite (`migrations/003_frame_sets.sql`) |
| Serve binary (`src/bin/a2a-lab-ot2.rs`, `src/fixture.rs`, `src/lib.rs`) | Video serve hooks | Frame-set service, Orbbec CLI, agent-image tasks | **Both:** merged wiring — video catalog from main, frame-set service and Orbbec options from deploy |
| Docs (`doc-2`, `doc-3`, `doc-1`, `README`) | Video and stream notes | Orbbec, frame-set, and agent-image test rows | **Both:** combined tables and boundaries without dropping stream or external-video statements |
| Dependencies (`Cargo.toml`) | Unchanged dev-kit pin | Adds `image`, `libloading`, `futures-util` for Orbbec/frame-set | **Deploy additions** on top of main pin; no dev-kit repository edits |
| Agent packaging (`.mise/scripts/agent-*`, `agent/`) | Absent | Deploy-only operator scripts and Dockerfile | **Deploy:** copied scripts and agent image definitions for hardware-free `agent-image-verify` tests |

Non-overlapping deploy-only modules merged file-by-file (not directory replace):
`src/cameras/`, `src/frame_sets/`, `src/frame_set_service.rs`, `src/orbbec.rs`,
`src/orbbec_frames.rs`, and frame-set / Orbbec regression tests.

Explicit non-goals for TASK-42: full DVR disk archive (`src/videos/archive.rs`,
`persist.rs`, `scale.rs` on deploy), TASK-43–TASK-47 session/serve work, and
any change under `a2a-lab-dev-kit-rs`.

## Warm capture sessions and first-frame behavior (TASK-43)

Each enabled native camera keeps one long-lived MJPEG session for live video.
The external path uses Nokhwa after a thirty-frame settle period. The Orbbec
path opens the SDK color pipeline once and waits up to twenty seconds for the
first color frame on a cold start; later reads use a two-second per-frame
budget on the same session. Service shutdown or catalog cancellation stops the
pump threads and drops the native sessions so device handles are released.

While a session is running, `get-current-image` for that source returns the
latest validated JPEG published by the same session (via in-process frame
memory) instead of opening a second capture session. Capture failures on one
source do not stop the other.

Hardware-free coverage lives in `tests/camera_warm_sessions.rs`. Injected
`LiveJpegSource` producers stand in for Nokhwa and Orbbec SDK sessions. Those
tests cover cold-start budget, sustained capture, exact still/live byte reuse,
reconnect, cancellation, and independent failure without USB, Docker, or
systemd.

## Indexed video DVR format, lifecycle, and recovery

Disk video retention defaults to three days under `.a2a-lab-ot2/video-dvr/`
(`index.sqlite` plus per-source payload trees). Each retained MJPEG frame is
indexed with generic `VideoSourceId` and `DvrTrackId` rows, half-open UTC
segment bounds, codec metadata, and an atomic payload file (`*.mjpeg` via
`*.mjpeg.tmp` rename). Restart reopens the SQLite index and payloads without
synthesizing gapless coverage; timeline intervals and discontinuities are
derived from indexed segment starts and the source frame period.

The live capture pump keeps a short in-memory window (default five seconds,
`--video-retention-secs`) distinct from disk retention. Startup and hourly
cleanup drop expired index rows, payload files, temp files, and orphaned clip
handles per source without touching another source's tree. Clip and follow
session TTLs remain bounded in memory; expired or missing media returns stable
`clip_expired`, `not_found`, or `no_recording` errors. Filesystem and SQLite
work runs off the async executor (`spawn_blocking` for payload I/O). Partial
tmp files are removed on open. Storage misconfiguration fails before capture
starts.

Hardware-free coverage lives in `tests/video_dvr.rs` (restart, source
isolation, genuine gaps, paginated window reads, clip expiry, cleanup,
partial files, and concurrent reads during writes).

## What the unit test suite covers

Tests bind local listeners and a mock robot-server. They do not start the Docker simulator and they do not call Bedrock, OpenAI, or Anthropic.

| File                          | Behavior under test                                                                                                                                                                                             |
| ----------------------------- | --------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `tests/inventory.rs`          | Every pinned OpenAPI operation is classified, inventory ids are unique, and read tasks are GET-backed.                                                                                                          |
| `tests/provider.rs`           | Task, log, and metric catalogs; schemas; pause, resume, stop, and delete; recovery and stateless commands; rejected input; logs and gauges; pagination; primitive HTTP tasks; readonly advertisement and start. |
| `tests/a2a.rs`                | A2A run control, readonly over A2A, `agent-message` context, MCP tool listing, OpenID Connect acceptance and rejection, packaged `/ui/` routes and assets, public shell under OIDC, and a headless fixture.     |
| `tests/agent.rs`              | Model selection, conversation history, tool calls, and one-context turn ordering.                                                                                                                               |
| `tests/sila.rs`               | Default SiLA identity, operator PEM replacement, rejection of a partial PEM set, and a TLS bind.                                                                                                                |
| `tests/image_catalog.rs`      | Two injectable cameras, pagination, search bounds, retention, and an isolated capture failure.                                                                                                                  |
| `tests/opentrons_camera.rs`   | Mock `POST /camera/picture`, JPEG bytes and dimensions, and disabled, malformed, timeout, and unreachable failures.                                                                                             |
| `tests/opentrons_camera_stream.rs` | Pinned v10.0.0 `GET /camera/stream` metadata, settings rejection, and proof that OT-2 has no ingestible native stream.                                                                                     |
| `tests/external_camera.rs`    | Injected V4L2 grabs, `/dev/video4` as the default, and no video device opened by the suite.                                                                                                                     |
| `tests/external_video.rs`     | External live video catalog defaults, invalid CLI limits, injected MJPEG follow, and shared A2A/MCP video skills.                                                                                               |
| `tests/camera_warm_sessions.rs` | Warm external and Orbbec MJPEG sessions, twenty-second Orbbec cold start, still/live JPEG reuse, reconnect, shutdown, and independent source failure through injected producers.                              |
| `tests/video_dvr.rs`          | Indexed disk DVR restart, source-isolated cleanup, gap vs outside-retention frame-at, paginated window reads, clip expiry, partial tmp recovery, and async read responsiveness.                               |
| `tests/image_serve.rs`        | Serve limits, both source ids, readonly image reads, exact A2A skills, exact MCP tools, and restored camera history across A2A and MCP.                                                                         |
| `tests/image_history.rs`      | Restart restoration for both cameras, per-source retention, seven-day expiry, corrupt and oversized history, failed capture, and no database creation when only sources are listed.                             |
| `tests/image_parity.rs`       | The same camera calls compared across A2A and MCP.                                                                                                                                                              |
| `tests/orbbec_camera.rs`      | Disabled and enabled Orbbec discovery, serial selection, JPEG history, and capture failures without an Orbbec device.                                                                                           |
| `tests/frame_set_contract.rs` | Portable frame-set validation, synchronization, and the import boundary of `src/frame_sets`.                                                                                                                    |
| `tests/frame_set_history.rs`  | Restart, retention, search, and corrupt frame-set history.                                                                                                                                                      |
| `tests/frame_set_service.rs`  | A2A task results, media URLs, one-capture color reuse, and two fake backends. No RealSense SDK.                                                                                                                 |
| `tests/camera_backend.rs`     | Camera selection and the Orbbec color adapter.                                                                                                                                                                  |
| `tests/image_contract.rs`     | Shared image-source contract for `opentrons-camera`, `external-camera`, and `orbbec-camera`, including A2A and MCP parity.                                                                                      |
| `tests/agent_image.rs`        | Agent image script checks for architecture, checksum, USB arguments, and writable state. No Docker and no camera.                                                                                               |
| `tests/image_demo.rs`         | `list-image-sources` and a saved current JPEG without a camera or the simulator.                                                                                                                                |
| `tests/support/mod.rs`        | The mock robot-server used by the suite.                                                                                                                                                                        |

Provider tests require every non-skip inventory id to appear in `list_tasks`, `list_log_sources`, or `list_metrics`. That is the catalog contract. [doc-1](<../ot2-http/doc-1 - OT-2-robot-server-HTTP-mapping.md>) defines how each route is classified.

Image tests cover `list_image_sources`, `list_images`, `search_images`, `get_image`, and `get_current_image` for `opentrons-camera`, `external-camera`, and `orbbec-camera` when configured. They mock `POST /camera/picture` and inject native grabs. History tests use a temporary SQLite file and check restart, the 7-day age, retention, and failed writes. Live video tests inject MJPEG through `LiveJpegSource`; they never call `POST /camera/picture` for frames. Stream tests assert pinned robot-server v10.0.0 advertises URL templates only. `mise run quality` does not open a video device, start Docker, load Orbbec SDK, or use model credentials. Frame-set tests use in-memory and SQLite fakes. One fake reports Orbbec-shaped channels and another reports RealSense-shaped channels. Neither loads `librealsense`. `mise run agent-image-verify-all` checks the amd64 and arm64 agent images with Docker and no camera. `A2ALAB_ORBBEC_LIVE=1 mise run agent-orbbec-live` is the only check that captures from a connected Orbbec device. The agent image is `a2a-lab-ot2-agent`; `a2a-lab-ot2-sim` remains the robot-server simulator. `mise run images` lists the OT-2 camera. `A2ALAB_EXTERNAL_CAMERA_INDEX=4 mise run images` adds the extra camera.

Expected outcomes differ by environment:

| Environment                                 | `opentrons-camera`                                                                   | `external-camera`                                                               |
| ------------------------------------------- | ------------------------------------------------------------------------------------ | ------------------------------------------------------------------------------- |
| Deterministic tests                         | Mock JPEG, or a stable error for a disabled, malformed, or unreachable robot-server. | Injected JPEG, or a stable error for a missing, busy, or timed-out device.      |
| Pinned simulator                            | The source is advertised. A current image fails because the simulator has no camera. | Absent unless `--v4l2-device` names a node on this host.                        |
| Real OT-2                                   | A JPEG from `POST /camera/picture` when the operator enabled the camera.             | Absent unless a host device is configured. It reads that device, not the robot. |
| Linux host with `--v4l2-device /dev/video4` | Depends on whether robot-server is a simulator or a real OT-2.                       | A JPEG from that device.                                                        |

## Packaged web interface

Normal `serve` enables the dev-kit browser on the A2A listener. `GET /ui`, `GET /ui/`, and `GET /ui/index.html` return the shell, and referenced files load from `/ui/assets/`. An unknown asset returns 404. The page is a same-origin client of that listener.

The shell and packaged assets stay public when OpenID Connect is configured. Protected A2A protocol routes still reject a missing token and a token without the required scope. A valid access token is accepted through the HTTP bearer alternative on the agent card. The browser does not run an OpenID Connect login flow. An operator pastes the access token into the bearer field, and the page keeps it only in page memory.

The Tasks page does not cancel runs and does not call `get_current_image`. The Images page calls it when the operator captures from one selected source, then refreshes the stored-frame list. It previews only decoded payloads of at most 262144 bytes. Frames from both cameras come from `.a2a-lab-ot2/images.sqlite`. They survive a restart until they are older than 30 days or beyond the per-source `--image-retention`. Listing and searching do not capture, and opening the database does not open a camera. The service user needs write access to the database directory. Back up the SQLite file together with its `-wal` and `-shm` companions.

`tests/a2a.rs` covers those routes, the public shell, the protocol gate, and the fixture. `fixture` does not call `with_web_ui`, so `/ui/` is absent there. `mise run compliance` talks to that headless fixture.

The compliance workflow, the release workflow for every matrix target, and `mise run build-linux-x86_64` install `wasm32-unknown-unknown` before compiling. The dev-kit build script compiles the browser to wasm32 even when the OT-2 binary target is something else.

## Live smoke

`mise run smoke` is separate from `mise run quality`. It needs:

- a running Docker daemon
- permission to build and replace the `a2a-lab-ot2-sim` image and container
- free TCP ports `31950`, `31000`, and `31001`
- Bedrock credentials that can invoke the selected model (`AWS_PROFILE` or the `default` profile, plus `AWS_REGION` or `AWS_DEFAULT_REGION` when the profile region is not enough)

Smoke always rebuilds through `mise run ot2-simulator-setup`, removes the container named `a2a-lab-ot2-sim`, and exits if `127.0.0.1:31950` stays occupied. It then starts the simulator and `a2a-lab-ot2 serve` with `A2ALAB_SILA_PORT=0`. After the agent card is ready, it requests `/ui/` and checks that the shell loaded. It also covers an A2A `list_tasks` call, an A2A `start_task` of `get_protocols` plus `get_task_status` of that run, the MCP tool list, two Bedrock `agent-message` turns that share a context, and the direct CLI operations `list-tasks`, `list-log-sources`, `list-metrics`, `start-task`, `home`, `query-logs`, and `query-metrics`.

Those CLI operations call the simulator. `start-task get_protocols` lists uploaded protocols. HTTP task run ids stay in that process, so smoke reads status from the live A2A listener instead of a second CLI process. `home` issues a home command. On the way out, smoke stops its processes and removes the `a2a-lab-ot2-sim` container, including a simulator that was already using that name.

A successful run prints:

```text
smoke ok
```

## Troubleshooting

| Symptom                                                                                 | What to check                                                                                                                                                              |
| --------------------------------------------------------------------------------------- | -------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| Docker is required, or the daemon is not running                                        | Start Docker Desktop, then rerun the mise task.                                                                                                                            |
| `127.0.0.1:31950` stayed occupied                                                       | Another process is bound there. Stop it, then rerun smoke. `mise run ot2-simulator-clean` removes this repo's simulator container and image.                               |
| Agent startup names robot-server, the conversation store, SiLA, MCP, the model, or OIDC | The message includes that dependency and address. For Bedrock, refresh the profile, confirm `AWS_REGION` or `AWS_DEFAULT_REGION`, and confirm the model id can be invoked. |
| A2A or MCP fails to bind                                                                | Ports `31000` and `31001` are already taken.                                                                                                                               |
| Smoke removed a simulator you were using                                                | Smoke deletes the container named `a2a-lab-ot2-sim`. Start it again with `mise run ot2-simulator`.                                                                         |

## Simulator tasks that are not the suite

These exercise a live robot-server. They are not part of `mise run test` or `mise run quality`.

| Task                            | Effect                                                                                                                           |
| ------------------------------- | -------------------------------------------------------------------------------------------------------------------------------- |
| `mise run ot2-simulator-setup`  | Builds image `a2a-lab-ot2-sim:v10.0.0`. `OPENTRONS_TAG` selects another tag. `edge`, `latest`, `main`, and `master` are refused. |
| `mise run ot2-simulator`        | Runs container `a2a-lab-ot2-sim` at `127.0.0.1:31950`.                                                                           |
| `mise run ot2-simulator-health` | Requests `/health` on `A2ALAB_OPENTRONS_URL`, default `http://127.0.0.1:31950`.                                                  |
| `mise run a2a-lab-ot2-example`  | Prints merged `query_logs` output for the robot at `A2ALAB_OPENTRONS_URL`.                                                       |
| `mise run ot2-simulator-clean`  | Removes the simulator container, image, and `.cache/opentrons`.                                                                  |

See the [README](../../../../README.md) for the simulator and hardware quickstarts, and [doc-3](<../cli/doc-3 - a2a-lab-ot2-command-and-configuration-reference.md>) for command options.
