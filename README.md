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

One command starts the simulator and the lab agent:

```bash
mise run start
```

Ctrl-C stops both. In a second terminal, call the a2a-lab operations:

```bash
mise run a2a-lab -- list-workflows
mise run a2a-lab -- list-log-sources
mise run a2a-lab -- list-metrics
mise run a2a-lab -- start-workflow
mise run a2a-lab -- get-workflow-status <run_id>
mise run a2a-lab -- pause <run_id>
mise run a2a-lab -- resume <run_id>
mise run a2a-lab -- stop <run_id>
mise run a2a-lab -- query-logs run_commands
mise run a2a-lab -- query-logs command_errors
mise run a2a-lab -- query-metrics run_progress_percent
mise run a2a-lab -- home
mise run a2a-lab -- command home '{}'
```

To run the processes in separate terminals instead:

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

`mise run smoke` starts the live simulator, exercises pause/resume/stop/home, and tears the processes down. It is opt-in and slower than the unit suite.

## Opentrons App

Start the simulator, then add `127.0.0.1` as a robot network address in the Opentrons App. The development robot appears as `opentrons-dev`.
