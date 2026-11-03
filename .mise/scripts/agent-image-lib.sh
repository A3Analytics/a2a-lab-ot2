# shellcheck shell=bash
# Shared helpers for the Orbbec-enabled agent image. These functions do not start Docker.

agent_root() {
  cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd
}

agent_version() {
  sed -n 's/^version = "\(.*\)"/\1/p' "$(agent_root)/Cargo.toml" | head -n 1
}

agent_image_name() {
  local arch="$1"
  printf 'a2a-lab-ot2-agent:%s-%s\n' "$(agent_version)" "$arch"
}

agent_container_name() {
  printf 'a2a-lab-ot2-agent\n'
}

normalize_architecture() {
  case "$1" in
    amd64 | x86_64 | linux/amd64) printf 'amd64\n' ;;
    arm64 | aarch64 | linux/arm64) printf 'arm64\n' ;;
    *)
      printf 'unsupported architecture %s; expected amd64 or arm64\n' "$1" >&2
      return 1
      ;;
  esac
}

sha256_file() {
  if command -v sha256sum >/dev/null 2>&1; then
    sha256sum "$1" | awk '{print $1}'
  else
    shasum -a 256 "$1" | awk '{print $1}'
  fi
}

verify_sha256() {
  local file="$1"
  local expected="$2"
  local actual
  actual="$(sha256_file "$file")"
  if [[ "$actual" != "$expected" ]]; then
    printf 'Orbbec SDK checksum failure for %s: expected %s, got %s\n' "$file" "$expected" "$actual" >&2
    return 1
  fi
}

accept_sdk_url() {
  case "$1" in
    https://github.com/orbbec/OrbbecSDK_v2/releases/download/v2.10.6/OrbbecSDK_v2.10.6_*_linux_*.tar.gz) return 0 ;;
    *)
      printf 'refusing unpinned Orbbec SDK download %s\n' "$1" >&2
      return 1
      ;;
  esac
}

sdk_url_for() {
  local arch
  arch="$(normalize_architecture "$1")" || return 1
  # shellcheck disable=SC1091
  source "$(agent_root)/agent/orbbec-sdk.env"
  local url sha
  case "$arch" in
    amd64) url="$ORBBEC_SDK_AMD64_URL"; sha="$ORBBEC_SDK_AMD64_SHA256" ;;
    arm64) url="$ORBBEC_SDK_ARM64_URL"; sha="$ORBBEC_SDK_ARM64_SHA256" ;;
  esac
  accept_sdk_url "$url" || return 1
  printf '%s %s\n' "$url" "$sha"
}

require_docker() {
  if ! command -v docker >/dev/null 2>&1; then
    printf 'Docker is required for the Orbbec agent image. Install Docker and retry.\n' >&2
    return 1
  fi
  if ! docker info >/dev/null 2>&1; then
    printf 'Docker daemon is not running. Start Docker and retry.\n' >&2
    return 1
  fi
}

build_agent_run_args() {
  local image="$1"
  local state="$2"
  local opentrons="${A2ALAB_OPENTRONS_URL:-http://host.docker.internal:31950}"
  local a2a_port="${A2ALAB_A2A_PORT:-31000}"
  local mcp_port="${A2ALAB_MCP_PORT:-31001}"
  local sila_port="${A2ALAB_SILA_PORT:-50052}"
  local frame_port="${A2ALAB_FRAME_SET_PORT:-31003}"
  local -a args=(
    run --rm --init
    --name "$(agent_container_name)"
    --user "$(id -u):$(id -g)"
    -e "A2ALAB_OPENTRONS_URL=${opentrons}"
    -e A2ALAB_LISTEN_HOST=0.0.0.0
    -e A2ALAB_SILA_HOST=0.0.0.0
    -e "A2ALAB_SILA_PORT=${sila_port}"
    -p "${a2a_port}:31000"
    -p "${mcp_port}:31001"
    -p "${sila_port}:${sila_port}"
    -p "${frame_port}:31003"
    -v "${state}:/var/lib/a2a-lab-ot2"
  )
  if [[ -n "${A2ALAB_PUBLIC_URL:-}" ]]; then
    args+=(-e "A2ALAB_PUBLIC_URL=${A2ALAB_PUBLIC_URL}")
  fi
  if [[ -n "${A2ALAB_ORBBEC_CAMERA:-}" ]]; then
    args+=(-e "A2ALAB_ORBBEC_CAMERA=${A2ALAB_ORBBEC_CAMERA}")
  fi
  if [[ -n "${A2ALAB_ORBBEC_SERIAL:-}" ]]; then
    args+=(-e "A2ALAB_ORBBEC_SERIAL=${A2ALAB_ORBBEC_SERIAL}")
  fi
  if [[ -n "${A2ALAB_ORBBEC_USB_DEVICES:-}" ]]; then
    local device
    local devices="${A2ALAB_ORBBEC_USB_DEVICES//,/ }"
    for device in $devices; do
      args+=(--device "${device}:${device}")
    done
  fi
  args+=("$image")
  printf '%s\n' "${args[@]}"
}

validate_agent_run() {
  local state="$1"
  if [[ ! -d "$state" || ! -w "$state" ]]; then
    printf 'application state directory %s is not writable\n' "$state" >&2
    return 1
  fi
  if [[ -n "${A2ALAB_ORBBEC_USB_DEVICES:-}" ]]; then
    local device
    local devices="${A2ALAB_ORBBEC_USB_DEVICES//,/ }"
    for device in $devices; do
      if [[ ! -e "$device" ]]; then
        printf 'USB device %s is not accessible. Pass an existing device node. Do not use --privileged.\n' "$device" >&2
        return 1
      fi
    done
  fi
  local rendered
  rendered="$(build_agent_run_args "image:test" "$state")"
  case "$rendered" in
    *"--privileged"*|*"--network host"*)
      printf 'refusing privileged or host networking for the agent container\n' >&2
      return 1
      ;;
  esac
}
