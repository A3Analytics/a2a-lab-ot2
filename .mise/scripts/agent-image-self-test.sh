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

amd64="$(sdk_url_for amd64)"
arm64="$(sdk_url_for arm64)"
[[ "$amd64" == *linux_x86_64.tar.gz* ]] || fail "amd64 url"
[[ "$arm64" == *linux_arm64.tar.gz* ]] || fail "arm64 url"
[[ "$amd64" == *23ec9f7023881cc9061bf0b4ba65735fb16c10e168fd5d62b1548c847e7ddb46 ]] || fail "amd64 checksum"
[[ "$arm64" == *00fafc2b566ca98feb3ef0242143d830209b09bac5d41c557cf6ec40452c8df9 ]] || fail "arm64 checksum"
if accept_sdk_url "https://github.com/orbbec/OrbbecSDK_v2/archive/refs/heads/main.tar.gz" >/dev/null 2>&1; then
  fail "moving ref was accepted"
fi

printf 'nope' >"$tmp/file"
if verify_sha256 "$tmp/file" "0000000000000000000000000000000000000000000000000000000000000000" >/dev/null 2>&1; then
  fail "bad checksum was accepted"
fi
verify_sha256 "$tmp/file" "$(sha256_file "$tmp/file")"

state="$(mktemp -d)"
export A2ALAB_OPENTRONS_URL="http://robot.example:31950"
export A2ALAB_A2A_PORT=31010
export A2ALAB_MCP_PORT=31011
export A2ALAB_SILA_PORT=50053
export A2ALAB_FRAME_SET_PORT=31013
export A2ALAB_ORBBEC_USB_DEVICES="/dev/bus/usb/001/002"
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
unset A2ALAB_ORBBEC_USB_DEVICES
validate_agent_run "$state"

export A2ALAB_ORBBEC_USB_DEVICES="/tmp/a2a-lab-ot2-missing-usb-node"
if validate_agent_run "$state" >/dev/null 2>"$tmp/usb.txt"; then
  fail "missing usb device was accepted"
fi
grep -q 'USB device /tmp/a2a-lab-ot2-missing-usb-node is not accessible' "$tmp/usb.txt"
grep -q 'Do not use --privileged' "$tmp/usb.txt"
unset A2ALAB_ORBBEC_USB_DEVICES

if validate_agent_run "$tmp/file" >/dev/null 2>"$tmp/state.txt"; then
  fail "unwritable state was accepted"
fi
grep -q 'is not writable' "$tmp/state.txt"

simulator="$(cat "$root/ot2-simulator/Dockerfile")"
[[ "$simulator" == *"v10.0.0"* ]] || fail "simulator pin"
if printf '%s' "$simulator" | grep -qi orbbec; then
  fail "simulator dockerfile mentions orbbec"
fi

printf 'agent image self-test ok\n'
