#!/usr/bin/env bash
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
# shellcheck source=agent-image-lib.sh
source "$root/.mise/scripts/agent-image-lib.sh"
# shellcheck source=agent-media-smoke-lib.sh
source "$root/.mise/scripts/agent-media-smoke-lib.sh"

fail() {
  printf 'agent media smoke self-test: %s\n' "$1" >&2
  exit 1
}

mkdir -p "$root/target"
if bash "$root/.mise/scripts/agent-media-smoke.sh" >"$root/target/agent-media-smoke-skip.txt" 2>&1; then
  :
else
  fail "skip path should succeed"
fi
grep -q 'skipping dual-source media smoke' "$root/target/agent-media-smoke-skip.txt" || fail "skip message"

state="$(mktemp -d)"
export A2ALAB_EXTERNAL_CAMERA_INDEX=2
export A2ALAB_VIDEO_RETENTION_SECS=5
args="$(build_agent_run_args "a2a-lab-ot2-agent:test-arm64" "$state")"
[[ "$args" == *"A2ALAB_EXTERNAL_CAMERA_INDEX=2"* ]] || fail "external index env"
[[ "$args" == *"A2ALAB_VIDEO_RETENTION_SECS=5"* ]] || fail "video retention env"

rollback="$(print_agent_rollback_instructions "$state")"
[[ "$rollback" == *"Do not delete images.sqlite"* ]] || fail "rollback text"
[[ "$rollback" == *"$state"* ]] || fail "rollback state path"

policy="$(agent_retention_policy_summary)"
[[ "$policy" == *"259200"* ]] || fail "disk retention policy"
[[ "$policy" == *"30 days"* ]] || fail "still age policy"

native_policy="$(agent_native_media_smoke_retention_summary)"
[[ "$native_policy" == *"10800"* ]] || fail "native disk retention policy"
[[ "$native_policy" == *"mac-built-in"* ]] || fail "native custom source id"

fixture="$root/fixtures/native-macos-media-smoke-v1.json"
[[ -f "$fixture" ]] || fail "native macOS smoke fixture"
grep -q '"retention": "3h"' "$fixture" || fail "native fixture dvr retention"
grep -q '"id": "mac-built-in"' "$fixture" || fail "native fixture built-in source"
if grep -q '"id": "usb-webcam"' "$fixture"; then
  fail "native fixture must not open the usb webcam"
fi

printf 'agent media smoke self-test ok\n'
