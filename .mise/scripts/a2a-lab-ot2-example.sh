#!/usr/bin/env bash
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
cd "$root"

ot_url="${A2ALAB_OPENTRONS_URL:-http://127.0.0.1:31950}"

if ! curl -fsS -H 'Opentrons-Version: *' "$ot_url/health" >/dev/null 2>&1; then
  printf 'OT-2 simulator is not running at %s. Start it with: mise run ot2-simulator\n' "$ot_url" >&2
  exit 1
fi

cargo run --quiet --bin a2a-lab-ot2 -- --opentrons-url "$ot_url" query-logs
