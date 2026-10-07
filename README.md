# a2a-lab-ot2

`a2a-lab-ot2` exposes an Opentrons OT-2 robot-server as an [a2a-lab](https://github.com/A3Analytics/a2a-lab-dev-kit-rs). One provider serves the CLI. With no subcommand, that same provider is also available over A2A, MCP, and SiLA.

It targets robot-server `v10.0.0`: the bundled simulator, or another robot-server that speaks that API.

## a2a-lab operations

The OT-2 robot-server is an a2a-lab. These operations are how a caller lists, runs, and observes it:

| Operation          | On the OT-2                                                              |
| ------------------ | ------------------------------------------------------------------------ |
| `list_tasks`       | Protocols and robot-server actions the OT-2 can run                      |
| `start_task`       | Starts one of those tasks on the robot                                   |
| `get_task_status`  | State of that run                                                        |
| `list_log_sources` | Journals and command streams the OT-2 exposes                            |
| `query_logs`       | Records from those sources                                               |
| `list_metrics`     | Gauges such as health, run progress, door, lights, and attached hardware |
| `query_metric`     | Current value of one gauge                                               |

`agent-message` is an A2A skill beside those seven. A model takes plain text and calls the lab operations through MCP. MCP and SiLA serve the seven operations.

`--readonly` limits `list_tasks` and `start_task` to reads. It is an inventory filter. It does not authenticate a caller, and it does not make the OT-2 physically safe.

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
mise run a2a-lab-ot2 -- agent-message "which tasks can I run?"
```

`mise run start` serves A2A, MCP, and SiLA after robot-server is healthy. To print logs from the robot:

```bash
mise run a2a-lab-ot2-example
```

## Real OT-2

Point `OPENTRONS_URL` at a robot-server that speaks the `v10.0.0` API.

```bash
OPENTRONS_URL=http://<robot-host>:31950 mise run ot2-simulator-health
OPENTRONS_URL=http://<robot-host>:31950 mise run a2a-lab-ot2 -- list-tasks
OPENTRONS_URL=http://<robot-host>:31950 mise run a2a-lab-ot2 -- start-task get_protocols
OPENTRONS_URL=http://<robot-host>:31950 mise run start
```

## Endpoints

| Service           | Address                      | Configurable                                                      | Authentication                                                  |
| ----------------- | ---------------------------- | ----------------------------------------------------------------- | --------------------------------------------------------------- |
| robot-server      | `http://127.0.0.1:31950`     | `--opentrons-url` or `OPENTRONS_URL`                              | none in this adapter                                            |
| A2A 1.0 HTTP+JSON | `http://127.0.0.1:31000`     | listener is fixed; `--a2a-url` changes the `agent-message` client | OpenID Connect when `--oidc-issuer` or `A2A_OIDC_ISSUER` is set |
| MCP               | `http://127.0.0.1:31001/mcp` | no                                                                | open                                                            |
| SiLA 2            | `127.0.0.1:50052`            | `--sila-host`, `--sila-port`, and `SILA_*`                        | open; TLS                                                       |

A2A serves `/.well-known/agent-card.json` and `POST /message:send`. OpenID Connect protects that A2A listener only. MCP and SiLA stay open. With an issuer, A2A expects a bearer token for audience `a2a-lab` and scope `a2a.invoke`.
