# shellcheck shell=bash
# Shared helpers for the agent image. These functions do not start Docker.

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

agent_container_config_path() {
  printf '/etc/a2alab/a2a-lab.json\n'
}

validate_a2alab_config_selection() {
  local config="${A2ALAB_CONFIG:-}"
  if [[ -z "$config" ]]; then
    return 0
  fi
  if [[ ! -f "$config" ]]; then
    printf 'configuration file %s is missing or not a regular file\n' "$config" >&2
    return 1
  fi
  if [[ ! -r "$config" ]]; then
    printf 'configuration file %s is not readable\n' "$config" >&2
    return 1
  fi
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

require_docker() {
  if ! command -v docker >/dev/null 2>&1; then
    printf 'Docker is required for the agent image. Install Docker and retry.\n' >&2
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
  if [[ -n "${A2ALAB_CONFIG:-}" ]]; then
    args+=(-v "${A2ALAB_CONFIG}:$(agent_container_config_path):ro")
  fi
  local credential_name
  local -a credential_names=(
    AWS_PROFILE
    AWS_REGION
    AWS_DEFAULT_REGION
    AWS_ACCESS_KEY_ID
    AWS_SECRET_ACCESS_KEY
    AWS_SESSION_TOKEN
    OPENAI_API_KEY
    ANTHROPIC_API_KEY
  )
  for credential_name in "${credential_names[@]}"; do
    if [[ -n "${!credential_name:-}" ]]; then
      args+=(-e "${credential_name}=${!credential_name}")
    fi
  done
  if [[ -n "${A2ALAB_PUBLIC_URL:-}" ]]; then
    args+=(-e "A2ALAB_PUBLIC_URL=${A2ALAB_PUBLIC_URL}")
  fi
  if [[ -n "${A2ALAB_CAMERA_INDEX:-}" ]]; then
    args+=(-e "A2ALAB_CAMERA_INDEX=${A2ALAB_CAMERA_INDEX}")
  fi
  if [[ -n "${A2ALAB_EXTERNAL_CAMERA_INDEX:-}" ]]; then
    args+=(-e "A2ALAB_EXTERNAL_CAMERA_INDEX=${A2ALAB_EXTERNAL_CAMERA_INDEX}")
  fi
  if [[ -n "${A2ALAB_EXTERNAL_CAMERA_DESCRIPTION:-}" ]]; then
    args+=(-e "A2ALAB_EXTERNAL_CAMERA_DESCRIPTION=${A2ALAB_EXTERNAL_CAMERA_DESCRIPTION}")
  fi
  if [[ -n "${A2ALAB_EXTERNAL_VIDEO_DEVICES:-}" ]]; then
    local device
    local devices="${A2ALAB_EXTERNAL_VIDEO_DEVICES//,/ }"
    for device in $devices; do
      args+=(--device "${device}:${device}")
    done
  fi
  if [[ -n "${A2ALAB_VIDEO_FPS:-}" ]]; then
    args+=(-e "A2ALAB_VIDEO_FPS=${A2ALAB_VIDEO_FPS}")
  fi
  if [[ -n "${A2ALAB_VIDEO_RETENTION_SECS:-}" ]]; then
    args+=(-e "A2ALAB_VIDEO_RETENTION_SECS=${A2ALAB_VIDEO_RETENTION_SECS}")
  fi
  if [[ -n "${A2ALAB_VIDEO_DISK_RETENTION_SECS:-}" ]]; then
    args+=(-e "A2ALAB_VIDEO_DISK_RETENTION_SECS=${A2ALAB_VIDEO_DISK_RETENTION_SECS}")
  fi
  if [[ -n "${A2ALAB_VIDEO_DVR_ROOT:-}" ]]; then
    args+=(-e "A2ALAB_VIDEO_DVR_ROOT=${A2ALAB_VIDEO_DVR_ROOT}")
  fi
  if [[ -n "${A2ALAB_IMAGE_RETENTION:-}" ]]; then
    args+=(-e "A2ALAB_IMAGE_RETENTION=${A2ALAB_IMAGE_RETENTION}")
  fi
  args+=("$image")
  if [[ -n "${A2ALAB_CONFIG:-}" ]]; then
    args+=(--config "$(agent_container_config_path)")
  fi
  printf '%s\n' "${args[@]}"
}

agent_default_state_dir() {
  printf '%s\n' "${A2ALAB_STATE_DIR:-$(agent_root)/.a2a-lab-ot2-agent}"
}

agent_retention_policy_summary() {
  cat <<'EOF'
live video memory retention: 5 seconds per enabled source (A2ALAB_VIDEO_RETENTION_SECS default)
indexed disk video retention: 259200 seconds (3 days) (A2ALAB_VIDEO_DISK_RETENTION_SECS default)
still-image age retention: 30 days with per-source count cap (A2ALAB_IMAGE_RETENTION default)
persistent state mount: /var/lib/a2a-lab-ot2 (images.sqlite, video-dvr/, frame-sets.sqlite, conversation db)
EOF
}

print_agent_rollback_instructions() {
  local state="${1:-$(agent_default_state_dir)}"
  cat <<EOF
Rollback without deleting retained media:
1. Stop the agent container: mise run agent-stop (SIGTERM, then remove the container).
2. Keep the host state directory intact: ${state}
3. Do not delete images.sqlite, video-dvr/, or frame-sets.sqlite under that directory.
4. Fix the reported device or permission issue, then rerun mise run agent-run.
5. To revert the container image only, rebuild with mise run agent-image and restart; retained files stay on the host mount.
EOF
}

validate_agent_run() {
  local state="$1"
  if [[ ! -d "$state" || ! -w "$state" ]]; then
    printf 'application state directory %s is not writable\n' "$state" >&2
    return 1
  fi
  if [[ -n "${A2ALAB_EXTERNAL_VIDEO_DEVICES:-}" ]]; then
    local device
    local devices="${A2ALAB_EXTERNAL_VIDEO_DEVICES//,/ }"
    for device in $devices; do
      if [[ ! -e "$device" ]]; then
        printf 'USB device %s is not accessible. Pass an existing device node. Do not use --privileged.\n' "$device" >&2
        return 1
      fi
    done
  fi
  if ! validate_a2alab_config_selection; then
    return 1
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
