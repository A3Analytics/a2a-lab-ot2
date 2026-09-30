# shellcheck shell=bash
# Shared Docker names for the pinned OT-2 robot-server image.

ot2_simulator_tag() {
  printf '%s\n' "${OPENTRONS_TAG:-v10.0.0}"
}

ot2_simulator_image() {
  printf 'a2a-lab-ot2-sim:%s\n' "$(ot2_simulator_tag)"
}

ot2_simulator_container() {
  printf 'a2a-lab-ot2-sim\n'
}

require_docker() {
  if ! command -v docker >/dev/null 2>&1; then
    printf 'Docker is required for the OT-2 simulator. Install Docker Desktop and retry.\n' >&2
    exit 1
  fi
  if ! docker info >/dev/null 2>&1; then
    printf 'Docker daemon is not running. Start Docker Desktop and retry.\n' >&2
    exit 1
  fi
}

refuse_moving_tag() {
  local tag
  tag="$(ot2_simulator_tag)"
  if [[ "$tag" == "edge" || "$tag" == "latest" || "$tag" == main || "$tag" == master ]]; then
    printf 'refusing moving ref %s; pin a versioned release tag\n' "$tag" >&2
    exit 1
  fi
}
