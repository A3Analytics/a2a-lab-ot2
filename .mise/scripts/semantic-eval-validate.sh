#!/usr/bin/env bash
set -euo pipefail
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
cd "$ROOT"

echo "Running deterministic semantic evaluation manifest validation (read-only DVR fixture)..."
cargo nextest run -E 'test(semantic_eval_validate_deterministic_fixture)'

TEMPLATE="$ROOT/fixtures/semantic_eval/chupacabra-pilot-template-v1.json"
if [[ -f "$TEMPLATE" ]]; then
  echo "Pilot template present at $TEMPLATE (populate retained_sources via semantic-eval discover on chupacabra)."
fi
