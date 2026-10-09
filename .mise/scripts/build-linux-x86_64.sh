#!/usr/bin/env bash
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
workspace="$(dirname "$root")"
repo="$(basename "$root")"
target="/work/$repo/target/docker-x86_64"

docker run --platform linux/arm64 --rm \
  -e HOST_UID="$(id -u)" \
  -e HOST_GID="$(id -g)" \
  -e CARGO_TARGET_DIR="$target" \
  -e CARGO_TARGET_X86_64_UNKNOWN_LINUX_GNU_LINKER=x86_64-linux-gnu-gcc \
  -e CC_x86_64_unknown_linux_gnu=x86_64-linux-gnu-gcc \
  -e CXX_x86_64_unknown_linux_gnu=x86_64-linux-gnu-g++ \
  -e AR_x86_64_unknown_linux_gnu=x86_64-linux-gnu-ar \
  -v "$workspace:/work" \
  -w "/work/$repo" \
  rust:1.98.1 sh -c '
    apt-get update >/dev/null &&
    apt-get install -y --no-install-recommends \
      clang libclang-dev gcc-x86-64-linux-gnu g++-x86-64-linux-gnu >/dev/null &&
    /usr/local/cargo/bin/rustup target add x86_64-unknown-linux-gnu &&
    /usr/local/cargo/bin/cargo build --release --locked \
      --target x86_64-unknown-linux-gnu --bin a2a-lab-ot2
    status=$?
    chown -R "$HOST_UID:$HOST_GID" "$CARGO_TARGET_DIR" 2>/dev/null || true
    exit "$status"
  '
