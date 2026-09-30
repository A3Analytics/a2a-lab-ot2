#!/usr/bin/env bash
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
tag="${OPENTRONS_TAG:-v10.0.0}"
cache="$root/.cache/opentrons"

bash "$root/.mise/scripts/simulator-setup.sh"

actual="$(git -C "$cache" describe --tags --exact-match)"
if [[ "$actual" != "$tag" ]]; then
  printf 'expected tag %s, checkout is %s\n' "$tag" "$actual" >&2
  exit 1
fi

cd "$cache/robot-server"
export OT_ROBOT_SERVER_DOT_ENV_PATH="$cache/robot-server/dev.env"
exec uv run --frozen --python 3.12 uvicorn robot_server.app:app --host 127.0.0.1 --port 31950 --ws wsproto
