#!/usr/bin/env bash
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
cd "$root"

a2a_url="${A2A_URL:-http://127.0.0.1:31000}"
ot_url="${OPENTRONS_URL:-http://127.0.0.1:31950}"

if ! curl -fsS "$a2a_url/.well-known/agent-card.json" >/dev/null 2>&1; then
  printf 'A2A agent is not running at %s. Start it with: mise run start\n' "$a2a_url" >&2
  exit 1
fi
if ! curl -fsS -H 'Opentrons-Version: *' "$ot_url/health" >/dev/null 2>&1; then
  printf 'OT-2 simulator is not running at %s. Start it with: mise run start\n' "$ot_url" >&2
  exit 1
fi

cargo run --quiet --bin a2a-lab -- --a2a "$a2a_url" query-logs
