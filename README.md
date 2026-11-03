# a2a-lab-ot2

[![A2A-LAB Compliance](https://github.com/A3Analytics/a2a-lab-ot2/actions/workflows/a2a-lab-compliance.yml/badge.svg?branch=main)](https://github.com/A3Analytics/a2a-lab-ot2/actions/workflows/a2a-lab-compliance.yml?query=branch%3Amain)

`a2a-lab-ot2` exposes an Opentrons OT-2 robot-server as an [A2A-LAB](https://github.com/A3Analytics/a2a-lab-dev-kit-rs).

It targets an OpenTrons 2 robot with API version `v10.0.0`.

## a2a-lab operations

The OT-2 robot-server is an a2a-lab. These operations are how a caller lists, runs, and observes it:

| Operation            | On the OT-2                                                              |
| -------------------- | ------------------------------------------------------------------------ |
| `list_tasks`         | Protocols and robot-server actions the OT-2 can run                      |
| `start_task`         | Starts one of those tasks on the robot                                   |
| `get_task_status`    | State of that run                                                        |
| `list_log_sources`   | Journals and command streams the OT-2 exposes                            |
| `query_logs`         | Records from those sources                                               |
| `list_metrics`       | Gauges such as health, run progress, door, lights, and attached hardware |
| `query_metric`       | Current value of one gauge                                               |
| `list_image_sources` | `opentrons-camera`, plus `external-camera` when a host device is set     |
| `list_images`        | Metadata for frames already captured from one source                     |
| `search_images`      | Metadata matching a source, time range, and/or caption text              |
| `get_image`          | One stored frame, inline                                                 |
| `get_current_image`  | A newly captured frame from one source, inline                           |
| `list_video_sources` | `external-camera` and/or `orbbec-camera` when those native sources are enabled |
| `get_video_frame_at` | A retained MJPEG frame from disk or the live window for one source              |
| `get_video_clip`     | A finite retained clip from the indexed DVR                                     |
| `get_video_timeline` | Retained coverage for one source                                                |
| `follow_video`       | Live follow; playback bytes are served on `/media/video-live/` on the A2A host |

`opentrons-camera` is the OT-2 on-board camera, asset `opentrons-ot2`. Current frames come from pinned robot-server `v10.0.0` `POST /camera/picture`. This agent does not call `GET /camera/stream`, does not enable the camera, and does not use the Flex live-stream service. OT-2 live streaming is unsupported. The operator enables the camera on the robot.

`external-camera` is off unless you set `--external-camera-index` or `A2ALAB_EXTERNAL_CAMERA_INDEX` to a zero-based native camera index. Capture reads that camera directly and never goes through robot-server. Omitting the setting serves only `opentrons-camera`. Its description defaults to "Still frame from the configured external camera." Set `--external-camera-description` or `A2ALAB_EXTERNAL_CAMERA_DESCRIPTION` to replace that text. An empty description is rejected.

When `--external-camera-index` or `--orbbec-camera` enables a native source, serve registers that source for live MJPEG and the indexed DVR. Each enabled source keeps an independent warm capture session. One source failing to start or capture does not remove the other. Live follow keeps five seconds of frames in memory per source (`--video-retention-secs`, default 5). The indexed DVR retains MJPEG on disk for three days by default (`--video-disk-retention-secs`) under `--video-dvr-root` (default `.a2a-lab-ot2/video-dvr`). Still-image history in `.a2a-lab-ot2/images.sqlite` remains capped by `--image-retention` and a separate thirty-day age limit. A2A and MCP return timeline, frame-at, clip, and follow metadata only; clip and live bytes are fetched from the authenticated `/media/video-clips/` and `/media/video-live/` routes on the same A2A listener through the dev-kit in-process gateway (no loopback A2A client to the process's own provider). The OT-2 onboard camera remains stills-only: `GET /camera/stream` is JSON metadata for the Flex live-stream service, which OT-2 does not run.

A requested `opentrons-camera` or `external-camera` frame is stored in `.a2a-lab-ot2/images.sqlite`. The service user must be able to create that directory and write the database. `list_images` and `search_images` read this file and do not capture. Loading it does not open either camera.

The file keeps stills for 30 days (and at most `--image-retention` frames per source).

`agent-message` is an A2A skill beside those seven. A model takes plain text and calls the lab operations through MCP. MCP and SiLA serve the A2A-LAB operations.

`--readonly` limits `list_tasks` and `start_task` to reads. Image reads stay available.

## A2A-LAB TCK

The badge reports the `main` branch status of this repository's
[A2A-LAB Compliance workflow](https://github.com/A3Analytics/a2a-lab-ot2/actions/workflows/a2a-lab-compliance.yml?query=branch%3Amain),
which runs the official A2A-LAB TCK.
Green means the identified repository commit passed the official A2A-LAB TCK profile `1.1.0`
and full suite revision recorded in that run's artifact. The badge and status remain owned by
this consumer repository.

This status is compliance evidence, not certification. It applies only to the tested deterministic
fixture and commit. It does not cover an untested deployment, the Docker simulator, a physical
OT-2, cameras, live model providers, OpenID Connect (OIDC), Standardization in Lab Automation
(SiLA), or the separate upstream A2A protocol TCK.

The standalone TCK replaces the legacy devkit Action `v0.1.0`; the application still pins its
devkit library dependency independently. See the authoritative
[A2A-LAB TCK profile](https://github.com/A3Analytics/a2a-lab-tck/blob/1fd47d5c73e6fa3f7bd6505d75e43ee68cae33fd/backlog/docs/reference/compliance/doc-1%20-%20A2A-LAB-TCK-profile.md)
and [adoption guidance](https://github.com/A3Analytics/a2a-lab-tck/blob/1fd47d5c73e6fa3f7bd6505d75e43ee68cae33fd/README.md#github-action).

Run the same default profile locally:

```bash
mise run compliance
```

See [Testing a2a-lab-ot2](<backlog/docs/technical/testing/doc-2 - Testing-a2a-lab-ot2.md>) for
basic, full, deterministic LLM, and opt-out commands; report interpretation; CI artifacts; and
scope boundaries.

## Simulator

Requires [mise](https://mise.jdx.dev/), git, and [Docker](https://docs.docker.com/get-docker/). The image is pinned to Opentrons `v10.0.0`.

```bash
mise install
mise run ot2-simulator-setup
mise run ot2-simulator
```

Leave the simulator running. In another terminal:

```bash
mise run start
mise run ot2-simulator-health
mise run a2a-lab-ot2 -- list-tasks
mise run images
mise run a2a-lab-ot2 -- agent-message "which tasks can I run?"
```

`mise run images` lists `opentrons-camera`. The pinned simulator advertises that source and does not emulate a usable on-board camera, so `get-current-image` for it fails there. A real OT-2 returns a JPEG when the camera is enabled. Set `A2ALAB_EXTERNAL_CAMERA_INDEX` to also list and capture `external-camera`. Serve then follows that same host camera as live MJPEG in the Videos UI. On the A2A lab host, the EMEET SmartCam Nova 4K is V4L2 index 4.

```bash
mise run images -- opentrons-camera frame.jpg
A2ALAB_EXTERNAL_CAMERA_INDEX=4 mise run images -- external-camera frame.jpg
A2ALAB_EXTERNAL_CAMERA_INDEX=4 mise run start
```

`mise run start` serves A2A, MCP, SiLA, and the packaged web UI after robot-server is healthy. Startup prints the UI on the same origin as A2A, at `/ui/`. To print logs from the robot:

```bash
mise run a2a-lab-ot2-example
```

## Orbbec color camera

`orbbec-camera` is off unless `--orbbec-camera`, `A2ALAB_ORBBEC_CAMERA=true`, or `--camera-backend orbbec` is set. It captures MJPEG color frames through Orbbec SDK `2.10.6`. The device is not opened until capture or live video starts. Orbbec still capture also enables `orbbec-camera` live video and the indexed DVR when configured.

Omit `--orbbec-serial` / `A2ALAB_ORBBEC_SERIAL` to use the only connected device. A missing device, permission failure, or unsupported profile returns an error naming `orbbec-camera` without affecting `external-camera`.

## Agent image deployment

`a2a-lab-ot2-agent` is separate from `a2a-lab-ot2-sim`. The simulator remains pinned robot-server `v10.0.0` and does not contain the agent or a camera SDK. The agent image is published for `linux/amd64` and `linux/arm64` with the release binary and Orbbec SDK `v2.10.6` runtime from pinned Linux tarballs (checksum verified at build time). It does not use `--privileged` or host networking.

```bash
mise run agent-image -- arm64
mise run agent-image-verify-all
```

Application state is the `/var/lib/a2a-lab-ot2` mount (`A2ALAB_STATE_DIR` on the host). The process writes `images.sqlite`, `video-dvr/`, `frame-sets.sqlite`, and the conversation database under that directory. The directory must be writable by the user that starts the container.

Dual-source deployment passes explicit device access and camera settings:

```bash
A2ALAB_OPENTRONS_URL=http://host.docker.internal:31950 \
A2ALAB_ORBBEC_CAMERA=true \
A2ALAB_ORBBEC_USB_DEVICES=/dev/bus/usb/001/002 \
A2ALAB_EXTERNAL_CAMERA_INDEX=4 \
A2ALAB_EXTERNAL_VIDEO_DEVICES=/dev/video4 \
A2ALAB_STATE_DIR=$PWD/.a2a-lab-ot2-agent \
mise run agent-run
```

`mise run agent-stop` sends SIGTERM, removes the container, and prints rollback steps that preserve retained image and video files on the host mount. Fix missing USB nodes, SDK mismatch, or storage permission errors using the script diagnostics; do not delete the state directory to roll back.

Opt-in media verification (not part of `mise run quality`):

```bash
# Linux agent container with both sources configured:
A2ALAB_MEDIA_SMOKE=1 \
A2ALAB_ORBBEC_CAMERA=true \
A2ALAB_ORBBEC_USB_DEVICES=/dev/bus/usb/001/002 \
A2ALAB_EXTERNAL_CAMERA_INDEX=4 \
A2ALAB_EXTERNAL_VIDEO_DEVICES=/dev/video4 \
mise run agent-media-smoke

# Native serve on the host (external-camera only on macOS):
A2ALAB_MEDIA_SMOKE=1 A2ALAB_MEDIA_SMOKE_NATIVE=1 \
A2ALAB_EXTERNAL_CAMERA_INDEX=0 \
mise run agent-media-smoke-native
```

The smoke check allows up to 20 seconds for the first Orbbec frame, verifies advancing live MJPEG on `/media/video-live/`, confirms `get_image` matches `get_current_image` for each verified source, restarts the agent to prove indexed DVR history survives, and prints retention policy values (5 s live memory, 259200 s disk video, 30 d still history). Each source failure is reported independently.

Install the image udev rules on Linux hosts that use Orbbec USB, reload udev, replug the camera, and pass the resulting device node in `A2ALAB_ORBBEC_USB_DEVICES`.

## Choose an external camera

Use the built-in inventory command for your operating system to identify candidate cameras:

Linux:

```bash
for camera in /sys/class/video4linux/video*; do printf '%s: ' "$(basename "$camera")"; cat "$camera/name"; done
```

macOS:

```bash
system_profiler SPCameraDataType
```

Windows PowerShell:

```powershell
Get-CimInstance Win32_PnPEntity | Where-Object PNPClass -In Camera, Image | Select-Object Name, PNPClass, Status
```

```bash
mise run a2a-lab-ot2 -- --external-camera-index 4 get-current-image --source external-camera --output frame.jpg
```

## Real OT-2

Point `A2ALAB_OPENTRONS_URL` at a robot-server that speaks the `v10.0.0` API.

```bash
A2ALAB_OPENTRONS_URL=http://<robot-host>:31950 mise run ot2-simulator-health
A2ALAB_OPENTRONS_URL=http://<robot-host>:31950 mise run a2a-lab-ot2 -- list-tasks
A2ALAB_OPENTRONS_URL=http://<robot-host>:31950 mise run a2a-lab-ot2 -- start-task get_protocols
A2ALAB_OPENTRONS_URL=http://<robot-host>:31950 mise run start
A2ALAB_OPENTRONS_URL=http://<robot-host>:31950 mise run images -- opentrons-camera frame.jpg
```

## Endpoints

| Service           | Address                      | Configurable                                                                                | Authentication                                                     |
| ----------------- | ---------------------------- | ------------------------------------------------------------------------------------------- | ------------------------------------------------------------------ |
| robot-server      | `http://127.0.0.1:31950`     | `--opentrons-url` or `A2ALAB_OPENTRONS_URL`                                                 | none in this adapter                                               |
| A2A 1.0 HTTP+JSON | `http://127.0.0.1:31000`     | `--a2alab-listen-host` / `A2ALAB_LISTEN_HOST`; `A2ALAB_PUBLIC_URL` sets the advertised host | OpenID Connect when `--oidc-issuer` or `A2ALAB_OIDC_ISSUER` is set |
| Web UI            | `http://127.0.0.1:31000/ui/` | same A2A listener and advertised origin; always on for `serve`                              | shell and assets are public; protocol routes follow A2A            |
| MCP               | `http://127.0.0.1:31001/mcp` | same bind host; `A2ALAB_PUBLIC_URL` keeps port `31001`                                      | open                                                               |
| SiLA 2            | `127.0.0.1:50052`            | `--sila-host`, `--sila-port`, and `A2ALAB_SILA_*`; `A2ALAB_PUBLIC_URL` keeps the SiLA port  | open; TLS                                                          |

A2A serves `/.well-known/agent-card.json` and `POST /message:send`. OpenID Connect protects those A2A protocol routes only. MCP and SiLA stay open. With an issuer, A2A expects a bearer token for audience `a2a-lab` and scope `a2a.invoke`.

## Web interface

`serve` always serves the packaged dev-kit browser on the existing A2A listener. There is no flag. `GET /ui`, `GET /ui/`, and `GET /ui/index.html` return the shell. Packaged assets are under `/ui/assets/`. Startup prints `Web UI` with the same origin already printed for A2A and the path `/ui/`. `A2ALAB_PUBLIC_URL` changes that origin for both lines.

The page is a same-origin client of the listener that served it. It loads the agent card and calls A2A HTTP+JSON on that origin. It does not open a second API.

Shell and asset routes are public. A2A protocol routes keep the listener's access gate.

When an issuer is set, the agent card advertises two alternative requirements: OpenID Connect, and HTTP bearer. Both use the same token check and the same required scope. The browser has no OpenID Connect login flow. Paste an access token into the bearer field. The page keeps that token in memory for the page load and sends it as `Authorization: Bearer`. A missing token is rejected. A token without the required scope is rejected. A valid token is accepted.

The Tasks page does not cancel runs and does not call `get_current_image`. The Images page captures the current image when one source is selected and lists stored frames from both cameras, including frames captured before the current process started. The page previews a decoded payload only when that payload is at most 262144 bytes.

Compiling this crate builds that browser for `wasm32-unknown-unknown`. `rust-toolchain.toml` installs the target. The release workflow and `mise run build-linux-x86_64` install it too, including when the OT-2 binary is cross-compiled.

The compliance `fixture` command does not serve `/ui/`.
