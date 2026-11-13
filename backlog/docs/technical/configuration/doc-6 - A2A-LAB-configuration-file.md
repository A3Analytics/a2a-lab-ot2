---
id: doc-6
title: A2A-LAB configuration file
type: guide
audience: technical
created_date: "2026-11-06 03:21"
---

# A2A-LAB configuration file

## Purpose

Select chat and live video settings from one JSON file. The file is optional. With neither `--config` nor `A2ALAB_CONFIG`, serve keeps the existing command-line and environment behavior.

Samples live in `.mise/examples/`. Each one passes `config validate` without cameras, a robot, or provider credentials. Only samples that include a `logging` object opt in to application logging at serve startup.

## Select the file

```bash
export A2ALAB_CONFIG="$PWD/.mise/examples/combined.json"
mise run a2a-lab-ot2 -- config validate
mise run start
```

`A2ALAB_CONFIG` selects the file. An explicit `--config` path wins when both are set. Relative config paths resolve from the process working directory. Paths may contain spaces. The process does not search parent directories or load a file because of its name.

A selected path that is empty, missing, unreadable, or not valid JSON fails before cameras, the model client, the robot, the database, or listeners start. The process does not fall back to defaults. `config validate` prints `configuration valid` and exits zero when the file and any explicit overrides are usable. It does not create the DVR directory.

The file is read once per invocation. Change it, then restart the process.

## Precedence

For each supported setting the winner is:

1. An explicit command-line flag.
2. An explicitly set environment variable.
3. The selected file.
4. The existing application default.

A Clap default is not an explicit override. It does not replace a value from the file. Invalid file values are errors even when a flag would have overridden them.

| File field                | Legacy override                                                    |
| ------------------------- | ------------------------------------------------------------------ |
| `videos.fps`              | `--video-fps` / `A2ALAB_VIDEO_FPS`                                 |
| `videos.live_retention`   | `--video-retention-secs` / `A2ALAB_VIDEO_RETENTION_SECS`           |
| `videos.dvr.retention`    | `--video-disk-retention-secs` / `A2ALAB_VIDEO_DISK_RETENTION_SECS` |
| `videos.dvr.storage_path` | `--video-dvr-root` / `A2ALAB_VIDEO_DVR_ROOT`                       |
| `chat.provider`           | `--model-provider` / `A2ALAB_MODEL_PROVIDER`                       |
| `chat.model`              | `--model` / `A2ALAB_MODEL`                                         |

Those legacy flags stay in whole seconds. They override the file duration after the file has parsed. A file path for DVR storage is resolved against the config file's directory, including when the process working directory is different. A path from `--video-dvr-root` or `A2ALAB_VIDEO_DVR_ROOT` keeps the existing working-directory meaning. Absolute paths stay absolute. `~` and environment variables inside a path are not expanded.

`--external-camera-index` and `--camera-index` (and their environment variables) replace the device index of `external-camera` or `camera` when that id is already in the file, and they keep the file's input format. If that id is absent, the flag adds it as MJPEG input. Other file sources stay. An explicit external-camera description updates that source only and does not enable it. `videos.sources` set to `[]` starts no capture unless one of those index flags adds a source.

Provider is chosen first. An explicit model from the command line, environment, or file wins. When the model is omitted, Bedrock still honors `--bedrock-model` / `A2ALAB_BEDROCK_MODEL`; OpenAI and Anthropic ignore that override and use the application's current default for the selected provider. The file does not infer a provider from a model id. Naming a model does not mean that provider is installed, credentialed, or able to answer.

## Duration strings

`videos.live_retention` and `videos.dvr.retention` are strings such as `5s`, `5m`, `1h`, `1h30m`, `3d`, and `1w`. A day is 24 hours and a week is seven such days. The value must be a positive whole number of seconds. `1.5m` is 90 seconds. `60m` and `1h` are the same duration. Blank strings, unitless numbers, numeric JSON, zero, negative spans, fractional seconds (`1.5s`), months, years, and overflows are rejected. The diagnostic names the field and does not repeat the rejected text.

Omitted `fps`, live retention, DVR retention, and storage path keep the current defaults: 5 frames per second, 5 seconds in memory, 3 days on disk, and `.a2a-lab-ot2/video-dvr`.

## Video sources

`videos` may be omitted. When it is present, `sources` is required. An empty list means no file-configured capture. Each source needs a unique id and `format`. Prefer `camera_id` (integer index or stable text id discovered by the devkit enumerator) over legacy `device_index`; when both are set they must agree or validation fails. Ids are the stable name for discovery, live media, retained DVR, and the runtime warm still cache. Reordering entries does not change that identity.

`format` is the camera input. `auto` lets the declarative runtime negotiate MJPEG or YUY2 at the configured width, height, and `capture_rate`. `mjpeg` requests JPEG frames. `yuy2` requests packed YUY2 bytes; `yuyv` is an alias. Input format does not select the output codec: live follow and DVR recording use the devkit color pipeline (H.264 for configured native sources). There is no `output_format` or `backend` field. Validation does not open the device, so it cannot prove the camera supports the mode; a missing or busy camera fails that source at runtime without stopping other sources.

Legacy `--external-camera-index`, `--camera-index`, and matching environment variables still add or replace `external-camera` and `camera` with MJPEG input mapped into the same declarative `VideoConfig` as file entries. When a configuration file is selected, `serve` starts only the declarative runtime path—there is no separate OT-2 capture pump.

Still images for file-configured video sources use the runtime `still_cache` when the source is enabled on `serve`; the separate `opentrons-camera` image history path is unchanged. OT-2 onboard robot pictures and Orbbec frame sets are not `videos.sources` entries.

Each configured `videos.sources` entry is also listed as a native image source. While `serve` runs, retained DVR frames from those sources are eligible for semantic indexing when the file includes a v2 `semantic_indexing` block (see below). That path is independent from `opentrons-camera`, which is always advertised for robot still capture, is not a `videos.sources` entry, does not record declarative DVR, and carries no semantic indexing identity.

### macOS built-in camera example

The local credential-free sample `.mise/examples/local.json` supports bench use on macOS. It sets `schema_version` to `2`, selects the built-in camera with `camera_id` (for example `0`) and `format: "auto"`, and pairs that source with Bedrock description and embedding providers under `semantic_indexing`. The `auto` format lets the declarative runtime negotiate MJPEG or YUY2 at the configured width, height, and `capture_rate`, matching other native sources.

```bash
export A2ALAB_CONFIG="$PWD/.mise/examples/local.json"
mise run a2a-lab-ot2 -- config validate
```

`config validate` succeeds without opening the camera and without AWS credentials because live Bedrock adapters are opt-in (see [doc-3](<../cli/doc-3 - a2a-lab-ot2-command-and-configuration-reference.md>)). Omitted `videos.dvr.retention` keeps the default indexed disk retention of three days (`259200` seconds).

While `serve` runs, A2A exposes the runtime capture-settings document through `A2aServer::with_capture_settings` (same settings surface as the devkit web UI capture view). Changing capture parameters follows the devkit settings-watch contract; edits are not written back to the JSON file.

## Semantic indexing (`schema_version` 2)

A `semantic_indexing` object is valid only when `schema_version` is `2`. It configures Luna frame descriptions, optional text embeddings, Jev frame decisions, per-source schedules (derived from `videos.sources`), and storage paths such as `semantic_indexing.limits.storage.vector_index_path`. Semantic providers are independent from `chat`; a file may use OpenAI for chat and Bedrock for indexing.

The file has no secret fields. Bedrock credentials and regions are supplied through the AWS SDK environment and shared profile chain (`AWS_PROFILE`, `AWS_REGION` or `AWS_DEFAULT_REGION`), not through JSON keys. Set `A2ALAB_SEMANTIC_INDEXING_LIVE=true` before `serve` to select live Bedrock adapters; when the variable is unset or false, deterministic providers are used so local validation, `config validate`, and tests run without AWS access.

Before live search, verify that the configured values are mutually compatible:

- `semantic_indexing.description.model` is a Bedrock model id that supports image input in your account and region.
- `semantic_indexing.embeddings.model` is an enabled Bedrock embedding model id.
- `semantic_indexing.embeddings.space` (`dimensions`, `normalization`, `metric`, and `generation`) matches the vector space produced by that embedding model. Search requests must use the same model space as `get_semantic_search_capabilities` reports; a mismatch returns `provenance.model_space does not match the configured semantic embedding model space`.

Operator commands for launch, AWS resolution, semantic search, and troubleshooting live in [doc-3](<../cli/doc-3 - a2a-lab-ot2-command-and-configuration-reference.md>). The macOS Bedrock indexing contract from the retired plan doc-8 is preserved there and in this section alongside the TASK-89 example file.

## Chat

`chat` may be omitted. `provider` is `bedrock`, `openai`, or `anthropic`. `model` is optional. When present it must contain non-whitespace text. Credentials stay in the existing provider environment and profile chain (`OPENAI_API_KEY`, `ANTHROPIC_API_KEY`, and AWS settings). The file has no secret fields.

## Application logging

The dev kit reads an optional `logging` object. When the object is present, `a2a-lab-ot2` installs application logging once during startup and keeps the subscriber for the process lifetime. When `logging` is omitted, no subscriber is installed.

Omitted `logging.format` uses human-readable stderr text. Set `"format": "json"` for structured JSON lines. Optional `logging.opentelemetry.endpoint` requires the dev kit `otel` feature at build time. The file must not contain credentials or payload samples.

`.mise/examples/logging.json` shows a minimal enabled section. `config validate` does not install logging; only normal execution after configuration selection does.

```bash
export A2ALAB_CONFIG="$PWD/.mise/examples/logging.json"
mise run start
```

## Migration

`.mise/examples/migration.json` keeps the ids `external-camera` and `camera`. Copy that file into the directory you used to start the agent, or set `videos.dvr.storage_path` to the absolute directory already used by `--video-dvr-root` (default `.a2a-lab-ot2/video-dvr` relative to that old working directory). Do not point the file at a new directory if you need the existing recordings. Historical DVR files are not deleted when you switch to the file.

## Container

When `A2ALAB_CONFIG` is set, `mise run agent-run` mounts that host file read-only at `/etc/a2alab/a2a-lab.json` and passes `--config` with that container path. An unset variable adds neither the mount nor the flag. A missing or unreadable host file fails before `docker run`.

`.mise/examples/container.json` stores DVR bytes at `/var/lib/a2a-lab-ot2/video-dvr`, which is on the writable state mount. The config mount is not writable. Device nodes stay explicit, for example `A2ALAB_EXTERNAL_VIDEO_DEVICES=/dev/video4`. Duration strings are forwarded unchanged.

```bash
export A2ALAB_CONFIG="$PWD/.mise/examples/container.json"
A2ALAB_OPENTRONS_URL=http://host.docker.internal:31950 \
A2ALAB_EXTERNAL_VIDEO_DEVICES=/dev/video4 \
A2ALAB_STATE_DIR="$PWD/.a2a-lab-ot2-agent" \
mise run agent-run
```

## Out of scope

This guide does not cover network or file video ingestion or hot reload. It has no secret fields. Unknown keys fail validation; v2 files accept `semantic_indexing` when `schema_version` is `2`.
