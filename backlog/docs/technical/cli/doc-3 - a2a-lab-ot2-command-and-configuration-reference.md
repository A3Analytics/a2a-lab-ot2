---
id: doc-3
title: a2a-lab-ot2 command and configuration reference
type: specification
audience: technical
created_date: "2026-10-07 02:15"
---

# a2a-lab-ot2 command and configuration reference

## Purpose

Reference for `a2a-lab-ot2`: every generated option, environment variable, subcommand, and the output a caller can observe. Route classification and `start_task` field mapping stay in [doc-1](<../ot2-http/doc-1 - OT-2-robot-server-HTTP-mapping.md>).

Invoke the binary through mise:

```bash
mise run a2a-lab-ot2 -- --help
mise run a2a-lab-ot2 -- <command> --help
mise run start -- [options]
```

`mise run start` and `mise run a2a-lab-ot2` both run the binary. `start` is the serve path. Put global options before the subcommand. `--readonly` is global, so it may also follow the subcommand. `help` prints this text or the help of a subcommand.

Generated top-level help begins `Call a2a-lab operations against an OT-2 robot-server HTTP API` and lists `serve`, `list-tasks`, `list-log-sources`, `list-metrics`, `list-image-sources`, `get-current-image`, `query-logs`, `query-metrics`, `start-task`, `get-task-status`, `pause`, `resume`, `stop`, `home`, `command`, `agent-message`, and `help`. `--version` prints `a2a-lab-ot2` and the package version.

## Where an option applies

| Commands                   | Options that change behavior                                                                                                                                                                                                                                                          |
| -------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `serve`, and no subcommand | Robot URL, readonly, camera device, image limits, conversation store, model selection, history limit, OIDC, and every SiLA setting.                                                                                                                                                  |
| `list-image-sources`, `get-current-image` | Robot URL, V4L2 device, capture timeout, retention, and decoded-byte maximum. Listing sources does not open the device or contact robot-server.                                                                                                                             |
| `agent-message`            | `--a2a-url` / `A2ALAB_URL` and `--a2a-token` / `A2ALAB_TOKEN`, plus `--context-id` and the text arguments. Other accepted flags are ignored. The generated help text says `Send plain text to the Bedrock agent over A2A`; the server-side provider is still Bedrock, OpenAI, or Anthropic. |
| Every other subcommand     | `--opentrons-url` / `A2ALAB_OPENTRONS_URL` and `--readonly`. The process does not bind A2A, MCP, or SiLA and does not open the conversation database.                                                                                                                                        |

A command-line value replaces the environment variable of the same option. The environment variable replaces the built-in default. Empty `A2ALAB_MODEL`, `A2ALAB_BEDROCK_MODEL`, `A2ALAB_OIDC_ISSUER`, and `A2ALAB_TOKEN` are treated as unset.

## Global options

| Option                    | Environment             | Default                                     | Effect                                                                                                                 |
| ------------------------- | ----------------------- | ------------------------------------------- | ---------------------------------------------------------------------------------------------------------------------- |
| `--opentrons-url`         | `A2ALAB_OPENTRONS_URL`  | `http://127.0.0.1:31950`                    | Robot-server base URL for health, runs, logs, and tasks.                                                               |
| `--readonly`              | none                    | off                                         | Advertise and execute only GET-backed tasks.                                                                           |
| `--a2a-url`               | `A2ALAB_URL`            | `http://127.0.0.1:31000`                    | A2A origin used by `agent-message`. It does not move the A2A listener.                                                 |
| `--a2alab-listen-host`    | `A2ALAB_LISTEN_HOST`    | `127.0.0.1`                                 | Bind host for A2A and MCP. `0.0.0.0` listens on every interface.                                                       |
| `--a2alab-public-url`     | `A2ALAB_PUBLIC_URL`     | none                                        | Host advertised for A2A, its gRPC interface, MCP, and SiLA. Each service keeps its own port.                           |
| `--conversation-db`       | `A2ALAB_CONVERSATION_DB` | `.a2a-lab-ot2/conversations.sqlite3`       | SQLite file of Rig messages, one conversation per A2A context. Parent directories are created. The file uses WAL mode. |
| `--model-provider`        | `A2ALAB_MODEL_PROVIDER` | `bedrock`                                  | `bedrock`, `openai`, or `anthropic`.                                                                                   |
| `--model`                 | `A2ALAB_MODEL`          | provider default below                     | Model id. A non-empty value wins over `--bedrock-model`.                                                               |
| `--bedrock-model`         | `A2ALAB_BEDROCK_MODEL`  | none                                       | Bedrock id used when `--model` is empty and the provider is Bedrock. Ignored for OpenAI and Anthropic.                 |
| `--history-limit`         | `A2ALAB_HISTORY_LIMIT`  | `40`                                       | Rig messages kept for each context. `0` is rejected on serve with `history_limit` `must keep at least one message`.    |
| `--a2a-token`             | `A2ALAB_TOKEN`          | none                                       | Bearer token sent by `agent-message` when A2A requires OpenID Connect.                                                 |
| `--oidc-issuer`           | `A2ALAB_OIDC_ISSUER`    | none                                       | When non-empty, A2A requires a bearer token. MCP and SiLA stay open.                                                   |
| `--oidc-audience`         | `A2ALAB_OIDC_AUDIENCE`  | `a2a-lab`                                  | Access-token audience checked while OIDC is enabled.                                                                   |
| `--oidc-scope`            | `A2ALAB_OIDC_SCOPE`     | `a2a.invoke`                               | Access-token scope checked while OIDC is enabled.                                                                      |
| `--oidc-discovery`        | `A2ALAB_OIDC_DISCOVERY` | `{issuer}/.well-known/openid-configuration` | Discovery document URL.                                                                                               |
| `--sila-uuid`             | `A2ALAB_SILA_UUID`      | `0e2a0002-0000-4000-8000-000000000002`     | SiLA server UUID.                                                                                                      |
| `--sila-host`             | `A2ALAB_SILA_HOST`      | `127.0.0.1`                                | SiLA bind host.                                                                                                        |
| `--sila-port`             | `A2ALAB_SILA_PORT`      | `50052`                                    | SiLA bind port. `0` selects an ephemeral port.                                                                         |
| `--sila-cert`             | `A2ALAB_SILA_CERT`      | none                                       | PEM certificate. Required together with the key and CA.                                                                |
| `--sila-key`              | `A2ALAB_SILA_KEY`       | none                                       | PEM private key for `--sila-cert`.                                                                                     |
| `--sila-ca`               | `A2ALAB_SILA_CA`        | none                                       | PEM CA for `--sila-cert`.                                                                                              |
| `--sila-name-path`        | `A2ALAB_SILA_NAME_PATH` | `.a2a-lab-ot2/sila-name`                   | Persisted SiLA server name.                                                                                            |
| `--sila-connection-store` | `A2ALAB_SILA_CONNECTION_STORE` | `.a2a-lab-ot2/sila-connections.json` | Persisted server-initiated SiLA clients.                                                                               |
| `--sila-ca-out`           | `A2ALAB_SILA_CA_OUT`    | `.a2a-lab-ot2/sila-ca.crt`                 | File written with the self-signed CA.                                                                                  |
| `--sila-announce`         | `A2ALAB_SILA_ANNOUNCE`  | off                                        | Advertise `_sila._tcp.local.` after the listener is ready.                                                             |
| `--v4l2-device`           | `A2ALAB_V4L2_DEVICE`    | off                                        | Enables `external-camera` on this host V4L2 node, for example `/dev/video4`. Omit it to serve only the OT-2 camera. The node is not opened until a current image is requested. A command-line value replaces the environment value. An empty value is rejected. |
| `--v4l2-description`      | `A2ALAB_V4L2_DESCRIPTION` | `Still frame from the host Linux V4L2 camera.` | Description advertised for `external-camera` when that source is enabled. A command-line value replaces the environment value. An empty value is rejected. |
| `--max-image-bytes`       | `A2ALAB_MAX_IMAGE_BYTES` | `67108864`                                | Decoded image maximum. The default is 64 MiB. Zero and overflow-prone values are rejected before listeners start. The same limit is used by the lab service and the A2A MCP connection. |
| `--capture-timeout-ms`    | `A2ALAB_CAPTURE_TIMEOUT_MS` | `10000`                               | Milliseconds allowed for one capture on either camera. Zero is rejected.                                          |
| `--image-retention`       | `A2ALAB_IMAGE_RETENTION` | `32`                                      | Recent frames kept for each source in this process. Zero and overflow-prone values are rejected. Evicted ids return not found. |

Relative paths are resolved from the process working directory.

## Model providers

Selection order:

1. Non-empty `--model` / `A2ALAB_MODEL`.
2. For Bedrock only, non-empty `--bedrock-model` / `A2ALAB_BEDROCK_MODEL`.
3. Provider default: Bedrock `global.openai.gpt-5.6-luna`, OpenAI `gpt-5.6-luna`, Anthropic `claude-sonnet-5`.

Credentials are process environment, not flags:

| Provider  | Variables                                                                                                                                |
| --------- | ---------------------------------------------------------------------------------------------------------------------------------------- |
| Bedrock   | AWS SDK default chain for `AWS_PROFILE`, or the `default` profile. `AWS_REGION`, then `AWS_DEFAULT_REGION`, replaces the profile region. |
| OpenAI    | `OPENAI_API_KEY`                                                                                                                         |
| Anthropic | `ANTHROPIC_API_KEY`                                                                                                                      |

The model is loaded only while serving. Startup prints `model <provider> <id>`. A credential or invoke failure names that provider and model.

## Commands and output

`list-tasks` prints `a2a-lab list_tasks` and then `id<TAB>name` for the first page of 50 tasks, sorted by id. It does not follow the next cursor. The full catalog is 7 composite tasks plus 108 primitive HTTP tasks. A2A and MCP callers can request a larger page. Readonly mode still pages at 50; that catalog is the 54 GET-backed primitives.

`list-log-sources` prints `a2a-lab list_log_sources` and `id<TAB>name` for all 15 sources. Readonly does not remove sources.

`list-metrics` prints `a2a-lab list_metrics` and `id<TAB>unit<TAB>name` for all 9 metrics. Readonly does not remove metrics.

`list-image-sources` prints `a2a-lab list_image_sources`, then `id<TAB>name`, an `asset` line when the source has one, and `description`. `opentrons-camera` is always listed and has asset `opentrons-ot2`. `external-camera` is listed only when `--v4l2-device` or `A2ALAB_V4L2_DEVICE` names a device. Its description is `--v4l2-description` or `A2ALAB_V4L2_DESCRIPTION`, or the default still-frame sentence when neither is set. This command does not open that device and does not call robot-server.

`get-current-image --source <id> --output <path>` captures one frame, writes the JPEG bytes to `<path>`, and prints `id`, `source`, `media_type`, `width`, `height`, `captured_at`, `bytes`, and `output`. The terminal does not contain the image bytes. `mise run images` lists the sources. `mise run images -- <source> <path>` saves one frame.

`opentrons-camera` uses pinned `v10.0.0` `POST /camera/picture`. The agent does not call `GET /camera/stream` and does not enable the camera. OT-2 live streaming is unsupported. `external-camera` reads the configured host V4L2 device, returns the 30th streamed frame, and never calls robot-server. Image payloads on A2A and MCP are inline base64. History is process-local. `--readonly` still serves these image reads.

`query-logs [SOURCE_IDS...]` reads every advertised source when the list is empty. Records are paged at 1,000, merged, and sorted by timestamp, source id, then message. Each line is one OTLP JSON object with `timeUnixNano`, `severityNumber`, `severityText`, `body.stringValue`, and `attributes`. `source_id` is always an attribute. Severity numbers are TRACE 1, DEBUG 5, INFO 9, WARN 13, and ERROR 17. The query range is `1970-01-01T00:00:00Z` through `2099-01-01T00:00:00Z`.

`query-metrics [METRIC_ID]` prints `a2a-lab query_metric` and then `id value`. An omitted id queries the first page of 10 metrics, which holds the current 9. A missing sample prints `-`.

`start-task TASK_ID` requires a task id. `--input` defaults to `{}`. The command waits for a terminal state unless `--no-wait` is set. `--timeout` is the wait in seconds and defaults to 60; `0` is rejected. `--no-wait` returns after the run is accepted. Output starts with `a2a-lab start_task <task>` and then the `TaskRun` lines below.

`get-task-status <RUN_ID>` prints `a2a-lab get_task_status` and the same `TaskRun` lines.

`pause <RUN_ID>`, `resume <RUN_ID>`, and `stop <RUN_ID>` call `pause_run`, `resume_run`, and `stop_run` with input `{"run_id":"<RUN_ID>"}` and wait.

`home` calls `execute_command` with `{"commandType":"home","params":{}}` and waits.

`command <COMMAND_TYPE> [PARAMS]` calls `execute_command` with `commandType` and `params`. `PARAMS` defaults to `{}` and is embedded as JSON.

`TaskRun` lines, printed only when the field is present except for id and state:

```text
run_id <id>
state submitted|working|completed|failed|canceled
progress <0 through 1>
message <short status>
result <json>
error_kind <errorType>
error_identifier <id>
```

`agent-message [TEXT...]` joins the words with spaces and prints the reply text, then `task_id <id>` when the reply has one, then `context_id <id>`. `--context-id` continues that conversation and the next turn uses a new task id. History for the context is kept by the server named in `--a2a-url`.

`serve` is the default command. A healthy start prints:

```text
images       opentrons-camera asset opentrons-ot2; external-camera off timeout 10000ms retention 32 max-bytes 67108864
OIDC         off
connected to <opentrons-url>
A2A          http://127.0.0.1:31000
MCP          http://127.0.0.1:31001/mcp
SiLA         <host:port>
SiLA CA      <ca-path>
model        <provider> <model>
conversation <sqlite-path>
```

When an issuer is set, the first line is `OIDC <issuer> (A2A only)`. Ctrl-C stops A2A, MCP, and SiLA.

## Listeners

| Listener            | Bind                                                        | Configurable |
| ------------------- | ----------------------------------------------------------- | ------------ |
| robot-server client | `--opentrons-url`                                           | yes          |
| A2A HTTP+JSON       | `--a2alab-listen-host` port `31000`                         | yes          |
| A2A gRPC            | `127.0.0.1:0`, or every interface when a public host is set | yes          |
| MCP Streamable HTTP | `--a2alab-listen-host` port `31001`, protocol `2026-07-28`  | yes          |
| SiLA gRPC/TLS       | `--sila-host` and `--sila-port`                             | yes          |

`--a2a-url` selects the client used by `agent-message`. `--a2alab-listen-host` selects the A2A and MCP bind address. `A2ALAB_PUBLIC_URL` replaces the advertised host for the agent card, including its gRPC interface, and for the printed MCP and SiLA addresses. The ports stay `31000`, `31001`, and the SiLA port.

## OpenID Connect

OIDC is off until `--oidc-issuer` or `A2ALAB_OIDC_ISSUER` is non-empty. It then protects A2A only. MCP and SiLA do not read `--a2a-token`. The agent card advertises the OpenID Connect scheme, and `agent-message` sends the bearer token to the A2A origin.

## SiLA material

The server type is `OpentronsOt2` and the default server name is `opentrons-ot2`. Supply `--sila-cert`, `--sila-key`, and `--sila-ca` together, or supply none. A partial set fails serve with `certificate, key, and CA PEM are required together`. With no operator PEM, serve writes a self-signed CA to `--sila-ca-out` and prints that path. With operator PEM, it prints the `--sila-ca` path and does not write `--sila-ca-out`.

`--sila-name-path` stores a renamed server name. `--sila-connection-store` stores server-initiated clients. Both files are used on every serve. `--sila-announce` adds mDNS advertisement of `_sila._tcp.local.`.

## Readonly behavior

`--readonly` removes the 7 composite tasks and every non-GET HTTP task from `list_tasks`. `start_task` for a removed id returns `not_found`. Logs, metrics, `get_task_status`, image reads, MCP, and SiLA still start. Image operations are not task-inventory mutations. The flag is an inventory filter. It is not an authorization check, and it is not a physical-safety control. On `agent-message` the flag is accepted and ignored by the client; a readonly MCP server hides writes from the model only when the server itself was started with `--readonly`.

## Synthetic journal records

`query_logs` for `api.log`, `serial.log`, `server.log`, and `update_server.log` uses the robot-server journal when it contains records. An empty journal is replaced with synthesized lines built from the current run and its commands. The lines describe a virtual Smoothie emulator and the run the adapter can see. They are not text read from journald. Any other empty journal, including `combined_api_server.log`, stays empty.

## Startup and failure

Serve rejects an empty V4L2 path, a zero capture timeout, a zero retention bound, and a zero or overflow-prone decoded-byte maximum before it contacts robot-server or binds a listener. It then prints the image line and checks robot-server `/health`, the conversation database, SiLA, an MCP session, the model provider, and the A2A bind. A failure is reported as `<dependency>: <cause>` and the process exits non-zero. Dependency names include `opentrons robot-server at <url>`, `conversation store at <path>`, `sila`, `mcp at http://127.0.0.1:31001/mcp`, `<provider> model <id>`, `oidc`, and `a2a at http://127.0.0.1:31000`.

Startup still succeeds when a configured camera is missing, disabled, busy, or unsupported. `opentrons-camera` stays listed. `external-camera` stays listed only when a device was configured. Only a capture of the failing source returns an error. Those errors name the source or device: missing and permission denied and busy and timeout are unavailable; a disabled camera, a malformed frame, and an oversized frame are invalid. With `--v4l2-device /dev/video4`, the image line names `external-camera device /dev/video4` and the effective description instead of `external-camera off`.

Direct commands exit non-zero when robot-server, task input, or a wait fails. A wait that exceeds `--timeout` reports `start_task wait timed out`. Unknown task ids, unknown log sources, unknown image sources, and readonly mutations surface as not-found errors from the provider.

Expected camera outcomes:

| Situation | What happens |
| --- | --- |
| Deterministic tests | Mock `POST /camera/picture` and an injected V4L2 grab. `mise run quality` does not need Docker, a camera, or model credentials. |
| Pinned simulator | `list-image-sources` includes `opentrons-camera`. A current image from that source fails because the simulator does not emulate a camera. |
| Real OT-2 | `POST /camera/picture` returns a JPEG when the operator has enabled the on-board camera. This agent does not change that setting. |
| Linux host with `--v4l2-device /dev/video4` | `external-camera` returns a JPEG from that device and does not call robot-server. Without the setting, that source is not listed. |

See the [README](../../../../README.md) for the simulator and hardware quickstarts, and [doc-2](<../testing/doc-2 - Testing-a2a-lab-ot2.md>) for the test and smoke tasks.
