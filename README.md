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

`opentrons-camera` is the OT-2 on-board camera, asset `opentrons-ot2`. Current frames come from pinned robot-server `v10.0.0` `POST /camera/picture`. This agent does not call `GET /camera/stream`, does not enable the camera, and does not use the Flex live-stream service. OT-2 live streaming is unsupported. The operator enables the camera on the robot.

`external-camera` is off unless you set `--external-camera-index` or `A2ALAB_EXTERNAL_CAMERA_INDEX` to a zero-based native camera index. Capture reads that camera directly and never goes through robot-server. Omitting the setting serves only `opentrons-camera`. Its description defaults to "Still frame from the configured external camera." Set `--external-camera-description` or `A2ALAB_EXTERNAL_CAMERA_DESCRIPTION` to replace that text. An empty description is rejected.

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

`mise run images` lists `opentrons-camera`. The pinned simulator advertises that source and does not emulate a usable on-board camera, so `get-current-image` for it fails there. A real OT-2 returns a JPEG when the camera is enabled. Set `A2ALAB_EXTERNAL_CAMERA_INDEX` to also list and capture `external-camera`.

```bash
mise run images -- opentrons-camera frame.jpg
A2ALAB_EXTERNAL_CAMERA_INDEX=0 mise run images -- external-camera frame.jpg
```

`mise run start` serves A2A, MCP, SiLA, and the packaged web UI after robot-server is healthy. Startup prints the UI on the same origin as A2A, at `/ui/`. To print logs from the robot:

```bash
mise run a2a-lab-ot2-example
```

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
mise run a2a-lab-ot2 -- --external-camera-index 0 get-current-image --source external-camera --output frame.jpg
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

The Tasks page does not cancel runs and does not call `get_current_image`. The Images page lists stored frames. It previews a decoded payload only when that payload is at most 262144 bytes.

Compiling this crate builds that browser for `wasm32-unknown-unknown`. `rust-toolchain.toml` installs the target. The release workflow and `mise run build-linux-x86_64` install it too, including when the OT-2 binary is cross-compiled.

The compliance `fixture` command does not serve `/ui/`.
