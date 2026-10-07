# a2a-lab-ot2

`a2a-lab-ot2` is an OT-2 adapter for [`a2a-lab-dev-kit`](https://github.com/A3Analytics/a2a-lab-dev-kit-rs). One `OpentronsLab` provider turns a robot-server HTTP API into a2a-lab tasks, logs, and metrics. The CLI calls that provider directly. With no subcommand, the same provider is served over A2A, MCP, and SiLA.

The classified surface is the OT-2 robot-server API pinned at `v10.0.0`. Point it at the bundled simulator or at another robot-server that speaks that API. Route classification lives in [doc-1](<backlog/docs/technical/ot2-http/doc-1 - OT-2-robot-server-HTTP-mapping.md>).

## a2a-lab operations

Every advertised task, log source, and metric uses asset `opentrons-ot2` and semantic id `opentrons.robot-server`.

| Operation          | OT-2 meaning                                                                                                                                           |
| ------------------ | ------------------------------------------------------------------------------------------------------------------------------------------------------ |
| `list_tasks`       | Composite run helpers plus one task for each inventoried HTTP operation. Each task carries `input_schema` and `output_schema`.                         |
| `start_task`       | Runs one of those tasks. Path parameters and JSON bodies become the robot-server request. The robot-server JSON object comes back on `TaskRun.result`. |
| `get_task_status`  | Reads the stored `TaskRun`: `run_id`, `state`, optional `progress`, `message`, `result`, `error_kind`, and `error_identifier`.                         |
| `list_log_sources` | Fifteen sources: robot-server journals, current-run commands, command errors, protocol analyses, and stateless commands.                               |
| `query_logs`       | Reads those sources over a wide time range and, in the CLI, prints one merged OTLP JSON line per record.                                               |
| `list_metrics`     | Nine gauges: health, free disk, run progress, command count, door, lights, and attached pipette, instrument, and module counts.                        |
| `query_metric`     | Samples the current gauge. A point is returned only when the sample time falls in the requested range.                                                 |

Composite tasks sit beside the HTTP inventory. They are `run_serial_dilution`, `pause_run`, `resume_run`, `stop_run`, `delete_run`, `resume_from_recovery`, `resume_from_recovery_assuming_false_positive`, and `execute_command`. `run_serial_dilution` uploads a protocol, creates a run, and plays it. Its `progress` moves from 0 through 1 as protocol commands reach a terminal state. The bundled protocol is `protocols/serial_dilution.py`; it drops a tip and then fails on purpose. A failed command or run sets `error_kind` from `errorType` and `error_identifier` from the error id. `message` stays a short status.

`agent-message` is an eighth A2A skill. It sends plain text to a model, which calls the seven operations through the local MCP server and stores one conversation per A2A context. MCP and SiLA expose the seven operations and do not expose `agent-message`.

`--readonly` keeps logs, metrics, status, and GET-backed tasks. Composite tasks and other mutations are omitted from `list_tasks` and return `not_found` from `start_task`. That flag is an inventory filter. It is not an authorization check, and it is not a physical-safety control.

Empty `api.log`, `serial.log`, `server.log`, and `update_server.log` journals are filled with synthesized records derived from the current run. Those lines are adapter output. They are not journal text observed from the robot. Other empty journals stay empty.

## Simulator

Requires [mise](https://mise.jdx.dev/), git, and [Docker](https://docs.docker.com/get-docker/). The first image build clones Opentrons tag `v10.0.0`.

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

`mise run start` is the binary with no subcommand, so it serves A2A, MCP, and SiLA. Startup checks robot-server health before it binds those listeners. The bundled failure protocol is simulator-only:

```bash
mise run ot2-simulation
mise run a2a-lab-ot2-example
```

`ot2-simulation` leaves a failed or awaiting-recovery run. `a2a-lab-ot2-example` prints `query_logs` as one OTEL stream.

## Real OT-2

Use a robot-server that matches the `v10.0.0` API. Set `OPENTRONS_URL` to that robot and keep `--readonly` until a separate procedure authorizes motion.

```bash
OPENTRONS_URL=http://<robot-host>:31950 mise run ot2-simulator-health
OPENTRONS_URL=http://<robot-host>:31950 mise run a2a-lab-ot2 -- --readonly list-tasks
OPENTRONS_URL=http://<robot-host>:31950 mise run a2a-lab-ot2 -- --readonly list-metrics
OPENTRONS_URL=http://<robot-host>:31950 mise run a2a-lab-ot2 -- --readonly query-metrics
OPENTRONS_URL=http://<robot-host>:31950 mise run start -- --readonly
```

`--readonly` still only filters the advertised task inventory. It does not authenticate the caller, and it does not stop the robot from moving if another client calls robot-server. Do not point `run_serial_dilution`, `home`, `mise run ot2-simulation`, or another mutating task at hardware from this example. The bundled protocol moves the robot and then fails.

Synthesized journal lines can appear on hardware too, whenever those four troubleshooting journals are empty. Treat them as adapter output.

## Endpoints

| Service           | Address                      | Who can change it                                                   | Authentication                                                         |
| ----------------- | ---------------------------- | ------------------------------------------------------------------- | ---------------------------------------------------------------------- |
| robot-server      | `http://127.0.0.1:31950`     | `--opentrons-url` or `OPENTRONS_URL`                                | none in this adapter                                                   |
| A2A 1.0 HTTP+JSON | `http://127.0.0.1:31000`     | fixed listener; `--a2a-url` changes only the `agent-message` client | OpenID Connect when `--oidc-issuer` or `A2A_OIDC_ISSUER` is set        |
| MCP               | `http://127.0.0.1:31001/mcp` | fixed                                                               | open                                                                   |
| SiLA 2            | `127.0.0.1:50052`            | `--sila-host`, `--sila-port`, and the matching `SILA_*` variables   | open; TLS with a self-signed CA unless operator PEM files are supplied |

A2A serves `/.well-known/agent-card.json` and `POST /message:send`. Lab data parts use `application/json`. The card lists the seven operations plus `agent-message`. MCP speaks protocol `2026-07-28` and lists the seven operations as tools. The A2A listener also binds an ephemeral gRPC port and publishes it on the agent card.

OIDC applies to the A2A listener. MCP and SiLA stay open on their own ports. With an issuer, A2A requires a bearer token for audience `a2a-lab` and scope `a2a.invoke` unless `--oidc-audience` or `--oidc-scope` replaces those values.

## Further reading

- [doc-1](<backlog/docs/technical/ot2-http/doc-1 - OT-2-robot-server-HTTP-mapping.md>) classifies every pinned robot-server route and defines `start_task` input and `TaskRun` output.
- [doc-2](<backlog/docs/technical/testing/doc-2 - Testing-a2a-lab-ot2.md>) explains `mise run test`, `mise run quality`, and the live `mise run smoke` check.
- [doc-3](<backlog/docs/technical/cli/doc-3 - a2a-lab-ot2-command-and-configuration-reference.md>) lists every option, environment variable, subcommand, and output.
