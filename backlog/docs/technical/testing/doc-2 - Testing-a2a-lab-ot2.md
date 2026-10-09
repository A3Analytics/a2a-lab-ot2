---
id: doc-2
title: Testing a2a-lab-ot2
type: guide
audience: technical
created_date: "2026-10-07 02:15"
---

# Testing a2a-lab-ot2

## Purpose

Run the local quality gate, deterministic A2A-LAB compliance profile, and optional live smoke check. Local checks use controlled software fixtures. Smoke drives the pinned `v10.0.0` robot-server container and a Bedrock conversation.

Install tools with `mise install` before these paths. All commands below go through mise.

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

## Run the deterministic A2A-LAB TCK

Run the default profile from the repository root:

```bash
mise run compliance
```

The task builds and starts the deterministic OT-2 fixture, runs profile `1.1.0` over its A2A and MCP endpoints, writes `target/compliance/a2a-lab-ot2.json`, and stops the fixture. By default, it fetches and verifies standalone `A3Analytics/a2a-lab-tck` release `v0.1.0` at immutable suite revision `1fd47d5c73e6fa3f7bd6505d75e43ee68cae33fd`. `A2ALAB_TCK_ROOT` is an explicit local-source override; it does not change the default. The application separately pins the public devkit library at `9d5327868d96b3e800fd89f6debf434bcc12709d`; it does not depend on the TCK crate.

Choose a suite or disable the LLM check with these commands:

```bash
# Basic: operation-level cases only.
A2ALAB_COMPLIANCE_SUITE=basic mise run compliance

# Full: basic cases plus linked capability scenarios and the deterministic LLM check.
A2ALAB_COMPLIANCE_SUITE=full mise run compliance

# Full without the LLM check.
A2ALAB_COMPLIANCE_SUITE=full A2ALAB_COMPLIANCE_LLM_CHECK=false mise run compliance
```

`full` includes every `basic` case and is the default. The full suite also runs linked capability scenarios. Its LLM check is on by default and uses the fixture's fake model to exercise `agent-message` without Bedrock, OpenAI, Anthropic, AWS, or other model-provider credentials. Set `A2ALAB_COMPLIANCE_LLM_CHECK=false` for the explicit credential-free opt-out. The `basic` suite does not run linked or LLM scenarios.

Interpret `target/compliance/a2a-lab-ot2.json` as follows:

- `selected_suite` and `enabled_checks` record the effective basic, full, and LLM-check scope.
- `outcome: "pass"` means the case or scenario met its assertions.
- `outcome: "fail"` on a required case or scenario makes `compliant` false and the command exits nonzero.
- `outcome: "skip"` is expected only when `required` is false, such as the unavailable alternative to a required fixture capability. A required skip makes the report noncompliant.
- `compliant: true` means both interfaces were tested and every required enabled case and scenario passed. It does not extend the result beyond the tested fixture, commit, profile, and suite revision.

To exercise a controlled failure:

```bash
A2ALAB_COMPLIANCE_FIXTURE_VARIANT=mcp-metric-error mise run compliance
```

The authoritative [A2A-LAB TCK profile](https://github.com/A3Analytics/a2a-lab-tck/blob/1fd47d5c73e6fa3f7bd6505d75e43ee68cae33fd/backlog/docs/reference/compliance/doc-1%20-%20A2A-LAB-TCK-profile.md) defines required cases, suites, report fields, and pass criteria. The standalone TCK [adoption guidance](https://github.com/A3Analytics/a2a-lab-tck/blob/1fd47d5c73e6fa3f7bd6505d75e43ee68cae33fd/README.md#github-action) defines fixture, Action, evidence, and consumer-owned badge semantics. Both links are pinned to the immutable commit behind release `v0.1.0`.

The standalone TCK replaces the legacy devkit Action `v0.1.0` as the compliance runner. Its Action interface and report contract remain compatible, but TCK provenance is now separate from the application's devkit dependency provenance.

## Inspect GitHub Actions evidence

The repository-owned [A2A-LAB Compliance workflow runs](https://github.com/A3Analytics/a2a-lab-ot2/actions/workflows/a2a-lab-compliance.yml?query=branch%3Amain), which invoke the official A2A-LAB TCK, are the hosted evidence source. For a completed run, open its summary and download the `a2a-lab-compliance-<commit-sha>` artifact. It contains:

- `a2a-lab-ot2.json`, including the per-case and per-scenario outcomes;
- `controlled-noncompliance.json`, showing the workflow's controlled failing check; and
- `provenance.json`, separately identifying the tested implementation commit, public devkit dependency, immutable TCK Action and suite revisions, profile, selected suite, LLM-check state, and workflow outcome.

The successful [hosted run for commit `9356cdd272c3ea5d5db2650e65c1c596bf83c29d`](https://github.com/A3Analytics/a2a-lab-ot2/actions/runs/37968981332) uploaded [`a2a-lab-compliance-9356cdd272c3ea5d5db2650e65c1c596bf83c29d`](https://github.com/A3Analytics/a2a-lab-ot2/actions/runs/37968981332/artifacts/11634811818). It is retained historical evidence from the legacy devkit Action: its provenance records profile `1.1.0`, the full suite, devkit suite revision `15e0c68492a050715e1f50423aee55a34353c00a`, and `action_outcome: "success"`; its primary report records `compliant: true`. It does not claim a standalone TCK result. New revisions require their own standalone TCK run and artifact.

The workflow uploads available reports even when compliance fails. A setup failure can occur before a report is created. A green badge means the matching `main` workflow succeeded for its recorded commit and configuration. It is A2A-LAB compliance evidence, not certification.

## Compliance boundaries

Deterministic compliance covers only the selected A2A-LAB profile behavior exposed by the software fixture over A2A and MCP. It does not run or validate:

- `mise run smoke` or a live model-provider conversation;
- the pinned `v10.0.0` robot-server Docker simulator;
- a physical OT-2 or an untested deployment;
- on-board or external camera availability;
- Bedrock, OpenAI, Anthropic, or other live model-provider behavior;
- OpenID Connect (OIDC) authentication;
- Standardization in Lab Automation (SiLA); or
- the separate upstream A2A protocol Technology Compatibility Kit (TCK).

Those systems need their own checks. They do not inherit a compliance result or badge from the deterministic fixture.

## What the unit test suite covers

Tests bind local listeners and a mock robot-server. They do not start the Docker simulator and they do not call Bedrock, OpenAI, or Anthropic.

| File                   | Behavior under test                                                                                                                                                                                                              |
| ---------------------- | -------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `tests/inventory.rs`   | Every pinned OpenAPI operation is classified, inventory ids are unique, and read tasks are GET-backed.                                                                                                                           |
| `tests/provider.rs`    | Task, log, and metric catalogs; schemas; pause, resume, stop, and delete; recovery and stateless commands; rejected input; logs and gauges; pagination; primitive HTTP tasks; readonly advertisement and start. |
| `tests/a2a.rs`         | A2A run control, readonly over A2A, `agent-message` context, MCP tool listing, and OpenID Connect acceptance and rejection.                                                                                                      |
| `tests/agent.rs`       | Model selection, conversation history, tool calls, and one-context turn ordering.                                                                                                                                                |
| `tests/sila.rs`        | Default SiLA identity, operator PEM replacement, rejection of a partial PEM set, and a TLS bind.                                                                                                                                 |
| `tests/image_catalog.rs` | Two injectable cameras, pagination, search bounds, retention, and an isolated capture failure.                                                                                                                                |
| `tests/opentrons_camera.rs` | Mock `POST /camera/picture`, JPEG bytes and dimensions, and disabled, malformed, timeout, and unreachable failures.                                                                                                          |
| `tests/v4l2_camera.rs` | Injected V4L2 grabs, `/dev/video4` as the default, and no video device opened by the suite.                                                                                                                                      |
| `tests/image_serve.rs` | Serve limits, both source ids, readonly image reads, exact A2A skills, and exact MCP tools.                                                                                                                                      |
| `tests/image_parity.rs` | The same camera calls compared across A2A and MCP.                                                                                                                                                                              |
| `tests/image_demo.rs`  | `list-image-sources` and a saved current JPEG without a camera or the simulator.                                                                                                                                                 |
| `tests/support/mod.rs` | The mock robot-server used by the suite.                                                                                                                                                                                         |

Provider tests require every non-skip inventory id to appear in `list_tasks`, `list_log_sources`, or `list_metrics`. That is the catalog contract. [doc-1](<../ot2-http/doc-1 - OT-2-robot-server-HTTP-mapping.md>) defines how each route is classified.

Image tests cover `list_image_sources`, `list_images`, `search_images`, `get_image`, and `get_current_image` for `opentrons-camera` and, when a device is configured, `external-camera`. They mock `POST /camera/picture` and inject the V4L2 grab. `mise run quality` does not open a video device, start Docker, or use model credentials. `mise run images` lists the OT-2 camera. `A2ALAB_V4L2_DEVICE=/dev/video4 mise run images` adds the extra camera.

Expected outcomes differ by environment:

| Environment | `opentrons-camera` | `external-camera` |
| --- | --- | --- |
| Deterministic tests | Mock JPEG, or a stable error for a disabled, malformed, or unreachable robot-server. | Injected JPEG, or a stable error for a missing, busy, or timed-out device. |
| Pinned simulator | The source is advertised. A current image fails because the simulator has no camera. | Absent unless `--v4l2-device` names a node on this host. |
| Real OT-2 | A JPEG from `POST /camera/picture` when the operator enabled the camera. | Absent unless a host device is configured. It reads that device, not the robot. |
| Linux host with `--v4l2-device /dev/video4` | Depends on whether robot-server is a simulator or a real OT-2. | A JPEG from that device. |

## Live smoke

`mise run smoke` is separate from `mise run quality`. It needs:

- a running Docker daemon
- permission to build and replace the `a2a-lab-ot2-sim` image and container
- free TCP ports `31950`, `31000`, and `31001`
- Bedrock credentials that can invoke the selected model (`AWS_PROFILE` or the `default` profile, plus `AWS_REGION` or `AWS_DEFAULT_REGION` when the profile region is not enough)

Smoke always rebuilds through `mise run ot2-simulator-setup`, removes the container named `a2a-lab-ot2-sim`, and exits if `127.0.0.1:31950` stays occupied. It then starts the simulator and `a2a-lab-ot2 serve` with `A2ALAB_SILA_PORT=0`. The check covers the agent card, an A2A `list_tasks` call, the MCP tool list, two Bedrock `agent-message` turns that share a context, and the direct CLI operations `list-tasks`, `list-log-sources`, `list-metrics`, `start-task`, `get-task-status`, `pause`, `resume`, `stop`, `home`, `query-logs`, and `query-metrics`.

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
| `mise run ot2-simulator-health` | Requests `/health` on `A2ALAB_OPENTRONS_URL`, default `http://127.0.0.1:31950`.                                                  |
| `mise run a2a-lab-ot2-example`  | Prints merged `query_logs` output for the robot at `A2ALAB_OPENTRONS_URL`.                                                       |
| `mise run ot2-simulator-clean`  | Removes the simulator container, image, and `.cache/opentrons`.                                                                  |

See the [README](../../../../README.md) for the simulator and hardware quickstarts, and [doc-3](<../cli/doc-3 - a2a-lab-ot2-command-and-configuration-reference.md>) for command options.
