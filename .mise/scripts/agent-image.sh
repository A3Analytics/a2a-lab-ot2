#!/usr/bin/env bash
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
# shellcheck source=agent-image-lib.sh
source "$root/.mise/scripts/agent-image-lib.sh"

usage() {
  printf 'usage: agent-image.sh build|verify [amd64|arm64|all]\n' >&2
  exit 2
}

host_arch() {
  local raw
  raw="$(docker info -f '{{.Architecture}}')"
  normalize_architecture "$raw"
}

compile_binary() {
  local arch="$1"
  local host="$2"
  local target packages
  case "$arch" in
    amd64) target="x86_64-unknown-linux-gnu" ;;
    arm64) target="aarch64-unknown-linux-gnu" ;;
  esac
  packages="clang libclang-dev pkg-config git"
  local -a env_args=()
  if [[ "$arch" != "$host" ]]; then
    case "$arch" in
      amd64)
        packages="$packages gcc-x86-64-linux-gnu g++-x86-64-linux-gnu"
        env_args+=(
          -e CARGO_TARGET_X86_64_UNKNOWN_LINUX_GNU_LINKER=x86_64-linux-gnu-gcc
          -e CC_x86_64_unknown_linux_gnu=x86_64-linux-gnu-gcc
          -e CXX_x86_64_unknown_linux_gnu=x86_64-linux-gnu-g++
          -e AR_x86_64_unknown_linux_gnu=x86_64-linux-gnu-ar
        )
        ;;
      arm64)
        packages="$packages gcc-aarch64-linux-gnu g++-aarch64-linux-gnu"
        env_args+=(
          -e CARGO_TARGET_AARCH64_UNKNOWN_LINUX_GNU_LINKER=aarch64-linux-gnu-gcc
          -e CC_aarch64_unknown_linux_gnu=aarch64-linux-gnu-gcc
          -e CXX_aarch64_unknown_linux_gnu=aarch64-linux-gnu-g++
          -e AR_aarch64_unknown_linux_gnu=aarch64-linux-gnu-ar
        )
        ;;
    esac
  fi
  local cargo_dir="/work/target/agent-${arch}"
  local -a docker_args=(
    run --platform "linux/${host}" --rm
    -e HOST_UID="$(id -u)"
    -e HOST_GID="$(id -g)"
    -e "CARGO_TARGET_DIR=${cargo_dir}"
  )
  if [[ ${#env_args[@]} -gt 0 ]]; then
    docker_args+=("${env_args[@]}")
  fi
  docker_args+=(
    -v "$root:/work"
    -w /work
    rust:1.98.1
    sh -c "
      set -eu
      apt-get update >/dev/null
      apt-get install -y --no-install-recommends $packages >/dev/null
      rustup target add $target wasm32-unknown-unknown
      cargo build --release --locked --target $target --bin a2a-lab-ot2
      status=\$?
      chown -R \"\$HOST_UID:\$HOST_GID\" \"$cargo_dir\" 2>/dev/null || true
      exit \$status
    "
  )
  docker "${docker_args[@]}"
  cp "$root/target/agent-${arch}/${target}/release/a2a-lab-ot2" "$3"
}

build_one() {
  local arch
  arch="$(normalize_architecture "$1")"
  require_docker
  local host
  host="$(host_arch)"
  local context="$root/agent/context/${arch}"
  rm -rf "$context"
  mkdir -p "$context"
  compile_binary "$arch" "$host" "$context/a2a-lab-ot2"
  cp "$root/agent/Dockerfile" "$context/Dockerfile"
  cp "$root/agent/orbbec-sdk.env" "$context/orbbec-sdk.env"
  local image
  image="$(agent_image_name "$arch")"
  if ! docker build --platform "linux/${arch}" -t "$image" "$context"; then
    printf 'agent image build failed for %s. Check the SDK download, checksum, shared library, and application startup.\n' "$arch" >&2
    return 1
  fi
  printf 'built %s\n' "$image"
}

verify_one() {
  local arch
  arch="$(normalize_architecture "$1")"
  require_docker
  local image
  image="$(agent_image_name "$arch")"
  if ! docker image inspect "$image" >/dev/null 2>&1; then
    printf 'missing agent image %s. Run the agent-image build first.\n' "$image" >&2
    return 1
  fi
  local entry
  entry="$(docker image inspect --format '{{json .Config.Entrypoint}}' "$image")"
  [[ "$entry" == '["/usr/local/bin/a2a-lab-ot2"]' ]] || {
    printf 'agent image %s entrypoint is %s, expected the a2a-lab-ot2 serve binary\n' "$image" "$entry" >&2
    return 1
  }
  docker run --rm --platform "linux/${arch}" --entrypoint /bin/sh "$image" -c '
    test ! -e /usr/bin/gcc
    test ! -d /opt/orbbec/examples
    test ! -e /usr/local/bin/OrbbecViewer
    test ! -e /opt/opentrons
    test -f /usr/share/doc/orbbec/LICENSE.txt
    test -d /usr/share/doc/orbbec/licenses
    test -f /opt/orbbec/lib/libOrbbecSDK.so.2.10.6
  '
  local version
  if ! version="$(docker run --rm --platform "linux/${arch}" "$image" orbbec-sdk-version)"; then
    printf 'Orbbec shared library could not be loaded in %s\n' "$image" >&2
    return 1
  fi
  [[ "$version" == "2.10.6" ]] || {
    printf 'agent image %s reported Orbbec SDK %s, expected 2.10.6\n' "$image" "$version" >&2
    return 1
  }
  docker run --rm --platform "linux/${arch}" --entrypoint /usr/local/bin/a2a-lab-ot2 "$image" --version >/dev/null
  local startup
  startup="$(docker run --rm --platform "linux/${arch}" -e A2ALAB_OPENTRONS_URL=http://127.0.0.1:9 "$image" 2>&1 || true)"
  [[ "$startup" == *"opentrons-camera"* && "$startup" == *"orbbec-camera off"* ]] || {
    printf 'agent image %s did not start the serve path:\n%s\n' "$image" "$startup" >&2
    return 1
  }
  local capture
  if capture="$(docker run --rm --platform "linux/${arch}" -e A2ALAB_ORBBEC_CAMERA=true "$image" get-current-image --source orbbec-camera --output /var/lib/a2a-lab-ot2/frame.jpg 2>&1)"; then
    printf 'agent image %s captured a frame without a camera:\n%s\n' "$image" "$capture" >&2
    return 1
  fi
  [[ "$capture" == *"orbbec-camera"* && "$capture" == *"no Orbbec device"* ]] || {
    printf 'agent image %s capture failure was not actionable:\n%s\n' "$image" "$capture" >&2
    return 1
  }
  printf 'verified %s\n' "$image"
}

command="${1:-}"
arch="${2:-}"
case "$command" in
  build | verify) ;;
  *) usage ;;
esac
if [[ -z "$arch" ]]; then
  require_docker
  arch="$(host_arch)"
fi
if [[ "$arch" == all ]]; then
  targets=(amd64 arm64)
else
  targets=("$(normalize_architecture "$arch")")
fi
for target in "${targets[@]}"; do
  if [[ "$command" == build ]]; then
    build_one "$target"
    verify_one "$target"
  else
    verify_one "$target"
  fi
done
