#!/usr/bin/env bash
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
# shellcheck source=ot2-simulator-docker.sh
source "$root/.mise/scripts/ot2-simulator-docker.sh"

require_docker
refuse_moving_tag

image="$(ot2_simulator_image)"
container="$(ot2_simulator_container)"

if ! docker image inspect "$image" >/dev/null 2>&1; then
  bash "$root/.mise/scripts/ot2-simulator-setup.sh"
fi

docker rm -f "$container" >/dev/null 2>&1 || true
exec docker run --rm --init --name "$container" \
  -p 127.0.0.1:31950:31950 \
  "$image"
