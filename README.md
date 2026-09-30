# a2a-lab-sdk-example

Example lab agent that wraps a persistent Opentrons OT-2 `robot-server` simulator with [`a2a-lab-sdk`](https://github.com/A3Analytics/a2a-lab-sdk-rs). The same seven lab operations are served over A2A (`127.0.0.1:31000`) and MCP (`127.0.0.1:31001/mcp`).

This is software simulation, not a physical robot. The simulator can also be discovered by the Opentrons App as a development robot at `127.0.0.1`.

All commands run through mise.

## Prerequisites

- [mise](https://mise.jdx.dev/)
- git
- Docker is not required

First install takes several minutes: Rust, Python 3.12, uv, and a checkout of Opentrons `v10.0.0`.

```bash
mise install
mise run simulator-setup
```

## Run

Start the OT-2 simulator and lab agent (leave this terminal running):

```bash
mise run start
```

Ctrl-C stops both. In a second terminal, optionally seed a paused serial-dilution run, then print logs as one OTEL stream:

```bash
mise run ot2-simulation
mise run a2a-lab-example
```

`ot2-simulation` talks to robot-server only. `a2a-lab-example` calls `query_logs` for every advertised source and prints merged OTLP JSON log records (one per line), not grouped by source.

Call the same operations yourself:

```bash
mise run a2a-lab -- list-workflows
mise run a2a-lab -- list-log-sources
mise run a2a-lab -- list-metrics
mise run a2a-lab -- start-workflow
mise run a2a-lab -- get-workflow-status <run_id>
mise run a2a-lab -- pause <run_id>
mise run a2a-lab -- resume <run_id>
mise run a2a-lab -- stop <run_id>
mise run a2a-lab -- query-logs
mise run a2a-lab -- query-logs run_commands command_errors
mise run a2a-lab -- query-metrics run_progress_percent
mise run a2a-lab -- home
mise run a2a-lab -- command home '{}'
```

To run the simulator and agent in separate terminals instead of `mise run start`:

```bash
mise run simulator
mise run serve
```

`mise run simulator-health` checks `GET /health` on the simulator. `mise run simulator-clean` removes the cached Opentrons checkout.

## Endpoints

| Service                | Address                                                                         |
| ---------------------- | ------------------------------------------------------------------------------- |
| Opentrons robot-server | `http://127.0.0.1:31950`                                                        |
| A2A                    | `http://127.0.0.1:31000` (`/.well-known/agent-card.json`, `POST /message:send`) |
| MCP                    | `http://127.0.0.1:31001/mcp`                                                    |

Workflows include `run_serial_dilution`, `pause_run`, `resume_run`, `stop_run`, `delete_run`, recovery actions, and `execute_command` for Protocol Engine commands such as `home`. Administrative, networking, and update endpoints are not exposed.

## Tests

```bash
mise run test
mise run quality
```

`mise run ot2-simulation` plays the bundled protocol on the live simulator so logs and metrics have data. `mise run a2a-lab-example` prints `query_logs` as one OTEL log stream. `mise run smoke` is a pass/fail check against the same simulator. All three are opt-in and slower than the unit suite.

## Opentrons App

Start the simulator, then add `127.0.0.1` as a robot network address in the Opentrons App. The development robot appears as `opentrons-dev`.
