#!/usr/bin/env bash
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
# shellcheck source=ot2-simulator-docker.sh
source "$root/.mise/scripts/ot2-simulator-docker.sh"

require_docker

container="$(ot2_simulator_container)"
image="$(ot2_simulator_image)"

docker rm -f "$container" >/dev/null 2>&1 || true
docker rmi "$image" >/dev/null 2>&1 || true
rm -rf "$root/.cache/opentrons"
printf 'removed %s and leftover host checkout\n' "$image"
