#!/usr/bin/env bash
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
cd "$root"

step() {
  local name="$1"
  shift
  printf 'processing: %s\n' "$name"
  SECONDS=0
  "$@"
  printf 'processing: %s - finished in: (%ds)\n' "$name" "$SECONDS"
}

check_warnings() {
  CARGO_TARGET_DIR="$root/target/quality" RUSTFLAGS="-D warnings" cargo check --workspace --quiet
}

clippy_warnings() {
  CARGO_TARGET_DIR="$root/target/quality" cargo clippy --all-targets --quiet -- -D warnings
}

step 'cargo fmt' cargo fmt --check
step check check_warnings
step clippy clippy_warnings
step nextest cargo nextest run --workspace
