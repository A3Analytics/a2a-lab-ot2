---
id: doc-1
title: OT-2 robot-server HTTP mapping
type: specification
audience: technical
created_date: "2026-09-30 21:49"
---

# OT-2 robot-server HTTP mapping

## Purpose

Map every HTTP operation the pinned OT-2 `v10.0.0` robot-server actually serves onto a2a-lab's seven operations. CLI lab commands, MCP tools, and SiLA expose those seven operations. The A2A card adds `agent-message`. `list_tasks`, `list_log_sources`, and `list_metrics` are the inventory. Listener addresses, flags, and test commands are in the [README](../../../../README.md), [doc-2](<../testing/doc-2 - Testing-a2a-lab-ot2.md>), and [doc-3](<../cli/doc-3 - a2a-lab-ot2-command-and-configuration-reference.md>).

Source of truth: `GET http://127.0.0.1:31950/openapi.json` with `Opentrons-Version: *`, encoded in `src/opentrons/inventory.rs` (`OPENAPI_OPERATIONS` + `ENTRIES`). Deprecated OpenAPI routes are omitted.

## Classification

| Kind   | Meaning                                                                                          |
| ------ | ------------------------------------------------------------------------------------------------ |
| Log    | Time-ordered event streams: journals, run commands, command errors, analyses, stateless commands |
| Metric | Numeric or 0/1 gauges from GETs (health, disk, run progress, door, lights, attached counts)      |
| Task   | Mutations and object-valued reads that are not a log stream or a gauge                           |
| Skip   | Flex-only or OT-2-unsupported (not advertised)                                                   |

Composite tasks (pause/resume/stop, recovery, `execute_command`) sit beside primitive HTTP tasks. They are not OpenAPI rows.

Some GETs are both a gauge and a task (`/pipettes`, `/modules`, `/robot/door/status`, `/robot/lights`) so callers can read a number or the object. `GET /health` is metrics only.

`--readonly` on `a2a-lab-ot2` advertises and starts only GET-backed tasks. Composite tasks and non-GET HTTP tasks return `not_found`. Logs, metrics, list operations, and task status are unchanged. `start_task` stays an A2A skill and MCP tool because object-valued GETs use it.

## Start-task input

Path parameters use OpenAPI names (`runId`) or snake_case (`run_id`). A trailing `Id` also accepts `id`.

JSON bodies:

- `body` — sent as-is
- `data` — wrapped as `{ "data": ... }`
- remaining object keys — sent as the body for POST/PUT/PATCH after path keys are stripped

Query string:

- `query` — object of extra query parameters
- `seconds` — `POST /identify`

Multipart uploads (`post_protocols`, `post_data_files`, `post_wifi_keys`) take `path` (and optional `filename`).

`TaskRun.message` is a short status. Primitive reads and writes put the robot-server JSON object on `TaskRun.result` (arrays and scalars are wrapped as `{"value": ...}`). `get_task_status` for a robot-server run sets `progress` from 0 through 1 using the share of terminal protocol commands. A failed command or run sets `error_kind` from `errorType` and `error_identifier` from the error id. Auth-scoped routes (for example `PUT /system/time`) stay advertised; robot-server errors surface as `A2aLabError`.

Each advertised task includes `input_schema` and `output_schema`. Composite tasks describe `run_id`, or `commandType` plus `params` for `execute_command`. Primitive tasks describe path parameters and, for mutations, `body` and `data`.

## Interfaces

The same provider backs the CLI, A2A, MCP, and SiLA. A2A lab data parts use `application/json`. Requests that still send `application/a2a+json` remain accepted. Bind addresses, OpenID Connect, and SiLA certificate files are specified in [doc-3](<../cli/doc-3 - a2a-lab-ot2-command-and-configuration-reference.md>).

## Camera images

Image operations are `list_image_sources`, `list_images`, `search_images`, `get_image`, and `get_current_image`. They are not robot-server task ids.

`opentrons-camera` is asset `opentrons-ot2`. A current frame is `POST /camera/picture` on pinned robot-server `v10.0.0`, with the `Opentrons-Version` header. The response body is stored byte for byte as `image/jpeg`. `GET /camera/stream` returns stream status and URLs, not frames, and this agent does not call it. OT-2 live streaming is unsupported. The agent does not enable or reconfigure the camera. The operator does that.

`external-camera` is optional. It is absent until `--v4l2-device` or `A2ALAB_V4L2_DEVICE` names a host V4L2 node, for example `/dev/video4`. A current frame is the 30th JPEG from that node's stream. The capture queues every buffer the device grants and keeps those buffers streaming until that frame is read. The earlier frames are discarded so the camera can settle. The read does not pass through robot-server. The advertised description is `--v4l2-description` or `A2ALAB_V4L2_DESCRIPTION`. When neither is set, the description is "Still frame from the host Linux V4L2 camera."

The pinned simulator can advertise `opentrons-camera` and does not emulate a usable on-board camera. A current-image call against the simulator fails. A real OT-2 returns a JPEG when the camera is enabled. A Linux host returns a JPEG from `external-camera` only after that device is configured. Deterministic tests mock `POST /camera/picture` and the V4L2 grab, and they do not open a video device.

Frames are inline base64. Each process keeps a bounded number of recent frames per source. A restart drops them. A missing, disabled, busy, permission-denied, timed-out, malformed, or oversized camera fails only that source.

## Coverage

`tests/inventory.rs` requires every `OPENAPI_OPERATIONS` pair to be classified. Provider tests require every non-skip inventory id to appear in `list_tasks`, `list_sources`, or `list_metrics`.

See TASK-1 through TASK-8.
