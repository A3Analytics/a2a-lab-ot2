#!/usr/bin/env bash
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
cd "$root"

# Lists image sources, or saves one current JPEG.
# Set A2ALAB_EXTERNAL_CAMERA_INDEX to add external-camera.
# A2ALAB_EXTERNAL_CAMERA_DESCRIPTION replaces its advertised description.

if [[ $# -eq 0 ]]; then
  exec cargo run --quiet --bin a2a-lab-ot2 -- list-image-sources
fi

if [[ $# -ne 2 ]]; then
  printf 'usage: mise run images -- [source output]\n' >&2
  exit 2
fi

exec cargo run --quiet --bin a2a-lab-ot2 -- get-current-image --source "$1" --output "$2"
