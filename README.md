# a2a-lab-sdk-example

Example lab agent that wraps a persistent Opentrons OT-2 `robot-server` simulator with [`a2a-lab-sdk`](https://github.com/A3Analytics/a2a-lab-sdk-rs). The `a2a-lab-ot2` executable calls the seven lab operations against the OT-2 HTTP API (`127.0.0.1:31950`). With no subcommand it also serves those operations over A2A (`127.0.0.1:31000`) and MCP (`127.0.0.1:31001/mcp`).

This is software simulation, not a physical robot. The simulator can also be discovered by the Opentrons App as a development robot at `127.0.0.1`.

All commands run through mise.

## Prerequisites

- [mise](https://mise.jdx.dev/)
- git
- [Docker](https://docs.docker.com/get-docker/) (Docker Desktop on macOS)

First install takes several minutes: Rust tooling plus a Docker image of Opentrons `v10.0.0` robot-server with journald.

```bash
mise install
mise run ot2-simulator-setup
```

## Run

Start the OT-2 simulator (leave this terminal running):

```bash
mise run ot2-simulator
```

In another terminal, run `a2a-lab-ot2`. With no extra args it serves A2A and MCP:

```bash
mise run start
```

Ctrl-C stops what that terminal started. Seed a serial-dilution run that fails mid-protocol (missing tip), then print logs as one OTEL stream:

```bash
mise run ot2-simulation
mise run a2a-lab-ot2-example
```

`ot2-simulation` talks to robot-server only and leaves a failed or awaiting-recovery run. Failed protocol steps stay in `run_commands` as error-level records. `api.log`, `serial.log`, `server.log`, and `update_server.log` come from `GET /logs/...` (journald inside the simulator container). If a journal is empty, the adapter fills it with realistic OT-2 records for the current run. `a2a-lab-ot2` (and `a2a-lab-ot2-example`) call those operations on the OT-2 HTTP API and print merged OTLP JSON log records (one per line).

Call the same operations yourself (point `--opentrons-url` at robot-server; default is `http://127.0.0.1:31950`):

```bash
mise run a2a-lab-ot2 -- list-tasks
mise run a2a-lab-ot2 -- list-log-sources
mise run a2a-lab-ot2 -- list-metrics
mise run a2a-lab-ot2 -- start-task
mise run a2a-lab-ot2 -- start-task --no-wait
mise run a2a-lab-ot2 -- start-task --timeout 120
mise run a2a-lab-ot2 -- get-task-status <run_id>
mise run a2a-lab-ot2 -- pause <run_id>
mise run a2a-lab-ot2 -- resume <run_id>
mise run a2a-lab-ot2 -- stop <run_id>
mise run a2a-lab-ot2 -- query-logs
mise run a2a-lab-ot2 -- query-logs run_commands api.log
mise run a2a-lab-ot2 -- query-metrics run_progress_percent
mise run a2a-lab-ot2 -- home
mise run a2a-lab-ot2 -- command home '{}'
mise run start -- --readonly
mise run a2a-lab-ot2 -- --readonly list-tasks
mise run a2a-lab-ot2 -- --readonly start-task get_protocols
```

`mise run start` is `a2a-lab-ot2` (default: A2A and MCP). `mise run ot2-simulator-health` checks `GET /health` on the simulator. `mise run ot2-simulator-clean` removes the simulator container and image.

## Endpoints

| Service                | Address                                                                         |
| ---------------------- | ------------------------------------------------------------------------------- |
| Opentrons robot-server | `http://127.0.0.1:31950`                                                        |
| A2A                    | `http://127.0.0.1:31000` (`/.well-known/agent-card.json`, `POST /message:send`) |
| MCP                    | `http://127.0.0.1:31001/mcp`                                                    |

`list-tasks` advertises composite helpers (`run_serial_dilution`, pause/resume/stop, recovery, `execute_command`) plus primitive robot-server HTTP operations from the pinned OT-2 v10.0.0 OpenAPI. Flex-only routes (estop, deck configuration, subsystems, live-stream settings) are omitted. `--readonly` keeps logs, metrics, and GET-backed tasks and hides composites and other mutations. `list-log-sources` includes run/analysis command streams and every `GET /logs/{identifier}` journal. `list-metrics` includes health, run progress, door, lights, pipette/instrument/module counts, and disk space. Path parameters and JSON bodies go in `start-task --input`.

## Tests

```bash
mise run test
mise run quality
```

`mise run ot2-simulation` plays the bundled protocol until a missing-tip error so `run_commands` includes both succeeded and failed steps. `mise run a2a-lab-ot2-example` prints `query_logs` as one OTEL log stream. `mise run smoke` is a pass/fail check against the same simulator. All three are opt-in and slower than the unit suite.

## Opentrons App

Start the simulator, then add `127.0.0.1` as a robot network address in the Opentrons App. The development robot appears as `opentrons-dev`.
