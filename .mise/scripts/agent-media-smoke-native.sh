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

if [[ -z "${A2ALAB_EXTERNAL_CAMERA_INDEX:-}" ]]; then
  media_smoke_diagnose "external camera" "set A2ALAB_EXTERNAL_CAMERA_INDEX for native smoke"
  exit 1
fi

state="${A2ALAB_STATE_DIR:-$root/.a2a-lab-ot2-native-smoke}"
mkdir -p "$state"
export A2ALAB_STATE_DIR="$state"

sim_log="$(mktemp)"
sim_pid=""
container="$(ot2_simulator_container)"
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
  rm -f "$agent_log" "$sim_log"
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
agent_pid=""
A2ALAB_SILA_PORT=0 \
A2ALAB_VIDEO_DVR_ROOT="$state/video-dvr" \
cargo run --quiet --bin a2a-lab-ot2 -- serve >"$agent_log" 2>&1 &
agent_pid=$!

origin="$(media_smoke_a2a_origin)"
media_smoke_wait_card "$origin" || {
  cat "$agent_log" >&2
  exit 1
}

export A2ALAB_MEDIA_SMOKE=1
export A2ALAB_MEDIA_SMOKE_NATIVE=1
export A2ALAB_MEDIA_SMOKE_SOURCES="${A2ALAB_MEDIA_SMOKE_SOURCES:-external-camera}"
bash "$root/.mise/scripts/agent-media-smoke.sh"
