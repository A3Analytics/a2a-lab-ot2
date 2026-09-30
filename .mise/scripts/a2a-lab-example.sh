#!/usr/bin/env bash
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
cd "$root"

a2a_url="${A2A_URL:-http://127.0.0.1:31000}"

if ! curl -fsS "$a2a_url/.well-known/agent-card.json" >/dev/null 2>&1; then
  printf 'A2A agent is not running at %s. Start it with: mise run start\n' "$a2a_url" >&2
  exit 1
fi

lab() {
  cargo run --quiet --bin a2a-lab -- --a2a "$a2a_url" "$@"
}

section() {
  printf '\n==> a2a-lab %s\n' "$1"
}

section "list_workflows"
lab list-workflows
section "list_log_sources"
lab list-log-sources
section "list_metrics"
lab list-metrics

section "query_logs run_commands"
lab query-logs run_commands
section "query_logs command_errors"
lab query-logs command_errors

section "query_metric healthy"
lab query-metrics healthy
section "query_metric run_progress_percent"
lab query-metrics run_progress_percent
section "query_metric run_command_count"
lab query-metrics run_command_count

printf '\na2a-lab-example ok\n'
