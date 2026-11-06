#!/usr/bin/env bash
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
# shellcheck source=agent-image-lib.sh
source "$root/.mise/scripts/agent-image-lib.sh"

fail() {
  printf 'agent image self-test: %s\n' "$1" >&2
  exit 1
}

assert_arg_pair() {
  local key="$1"
  local value="$2"
  shift 2
  local arg
  local expect_next=0
  for arg in "$@"; do
    if [[ "$expect_next" == 1 ]]; then
      if [[ "$arg" == "$value" ]]; then
        return 0
      fi
      expect_next=0
    elif [[ "$arg" == "$key" ]]; then
      expect_next=1
    fi
  done
  return 1
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
args=()
while IFS= read -r line; do
  args+=("$line")
done < <(build_agent_run_args "a2a-lab-ot2-agent:test-arm64" "$state")
joined="$(printf '%s\n' "${args[@]}")"
[[ "$joined" == *"A2ALAB_OPENTRONS_URL=http://robot.example:31950"* ]] || fail "robot url"
[[ "$joined" == *"31010:31000"* ]] || fail "a2a port"
[[ "$joined" == *"31011:31001"* ]] || fail "mcp port"
[[ "$joined" == *"50053:50053"* ]] || fail "sila port"
[[ "$joined" == *"31013:31003"* ]] || fail "frame set port"
assert_arg_pair --device "/dev/bus/usb/001/002:/dev/bus/usb/001/002" "${args[@]}" || fail "usb device"
assert_arg_pair -v "${state}:/var/lib/a2a-lab-ot2" "${args[@]}" || fail "state mount"
[[ "$joined" != *"--privileged"* ]] || fail "privileged"
[[ "$joined" != *"--network host"* ]] || fail "host network"
[[ "$joined" != *a2a-lab-ot2-sim* ]] || fail "simulator image"
[[ "$joined" != *"--config"* ]] || fail "unexpected config flag without A2ALAB_CONFIG"
unset A2ALAB_EXTERNAL_VIDEO_DEVICES
validate_agent_run "$state"

export AWS_PROFILE="agent-self-test-profile"
export OPENAI_API_KEY="agent-self-test-openai-key"
cred_args=()
while IFS= read -r line; do
  cred_args+=("$line")
done < <(build_agent_run_args "a2a-lab-ot2-agent:test-arm64" "$state")
assert_arg_pair -e "AWS_PROFILE=agent-self-test-profile" "${cred_args[@]}" || fail "aws profile forwarding"
assert_arg_pair -e "OPENAI_API_KEY=agent-self-test-openai-key" "${cred_args[@]}" || fail "openai key forwarding"
unset AWS_PROFILE OPENAI_API_KEY

config_dir="$tmp/config dir"
mkdir -p "$config_dir"
config_file="$config_dir/lab config.json"
cat >"$config_file" <<'EOF'
{"schema_version":1,"videos":{"live_retention":"5s","dvr":{"retention":"3d","storage_path":"/var/lib/a2a-lab-ot2/video-dvr"}}}
EOF
export A2ALAB_CONFIG="$config_file"
config_args=()
while IFS= read -r line; do
  config_args+=("$line")
done < <(build_agent_run_args "a2a-lab-ot2-agent:test-arm64" "$state")
assert_arg_pair -v "${config_file}:$(agent_container_config_path):ro" "${config_args[@]}" || fail "config mount"
assert_arg_pair --config "$(agent_container_config_path)" "${config_args[@]}" || fail "config flag"
grep -q '"live_retention":"5s"' "$config_file" || fail "live retention string"
grep -q '"retention":"3d"' "$config_file" || fail "dvr retention string"
grep -q '"/var/lib/a2a-lab-ot2/video-dvr"' "$config_file" || fail "dvr storage path"
unset A2ALAB_CONFIG

export A2ALAB_CONFIG="$tmp/missing-config.json"
if validate_agent_run "$state" >/dev/null 2>"$tmp/config.txt"; then
  fail "missing config was accepted"
fi
grep -q 'configuration file .*/missing-config.json is missing or not a regular file' "$tmp/config.txt"
unset A2ALAB_CONFIG

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
