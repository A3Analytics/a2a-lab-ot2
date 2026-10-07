---
id: doc-2
title: Testing a2a-lab-ot2
type: guide
audience: technical
created_date: "2026-10-07 02:15"
---

# Testing a2a-lab-ot2

## Purpose

Run the local quality gate and, when a simulator and a model provider are available, the live smoke check. Local checks use in-process fakes. Smoke drives the pinned `v10.0.0` robot-server container and a Bedrock conversation.

Install tools with `mise install` before either path. All commands below go through mise.

## Local quality checks

These tasks do not need Docker, a robot, or model credentials.

| Task                 | What a passing run means                                                                  |
| -------------------- | ----------------------------------------------------------------------------------------- |
| `mise run fmt-check` | Rust sources match the formatter.                                                         |
| `mise run check`     | The workspace type-checks with warnings denied.                                           |
| `mise run clippy`    | Clippy is clean for all targets, with warnings denied.                                    |
| `mise run test`      | The workspace test suite passes.                                                          |
| `mise run quality`   | Formatting, the warning-denied check, clippy, and the test suite all pass, in that order. |

```bash
mise run test
mise run quality
```

`mise run quality` is the gate for a change. `mise run test` is the faster loop while a failure is still inside the suite. `mise run fmt` rewrites sources; `fmt-check` only reports drift.

## What the suite covers

Tests bind local listeners and a mock robot-server. They do not start the Docker simulator and they do not call Bedrock, OpenAI, or Anthropic.

| File                   | Behavior under test                                                                                                                                                                                                              |
| ---------------------- | -------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `tests/inventory.rs`   | Every pinned OpenAPI operation is classified, inventory ids are unique, and read tasks are GET-backed.                                                                                                                           |
| `tests/provider.rs`    | Task, log, and metric catalogs; schemas; pause, resume, stop, and delete; recovery and stateless commands; rejected input; logs and gauges; pagination; primitive HTTP tasks; readonly advertisement and start. |
| `tests/a2a.rs`         | A2A run control, readonly over A2A, `agent-message` context, MCP tool listing, and OpenID Connect acceptance and rejection.                                                                                                      |
| `tests/agent.rs`       | Model selection, conversation history, tool calls, and one-context turn ordering.                                                                                                                                                |
| `tests/sila.rs`        | Default SiLA identity, operator PEM replacement, rejection of a partial PEM set, and a TLS bind.                                                                                                                                 |
| `tests/support/mod.rs` | The mock robot-server used by the suite.                                                                                                                                                                                         |

Provider tests require every non-skip inventory id to appear in `list_tasks`, `list_log_sources`, or `list_metrics`. That is the catalog contract. [doc-1](<../ot2-http/doc-1 - OT-2-robot-server-HTTP-mapping.md>) defines how each route is classified.

## Live smoke

`mise run smoke` is separate from `mise run quality`. It needs:

- a running Docker daemon
- permission to build and replace the `a2a-lab-ot2-sim` image and container
- free TCP ports `31950`, `31000`, and `31001`
- Bedrock credentials that can invoke the selected model (`AWS_PROFILE` or the `default` profile, plus `AWS_REGION` or `AWS_DEFAULT_REGION` when the profile region is not enough)

Smoke always rebuilds through `mise run ot2-simulator-setup`, removes the container named `a2a-lab-ot2-sim`, and exits if `127.0.0.1:31950` stays occupied. It then starts the simulator and `a2a-lab-ot2 serve` with `SILA_PORT=0`. The check covers the agent card, an A2A `list_tasks` call, the MCP tool list, two Bedrock `agent-message` turns that share a context, and the direct CLI operations `list-tasks`, `list-log-sources`, `list-metrics`, `start-task`, `get-task-status`, `pause`, `resume`, `stop`, `home`, `query-logs`, and `query-metrics`.

Those CLI operations call the simulator. `start-task get_protocols` lists uploaded protocols. `home` issues a home command. On the way out, smoke stops its processes and removes the `a2a-lab-ot2-sim` container, including a simulator that was already using that name.

A successful run prints:

```text
smoke ok
```

## Troubleshooting

| Symptom                                                                                 | What to check                                                                                                                                                              |
| --------------------------------------------------------------------------------------- | -------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| Docker is required, or the daemon is not running                                        | Start Docker Desktop, then rerun the mise task.                                                                                                                            |
| `127.0.0.1:31950` stayed occupied                                                       | Another process is bound there. Stop it, then rerun smoke. `mise run ot2-simulator-clean` removes this repo's simulator container and image.                               |
| Agent startup names robot-server, the conversation store, SiLA, MCP, the model, or OIDC | The message includes that dependency and address. For Bedrock, refresh the profile, confirm `AWS_REGION` or `AWS_DEFAULT_REGION`, and confirm the model id can be invoked. |
| A2A or MCP fails to bind                                                                | Ports `31000` and `31001` are already taken.                                                                                                                               |
| Smoke removed a simulator you were using                                                | Smoke deletes the container named `a2a-lab-ot2-sim`. Start it again with `mise run ot2-simulator`.                                                                         |

## Simulator tasks that are not the suite

These exercise a live robot-server. They are not part of `mise run test` or `mise run quality`.

| Task                            | Effect                                                                                                                           |
| ------------------------------- | -------------------------------------------------------------------------------------------------------------------------------- |
| `mise run ot2-simulator-setup`  | Builds image `a2a-lab-ot2-sim:v10.0.0`. `OPENTRONS_TAG` selects another tag. `edge`, `latest`, `main`, and `master` are refused. |
| `mise run ot2-simulator`        | Runs container `a2a-lab-ot2-sim` at `127.0.0.1:31950`.                                                                           |
| `mise run ot2-simulator-health` | Requests `/health` on `OPENTRONS_URL`, default `http://127.0.0.1:31950`.                                                         |
| `mise run a2a-lab-ot2-example`  | Prints merged `query_logs` output for the robot at `OPENTRONS_URL`.                                                              |
| `mise run ot2-simulator-clean`  | Removes the simulator container, image, and `.cache/opentrons`.                                                                  |

See the [README](../../../../README.md) for the simulator and hardware quickstarts, and [doc-3](<../cli/doc-3 - a2a-lab-ot2-command-and-configuration-reference.md>) for command options.
