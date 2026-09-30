#!/usr/bin/env bash
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
cd "$root"

tag="${OPENTRONS_TAG:-v10.0.0}"
cache="$root/.cache/opentrons"
server_log="$(mktemp)"
agent_log="$(mktemp)"
sim_pid=""
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
  rm -f "$server_log" "$agent_log"
}
trap cleanup EXIT

bash "$root/.mise/scripts/simulator-setup.sh"
actual="$(git -C "$cache" describe --tags --exact-match)"
if [[ "$actual" != "$tag" ]]; then
  printf 'expected tag %s, checkout is %s\n' "$tag" "$actual" >&2
  exit 1
fi

bash "$root/.mise/scripts/simulator.sh" >"$server_log" 2>&1 &
sim_pid=$!

for _ in $(seq 1 90); do
  if curl -fsS -H 'Opentrons-Version: *' http://127.0.0.1:31950/health >/dev/null 2>&1; then
    break
  fi
  if ! kill -0 "$sim_pid" 2>/dev/null; then
    cat "$server_log" >&2
    exit 1
  fi
  sleep 2
done
curl -fsS -H 'Opentrons-Version: *' http://127.0.0.1:31950/health >/dev/null

cargo run --quiet --bin a2a-lab-sdk-example >"$agent_log" 2>&1 &
agent_pid=$!

for _ in $(seq 1 30); do
  if curl -fsS http://127.0.0.1:31000/.well-known/agent-card.json >/dev/null 2>&1; then
    break
  fi
  if ! kill -0 "$agent_pid" 2>/dev/null; then
    cat "$agent_log" >&2
    exit 1
  fi
  sleep 1
done
curl -fsS http://127.0.0.1:31000/.well-known/agent-card.json >/dev/null

cargo run --quiet --bin a2a-lab -- list-workflows
cargo run --quiet --bin a2a-lab -- list-log-sources
cargo run --quiet --bin a2a-lab -- list-metrics
run_out="$(cargo run --quiet --bin a2a-lab -- start-workflow)"
printf '%s\n' "$run_out"
run_id="$(printf '%s\n' "$run_out" | awk '/^run_id / { print $2; exit }')"
if [[ -z "$run_id" ]]; then
  printf 'a2a-lab start-workflow did not print a run_id\n' >&2
  exit 1
fi
cargo run --quiet --bin a2a-lab -- get-workflow-status "$run_id"
cargo run --quiet --bin a2a-lab -- pause "$run_id" || true
cargo run --quiet --bin a2a-lab -- resume "$run_id"
cargo run --quiet --bin a2a-lab -- pause "$run_id" || true
cargo run --quiet --bin a2a-lab -- stop "$run_id"
cargo run --quiet --bin a2a-lab -- home
cargo run --quiet --bin a2a-lab -- query-logs
cargo run --quiet --bin a2a-lab -- query-metrics
curl -fsS -H 'Opentrons-Version: *' http://127.0.0.1:31950/health >/dev/null
printf 'smoke ok\n'
