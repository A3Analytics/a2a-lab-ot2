#!/usr/bin/env bash
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
# shellcheck source=ot2-simulator-docker.sh
source "$root/.mise/scripts/ot2-simulator-docker.sh"

require_docker
refuse_moving_tag

tag="$(ot2_simulator_tag)"
image="$(ot2_simulator_image)"

docker build \
  --build-arg "OPENTRONS_TAG=${tag}" \
  -t "$image" \
  -f "$root/ot2-simulator/Dockerfile" \
  "$root/ot2-simulator"

printf 'built %s\n' "$image"
