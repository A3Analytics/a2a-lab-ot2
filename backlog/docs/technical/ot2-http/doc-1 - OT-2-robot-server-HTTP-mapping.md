---
id: doc-1
title: OT-2 robot-server HTTP mapping
type: specification
audience: technical
created_date: "2026-09-30 21:49"
---

# OT-2 robot-server HTTP mapping

## Purpose

Map every HTTP operation the pinned OT-2 `v10.0.0` robot-server simulator actually serves onto a2a-lab's seven operations. The CLI, A2A card, and MCP tools stay those seven commands. `list-tasks` / `list-log-sources` / `list-metrics` are the remaining API.

Source of truth: `GET http://127.0.0.1:31950/openapi.json` with `Opentrons-Version: *`, encoded in `src/opentrons/inventory.rs` (`OPENAPI_OPERATIONS` + `ENTRIES`). Deprecated OpenAPI routes are omitted.

## Classification

| Kind   | Meaning                                                                                          |
| ------ | ------------------------------------------------------------------------------------------------ |
| Log    | Time-ordered event streams: journals, run commands, command errors, analyses, stateless commands |
| Metric | Numeric or 0/1 gauges from GETs (health, disk, run progress, door, lights, attached counts)      |
| Task   | Mutations and object-valued reads that are not a log stream or a gauge                           |
| Skip   | Flex-only or OT-2-unsupported (not advertised)                                                   |

Composite tasks (`run_serial_dilution`, pause/resume/stop, recovery, `execute_command`) sit beside primitive HTTP tasks. They are not OpenAPI rows.

Some GETs are both a gauge and a task (`/pipettes`, `/modules`, `/robot/door/status`, `/robot/lights`) so callers can read a number or the object. `GET /health` is metrics only.

## Start-task input

Path parameters use OpenAPI names (`runId`) or snake_case (`run_id`). A trailing `Id` also accepts `id`.

JSON bodies:

- `body` — sent as-is
- `data` — wrapped as `{ "data": ... }`
- remaining object keys — sent as the body for POST/PUT/PATCH after path keys are stripped

Query string:

- `query` — object of extra query parameters
- `seconds` — `POST /identify`

Multipart uploads (`post_protocols`, `post_data_files`, `post_wifi_keys`) take `path` (and optional `filename`). `post_protocols` defaults to the bundled serial-dilution file.

`TaskRun.message` is JSON text for primitive reads/writes. Auth-scoped routes (for example `PUT /system/time`) stay advertised; robot-server errors surface as `SdkError`.

## Coverage

`tests/inventory.rs` requires every `OPENAPI_OPERATIONS` pair to be classified. Provider tests require every non-skip inventory id to appear in `list_tasks`, `list_sources`, or `list_metrics`.

See TASK-1 through TASK-7.
