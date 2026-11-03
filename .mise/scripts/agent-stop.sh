#!/usr/bin/env bash
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
# shellcheck source=agent-image-lib.sh
source "$root/.mise/scripts/agent-image-lib.sh"

require_docker
name="$(agent_container_name)"
if docker inspect "$name" >/dev/null 2>&1; then
  docker stop --time 30 "$name" >/dev/null 2>&1 || true
fi
docker rm -f "$name" >/dev/null 2>&1 || true
printf 'stopped and removed %s if it was running; retained data stays on the A2ALAB_STATE_DIR mount\n' "$name"
print_agent_rollback_instructions "$(agent_default_state_dir)"
