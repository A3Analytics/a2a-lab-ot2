#!/usr/bin/env bash
set -euo pipefail
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
cd "$ROOT"

if [[ "${A2ALAB_JEV_SMOKE:-}" != "1" ]]; then
  echo "Skipped: set A2ALAB_JEV_SMOKE=1 with TypeSafe credentials and a retained frame id."
  echo "Example: A2ALAB_JEV_SMOKE=1 A2ALAB_JEV_LIVE=1 mise run jev-decision-smoke -- \\"
  echo "  --config /path/to/a2a-lab-v2-semantic-valid.json \\"
  echo "  --frame-id <manifest-frame-id>"
  exit 0
fi

CONFIG="${A2ALAB_JEV_CONFIG:-}"
FRAME_ID="${A2ALAB_JEV_SMOKE_FRAME_ID:-}"
if [[ -z "$CONFIG" || -z "$FRAME_ID" ]]; then
  echo "Set A2ALAB_JEV_CONFIG and A2ALAB_JEV_SMOKE_FRAME_ID (or pass CLI flags via mise run a2a-lab-ot2 --)."
  exit 1
fi

EVAL_ID="${A2ALAB_JEV_SMOKE_EVAL_ID:-jev-smoke}"

exec cargo run --bin a2a-lab-ot2 -- jev-decision-smoke \
  --config "$CONFIG" \
  --frame-id "$FRAME_ID" \
  --evaluation-id "$EVAL_ID" \
  "${@}"
