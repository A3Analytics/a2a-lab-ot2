#!/usr/bin/env bash
set -euo pipefail

if [[ "${A2ALAB_MEDIA_SMOKE:-}" != 1 ]]; then
  printf 'skipping native media smoke; set A2ALAB_MEDIA_SMOKE=1\n'
  exit 0
fi
if [[ "${A2ALAB_MEDIA_SMOKE_NATIVE:-}" != 1 ]]; then
  printf 'skipping native media smoke; set A2ALAB_MEDIA_SMOKE_NATIVE=1 for host-native serve checks\n'
  exit 0
fi

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
cd "$root"

# shellcheck source=ot2-simulator-docker.sh
source "$root/.mise/scripts/ot2-simulator-docker.sh"

# shellcheck source=agent-image-lib.sh
source "$root/.mise/scripts/agent-image-lib.sh"
# shellcheck source=agent-media-smoke-lib.sh
source "$root/.mise/scripts/agent-media-smoke-lib.sh"

builtin_index="${A2ALAB_MAC_BUILTIN_DEVICE_INDEX:-${A2ALAB_CAMERA_INDEX:-}}"
usb_index="${A2ALAB_USB_WEBCAM_DEVICE_INDEX:-${A2ALAB_EXTERNAL_CAMERA_INDEX:-}}"
if [[ -z "$builtin_index" && -z "$usb_index" ]]; then
  media_smoke_diagnose "camera" "set A2ALAB_MAC_BUILTIN_DEVICE_INDEX and/or A2ALAB_USB_WEBCAM_DEVICE_INDEX (legacy A2ALAB_CAMERA_INDEX / A2ALAB_EXTERNAL_CAMERA_INDEX also work)"
  exit 1
fi

config_template="${A2ALAB_CONFIG:-$root/fixtures/native-macos-media-smoke-v1.json}"
if [[ ! -f "$config_template" ]]; then
  media_smoke_diagnose "config" "missing configuration file $config_template"
  exit 1
fi

state="${A2ALAB_STATE_DIR:-$root/.a2a-lab-ot2-native-smoke}"
mkdir -p "$state"
export A2ALAB_STATE_DIR="$state"

rendered_config="$(mktemp)"

python3 - "$config_template" "$rendered_config" "$builtin_index" "$usb_index" <<'PY'
import json
import sys

template, output, builtin, usb = sys.argv[1:5]
with open(template, encoding="utf-8") as handle:
    document = json.load(handle)
sources = document.get("videos", {}).get("sources", [])
for source in sources:
    source_id = source.get("id")
    if source_id == "mac-built-in" and builtin:
        source["device_index"] = int(builtin)
    if source_id == "usb-webcam" and usb:
        source["device_index"] = int(usb)
dvr = document.setdefault("videos", {}).setdefault("dvr", {})
if dvr.get("retention") != "3h":
    raise SystemExit("native macOS smoke fixture must keep videos.dvr.retention at 3h (10800 seconds)")
with open(output, "w", encoding="utf-8") as handle:
    json.dump(document, handle, indent=2)
    handle.write("\n")
PY

export A2ALAB_CONFIG="$rendered_config"

sim_log="$(mktemp)"
sim_pid=""
container="$(ot2_simulator_container)"
agent_log=""
agent_pid=""
cleanup() {
  if [[ -n "$agent_pid" ]] && kill -0 "$agent_pid" 2>/dev/null; then
    kill "$agent_pid" 2>/dev/null || true
    wait "$agent_pid" 2>/dev/null || true
  fi
  if [[ -n "$sim_pid" ]] && kill -0 "$sim_pid" 2>/dev/null; then
    kill "$sim_pid" 2>/dev/null || true
    wait "$sim_pid" 2>/dev/null || true
  fi
  docker rm -f "$container" >/dev/null 2>&1 || true
  rm -f "$agent_log" "$sim_log" "$rendered_config"
}
trap cleanup EXIT

bash "$root/.mise/scripts/ot2-simulator-setup.sh"
docker rm -f "$container" >/dev/null 2>&1 || true
bash "$root/.mise/scripts/ot2-simulator.sh" >"$sim_log" 2>&1 &
sim_pid=$!
for _ in $(seq 1 90); do
  if curl -fsS -H 'Opentrons-Version: *' http://127.0.0.1:31950/health >/dev/null 2>&1; then
    break
  fi
  sleep 2
done
curl -fsS -H 'Opentrons-Version: *' http://127.0.0.1:31950/health >/dev/null

agent_log="$(mktemp)"
A2ALAB_SILA_PORT=0 \
A2ALAB_VIDEO_DVR_ROOT="$state/video-dvr" \
A2ALAB_CONFIG="$rendered_config" \
cargo run --quiet --bin a2a-lab-ot2 -- --config "$rendered_config" serve >"$agent_log" 2>&1 &
agent_pid=$!

origin="$(media_smoke_a2a_origin)"
media_smoke_wait_card "$origin" || {
  cat "$agent_log" >&2
  exit 1
}

export A2ALAB_MEDIA_SMOKE=1
export A2ALAB_MEDIA_SMOKE_NATIVE=1
export A2ALAB_MEDIA_SMOKE_SOURCES="${A2ALAB_MEDIA_SMOKE_SOURCES:-mac-built-in}"
export A2ALAB_NATIVE_AGENT_LOG="$agent_log"
if ! bash "$root/.mise/scripts/agent-media-smoke.sh"; then
  if [[ -f "$agent_log" ]]; then
    printf 'native serve agent log tail (failure):\n' >&2
    tail -n 60 "$agent_log" >&2 || true
  fi
  exit 1
fi
