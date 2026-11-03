#!/usr/bin/env bash
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
# shellcheck source=agent-image-lib.sh
source "$root/.mise/scripts/agent-image-lib.sh"

require_docker
docker rm -f "$(agent_container_name)" >/dev/null 2>&1 || true
printf 'removed %s if it was running\n' "$(agent_container_name)"
