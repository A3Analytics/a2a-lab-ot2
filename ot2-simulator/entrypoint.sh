#!/usr/bin/env bash
set -euo pipefail

mkdir -p /run/systemd/journal /var/log/journal

if [[ -x /lib/systemd/systemd-journald ]]; then
  /lib/systemd/systemd-journald >/tmp/journald.log 2>&1 &
  for _ in $(seq 1 50); do
    if journalctl -n 0 >/dev/null 2>&1; then
      break
    fi
    sleep 0.1
  done
  sleep 0.5
fi

seed() {
  local tag="$1"
  shift
  printf '%s\n' "$*" | systemd-cat -t "$tag" 2>/dev/null || logger -t "$tag" -- "$@" 2>/dev/null || true
}

seed opentrons-api "hardware_control.api: ENABLE_VIRTUAL_SMOOTHIE=true; using Smoothie emulator"
seed opentrons-api "protocol_engine: robot-server development simulator ready"
seed uvicorn "Started server process"
seed uvicorn "Uvicorn running on http://0.0.0.0:31950"
seed opentrons-api-serial "smoothie: virtual connection opened (emulator)"
seed opentrons-update-server "update_server: starting (dev robot)"
seed opentrons-update-server "no robot update in progress"

cd /opt/opentrons/robot-server
export OT_ROBOT_SERVER_DOT_ENV_PATH=/opt/opentrons/robot-server/dev.env
export ENABLE_VIRTUAL_SMOOTHIE=true

if command -v systemd-cat >/dev/null 2>&1; then
  exec systemd-cat -t uvicorn --stderr-priority=err \
    uv run --frozen --python /usr/local/bin/python3 \
    uvicorn robot_server.app:app --host 0.0.0.0 --port 31950 --ws wsproto
fi

exec uv run --frozen --python /usr/local/bin/python3 \
  uvicorn robot_server.app:app --host 0.0.0.0 --port 31950 --ws wsproto
