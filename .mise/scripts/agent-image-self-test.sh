#!/usr/bin/env bash
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
# shellcheck source=agent-image-lib.sh
source "$root/.mise/scripts/agent-image-lib.sh"

fail() {
  printf 'agent image self-test: %s\n' "$1" >&2
  exit 1
}

tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT

normalize_architecture amd64 >/dev/null
normalize_architecture arm64 >/dev/null
if normalize_architecture riscv64 >/dev/null 2>"$tmp/arch.txt"; then
  fail "riscv64 was accepted"
fi
grep -q 'unsupported architecture riscv64; expected amd64 or arm64' "$tmp/arch.txt"

state="$(mktemp -d)"
export A2ALAB_OPENTRONS_URL="http://robot.example:31950"
export A2ALAB_A2A_PORT=31010
export A2ALAB_MCP_PORT=31011
export A2ALAB_SILA_PORT=50053
export A2ALAB_FRAME_SET_PORT=31013
export A2ALAB_EXTERNAL_VIDEO_DEVICES="/dev/bus/usb/001/002"
args="$(build_agent_run_args "a2a-lab-ot2-agent:test-arm64" "$state")"
[[ "$args" == *"A2ALAB_OPENTRONS_URL=http://robot.example:31950"* ]] || fail "robot url"
[[ "$args" == *"31010:31000"* ]] || fail "a2a port"
[[ "$args" == *"31011:31001"* ]] || fail "mcp port"
[[ "$args" == *"50053:50053"* ]] || fail "sila port"
[[ "$args" == *"31013:31003"* ]] || fail "frame set port"
[[ "$args" == *"/dev/bus/usb/001/002:/dev/bus/usb/001/002"* ]] || fail "usb device"
[[ "$args" == *"${state}:/var/lib/a2a-lab-ot2"* ]] || fail "state mount"
[[ "$args" != *"--privileged"* ]] || fail "privileged"
[[ "$args" != *"--network host"* ]] || fail "host network"
[[ "$args" != *a2a-lab-ot2-sim* ]] || fail "simulator image"
unset A2ALAB_EXTERNAL_VIDEO_DEVICES
validate_agent_run "$state"

export A2ALAB_EXTERNAL_VIDEO_DEVICES="/tmp/a2a-lab-ot2-missing-usb-node"
if validate_agent_run "$state" >/dev/null 2>"$tmp/usb.txt"; then
  fail "missing usb device was accepted"
fi
grep -q 'USB device /tmp/a2a-lab-ot2-missing-usb-node is not accessible' "$tmp/usb.txt"
grep -q 'Do not use --privileged' "$tmp/usb.txt"
unset A2ALAB_EXTERNAL_VIDEO_DEVICES

if validate_agent_run "$tmp/file" >/dev/null 2>"$tmp/state.txt"; then
  fail "unwritable state was accepted"
fi
grep -q 'is not writable' "$tmp/state.txt"

simulator="$(cat "$root/ot2-simulator/Dockerfile")"
[[ "$simulator" == *"v10.0.0"* ]] || fail "simulator pin"
if printf '%s' "$simulator" | grep -qi 'orbbec'; then
  fail "simulator dockerfile mentions a vendor camera sdk"
fi

printf 'agent image self-test ok\n'
