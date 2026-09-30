#!/usr/bin/env bash
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
cd "$root"

sim_pid=""

cleanup() {
  if [[ -n "$sim_pid" ]] && kill -0 "$sim_pid" 2>/dev/null; then
    kill "$sim_pid" 2>/dev/null || true
    wait "$sim_pid" 2>/dev/null || true
  fi
}
trap cleanup EXIT INT TERM

bash "$root/.mise/scripts/simulator-setup.sh"
bash "$root/.mise/scripts/simulator.sh" &
sim_pid=$!

for _ in $(seq 1 90); do
  if curl -fsS -H 'Opentrons-Version: *' http://127.0.0.1:31950/health >/dev/null 2>&1; then
    break
  fi
  if ! kill -0 "$sim_pid" 2>/dev/null; then
    printf 'robot-server exited before becoming healthy\n' >&2
    exit 1
  fi
  sleep 2
done
curl -fsS -H 'Opentrons-Version: *' http://127.0.0.1:31950/health >/dev/null

printf 'simulator  http://127.0.0.1:31950\n'
printf 'A2A        http://127.0.0.1:31000\n'
printf 'MCP        http://127.0.0.1:31001/mcp\n'
printf 'in another terminal: mise run ot2-simulation && mise run a2a-lab-example\n'

cargo run --bin a2a-lab-sdk-example
