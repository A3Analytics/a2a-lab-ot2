# a2a-lab-ot2

Example lab agent that wraps a persistent Opentrons OT-2 `robot-server` simulator with [`a2a-lab-dev-kit`](https://github.com/A3Analytics/a2a-lab-dev-kit-rs). The `a2a-lab-ot2` executable calls the seven lab operations against the OT-2 HTTP API (`127.0.0.1:31950`). With no subcommand it also serves those operations over A2A 1.0 HTTP+JSON (`127.0.0.1:31000`), MCP (`127.0.0.1:31001/mcp`), and SiLA 2 (`127.0.0.1:50052`).

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

In another terminal, serve MCP at `http://127.0.0.1:31001/mcp`, A2A 1.0 HTTP+JSON at `http://127.0.0.1:31000`, and SiLA 2 at `127.0.0.1:50052`. Structured lab skills call those MCP tools directly. Plain-text `agent-message` turns go to a Rig agent on Amazon Bedrock, which calls the same MCP tools and stores one conversation per A2A context in SQLite. SiLA serves the same lab service over TLS:

```bash
mise run start
```

Bedrock uses the AWS SDK default credential chain for `AWS_PROFILE` (or the `default` profile): environment credentials first, then the shared profile, including SSO. `AWS_REGION` overrides the profile region. The model is `global.openai.gpt-5.6-luna` unless `--model` or `BEDROCK_MODEL` selects another id the account can invoke. Run `aws sso login` when the profile session is expired. Startup checks robot-server health, opens `.a2a-lab-ot2/conversations.sqlite3`, binds SiLA, accepts an MCP session, and loads that AWS profile before it binds A2A. A failure names the dependency and address. Ctrl-C stops A2A, MCP, and SiLA.

SiLA uses UUID `0e2a0002-0000-4000-8000-000000000002`, server type `OpentronsOt2`, and a self-signed certificate. The CA is written to `.a2a-lab-ot2/sila-ca.crt`. `--sila-cert`, `--sila-key`, and `--sila-ca` replace that certificate when all three PEM files are set. `--sila-host`, `--sila-port`, and `--sila-uuid` change the listener. `--sila-announce` advertises `_sila._tcp.local.` Server-initiated clients are stored in `.a2a-lab-ot2/sila-connections.json`, and a renamed server name is stored in `.a2a-lab-ot2/sila-name`.

A2A stays open unless `--oidc-issuer` or `A2A_OIDC_ISSUER` is set. With an issuer, A2A requires a bearer token for audience `a2a-lab` (or `--oidc-audience`) and scope `a2a.invoke` (or `--oidc-scope`). `--oidc-discovery` overrides the discovery document URL. MCP and SiLA do not use that token. `agent-message` sends `--a2a-token` or `A2A_TOKEN` when the server requires one.

Check the three services:

```bash
mise run ot2-simulator-health
curl -fsS http://127.0.0.1:31000/.well-known/agent-card.json
mise run a2a-lab-ot2 -- agent-message "which tasks can I run?"
mise run a2a-lab-ot2 -- agent-message --context-id <context_id> "what did you find?"
```

The follow-up reuses the printed `context_id` and receives a new `task_id`. History for that context is in `.a2a-lab-ot2/conversations.sqlite3`. `mise run smoke` runs the simulator, agent card, an A2A `list_tasks` call through MCP, MCP tool listing, and two Bedrock conversation turns. It needs Docker and Bedrock access and stays outside `mise run quality`.

If `ot2-simulator-health` fails, the simulator is not listening on `127.0.0.1:31950`. If a turn reports the Bedrock model, refresh the profile with `aws sso login`, then confirm `AWS_REGION` or the profile region and that the model id can be invoked. If startup reports A2A or MCP, ports `31000` and `31001` are already taken.

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

`agent-message` talks only to the A2A server (`--a2a-url`, `A2A_URL`, default `http://127.0.0.1:31000`). `--model-provider` / `MODEL_PROVIDER` is `bedrock` (default), `openai`, or `anthropic`. `--model` / `MODEL` selects the model id. When it is omitted, the defaults are Bedrock `global.openai.gpt-5.6-luna`, OpenAI `gpt-5.6-luna`, and Anthropic `claude-sonnet-5`. `--bedrock-model` / `BEDROCK_MODEL` still overrides the Bedrock default. Bedrock loads `AWS_PROFILE` or the `default` profile through the AWS SDK, and `AWS_REGION` overrides that profile's region. OpenAI uses `OPENAI_API_KEY`. Anthropic uses `ANTHROPIC_API_KEY`. Each A2A context is one conversation in `--conversation-db` / `CONVERSATION_DB` (default `.a2a-lab-ot2/conversations.sqlite3`). `--history-limit` / `HISTORY_LIMIT` keeps that many Rig messages, including tool calls. A follow-up passes the printed `context_id` and starts a new A2A task. `--readonly` still hides writes from the model because the tools are the same MCP server.

`mise run start` is `a2a-lab-ot2` (default: A2A, MCP, and SiLA). `mise run ot2-simulator-health` checks `GET /health` on the simulator. `mise run ot2-simulator-clean` removes the simulator container and image.

## Endpoints

| Service                | Address                                                                                                                                                                                                                     |
| ---------------------- | --------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| Opentrons robot-server | `http://127.0.0.1:31950`                                                                                                                                                                                                    |
| A2A 1.0 HTTP+JSON      | `http://127.0.0.1:31000` (`/.well-known/agent-card.json`, `POST /message:send`, `application/json`; requests may still use `application/a2a+json`; lab data parts call MCP, plain text calls Bedrock, OpenAI, or Anthropic) |
| MCP                    | `http://127.0.0.1:31001/mcp`                                                                                                                                                                                                |
| SiLA 2                 | `127.0.0.1:50052` (TLS, self-signed CA at `.a2a-lab-ot2/sila-ca.crt`)                                                                                                                                                       |

`list-tasks` advertises composite helpers (`run_serial_dilution`, pause/resume/stop, recovery, `execute_command`) plus primitive robot-server HTTP operations from the pinned OT-2 v10.0.0 OpenAPI. Each task includes `input_schema` and `output_schema`. Flex-only routes (estop, deck configuration, subsystems, live-stream settings) are omitted. `--readonly` keeps logs, metrics, and GET-backed tasks and hides composites and other mutations. `list-log-sources` includes run/analysis command streams and every `GET /logs/{identifier}` journal. `list-metrics` includes health, run progress, door, lights, pipette/instrument/module counts, and disk space. Path parameters and JSON bodies go in `start-task --input`. HTTP task results are JSON objects on `result`. `run_serial_dilution` reports `progress` from 0 through 1. A failed command or run sets `error_kind` and `error_identifier`. `message` stays a short status.

## Tests

```bash
mise run test
mise run quality
```

`mise run ot2-simulation` plays the bundled protocol until a missing-tip error so `run_commands` includes both succeeded and failed steps. `mise run a2a-lab-ot2-example` prints `query_logs` as one OTEL log stream. `mise run smoke` is a pass/fail check against the same simulator. All three are opt-in and slower than the unit suite.

## Opentrons App

Start the simulator, then add `127.0.0.1` as a robot network address in the Opentrons App. The development robot appears as `opentrons-dev`.
