#!/usr/bin/env bash
set -euo pipefail

if [[ "${A2ALAB_ORBBEC_LIVE:-}" != 1 ]]; then
  printf 'skipping live Orbbec check; set A2ALAB_ORBBEC_LIVE=1 with A2ALAB_ORBBEC_USB_DEVICES to capture one JPEG. Frame sets are served at :31003/ot2/frame-sets/ui when the agent is running.\n'
  exit 0
fi

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
# shellcheck source=agent-image-lib.sh
source "$root/.mise/scripts/agent-image-lib.sh"

require_docker
if [[ -z "${A2ALAB_ORBBEC_USB_DEVICES:-}" ]]; then
  printf 'A2ALAB_ORBBEC_USB_DEVICES must name the Orbbec USB device node\n' >&2
  exit 1
fi
arch="${A2ALAB_AGENT_ARCH:-$(docker info -f '{{.Architecture}}')}"
arch="$(normalize_architecture "$arch")"
image="$(agent_image_name "$arch")"
state="${A2ALAB_STATE_DIR:-$root/.a2a-lab-ot2-agent}"
mkdir -p "$state"
out="$(mktemp -d)"
validate_agent_run "$state"
args=()
while IFS= read -r line; do
  args+=("$line")
done < <(build_agent_run_args "$image" "$state")
# Drop the container name so two sequential runs do not collide, and add the output mount.
filtered=()
skip_name=0
for arg in "${args[@]}"; do
  if [[ "$skip_name" == 1 ]]; then
    skip_name=0
    continue
  fi
  if [[ "$arg" == "--name" ]]; then
    skip_name=1
    continue
  fi
  if [[ "$arg" == "$image" ]]; then
    filtered+=(-e A2ALAB_ORBBEC_CAMERA=true -v "${out}:/out" "$image")
    continue
  fi
  filtered+=("$arg")
done

capture="$(docker "${filtered[@]}" get-current-image --source orbbec-camera --output /out/frame.jpg)"
printf '%s\n' "$capture"
id="$(printf '%s\n' "$capture" | awk -F '\t' '$1 == "id" { print $2 }')"
media="$(printf '%s\n' "$capture" | awk -F '\t' '$1 == "media_type" { print $2 }')"
width="$(printf '%s\n' "$capture" | awk -F '\t' '$1 == "width" { print $2 }')"
height="$(printf '%s\n' "$capture" | awk -F '\t' '$1 == "height" { print $2 }')"
bytes="$(printf '%s\n' "$capture" | awk -F '\t' '$1 == "bytes" { print $2 }')"
[[ -n "$id" && "$media" == "image/jpeg" && "$width" != "0" && "$height" != "0" && "$bytes" != "0" ]] || {
  printf 'live Orbbec capture did not return one JPEG\n' >&2
  exit 1
}
docker "${filtered[@]}" get-image --id "$id" --output /out/stored.jpg >/dev/null
cmp /out/frame.jpg /out/stored.jpg
printf 'live Orbbec JPEG %s matches the stored frame\n' "$id"
rm -rf "$out"
