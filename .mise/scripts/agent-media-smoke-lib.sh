# shellcheck shell=bash
# Shared helpers for opt-in dual-source media smoke checks.

media_smoke_a2a_origin() {
  local port="${A2ALAB_A2A_PORT:-31000}"
  printf 'http://127.0.0.1:%s\n' "$port"
}

media_smoke_wait_card() {
  local origin="$1"
  local attempts="${2:-150}"
  local _ i
  for ((i = 0; i < attempts; i++)); do
    if curl -fsS "${origin}/.well-known/agent-card.json" >/dev/null 2>&1; then
      return 0
    fi
    sleep 1
  done
  printf 'A2A agent card did not become ready at %s\n' "$origin" >&2
  return 1
}

media_smoke_verify_jpeg_bytes() {
  local file="$1"
  python3 - "$file" <<'PY'
import pathlib, sys
path = pathlib.Path(sys.argv[1])
data = path.read_bytes()
if len(data) < 4 or data[:3] != b"\xff\xd8\xff":
    raise SystemExit(f"{path}: not a JPEG ({len(data)} bytes)")
print(len(data))
PY
}

media_smoke_report_policy() {
  agent_retention_policy_summary
}

media_smoke_diagnose() {
  local label="$1"
  local detail="$2"
  printf '%s: %s\n' "$label" "$detail" >&2
  print_agent_rollback_instructions "$(agent_default_state_dir)"
}
