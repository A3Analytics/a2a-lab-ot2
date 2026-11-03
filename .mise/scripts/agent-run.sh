#!/usr/bin/env bash
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
# shellcheck source=agent-image-lib.sh
source "$root/.mise/scripts/agent-image-lib.sh"

require_docker
arch="${A2ALAB_AGENT_ARCH:-$(docker info -f '{{.Architecture}}')}"
arch="$(normalize_architecture "$arch")"
image="$(agent_image_name "$arch")"
if ! docker image inspect "$image" >/dev/null 2>&1; then
  printf 'missing agent image %s. Build it with mise run agent-image -- %s\n' "$image" "$arch" >&2
  exit 1
fi
state="${A2ALAB_STATE_DIR:-$root/.a2a-lab-ot2-agent}"
mkdir -p "$state"
validate_agent_run "$state"
args=()
while IFS= read -r line; do
  args+=("$line")
done < <(build_agent_run_args "$image" "$state")
exec docker "${args[@]}"
