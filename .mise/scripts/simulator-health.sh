#!/usr/bin/env bash
set -euo pipefail

url="${OPENTRONS_URL:-http://127.0.0.1:31950}"
curl -fsS -H 'Opentrons-Version: *' "$url/health"
printf '\n'
