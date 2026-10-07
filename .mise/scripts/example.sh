#!/usr/bin/env bash
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
cd "$root"

ot_url="${A2ALAB_OPENTRONS_URL:-http://127.0.0.1:31950}"
started_here=0
sim_pid=""
server_log=""

cleanup() {
  if [[ "$started_here" -eq 0 ]]; then
    return
  fi
  if [[ -n "$sim_pid" ]] && kill -0 "$sim_pid" 2>/dev/null; then
    kill "$sim_pid" 2>/dev/null || true
    wait "$sim_pid" 2>/dev/null || true
  fi
  if [[ -n "$server_log" ]]; then
    rm -f "$server_log"
  fi
}
trap cleanup EXIT INT TERM

lab() {
  cargo run --quiet --bin a2a-lab-ot2 -- --opentrons-url "$ot_url" "$@"
}

section() {
  printf '\n==> a2a-lab-ot2 %s\n' "$1"
}

wait_http() {
  local url="$1"
  local header="${2-}"
  local tries="$3"
  local pid="$4"
  local log="$5"
  local delay="${6:-1}"
  local _
  for _ in $(seq 1 "$tries"); do
    if [[ -n "$header" ]]; then
      if curl -fsS -H "$header" "$url" >/dev/null 2>&1; then
        return 0
      fi
    elif curl -fsS "$url" >/dev/null 2>&1; then
      return 0
    fi
    if [[ -n "$pid" ]] && ! kill -0 "$pid" 2>/dev/null; then
      if [[ -n "$log" ]]; then
        cat "$log" >&2
      fi
      printf 'process exited before %s responded\n' "$url" >&2
      return 1
    fi
    sleep "$delay"
  done
  printf '%s did not become ready\n' "$url" >&2
  return 1
}

start_stack() {
  started_here=1
  server_log="$(mktemp)"
  bash "$root/.mise/scripts/ot2-simulator-setup.sh"
  bash "$root/.mise/scripts/ot2-simulator.sh" >"$server_log" 2>&1 &
  sim_pid=$!
  wait_http "$ot_url/health" "Opentrons-Version: *" 90 "$sim_pid" "$server_log" 2
}

if curl -fsS -H 'Opentrons-Version: *' "$ot_url/health" >/dev/null 2>&1; then
  printf 'using OT-2 HTTP at %s\n' "$ot_url"
else
  printf 'OT-2 is down; starting ot2-simulator\n'
  start_stack
fi

printf 'showcase: list_tasks list_log_sources list_metrics query_logs query_metric start_task get_task_status\n'

section "list_tasks"
lab list-tasks
section "list_log_sources"
lab list-log-sources
section "list_metrics"
lab list-metrics

section "query_metric healthy"
lab query-metrics healthy

section "start_task execute_command home"
lab home

section "start_task get_protocols"
run_out="$(lab start-task get_protocols)"
printf '%s\n' "$run_out"
run_id="$(printf '%s\n' "$run_out" | awk '/^run_id / { print $2; exit }')"
if [[ -z "$run_id" ]]; then
  printf 'start_task did not print a run_id\n' >&2
  exit 1
fi

section "get_task_status"
lab get-task-status "$run_id"

section "query_logs run_commands"
lab query-logs run_commands
section "query_metric run_progress_percent"
lab query-metrics run_progress_percent
section "query_metric run_command_count"
lab query-metrics run_command_count

section "start_task pause_run"
lab pause "$run_id" || true
section "get_task_status"
lab get-task-status "$run_id"

terminal=""
for _ in $(seq 1 8); do
  status_out="$(lab get-task-status "$run_id")"
  printf '%s\n' "$status_out"
  terminal="$(printf '%s\n' "$status_out" | awk '/^state / { print $2; exit }')"
  if [[ "$terminal" == "completed" || "$terminal" == "failed" || "$terminal" == "canceled" ]]; then
    break
  fi
  section "start_task resume_run"
  lab resume "$run_id" || true
  sleep 0.5
  section "query_metric run_progress_percent"
  lab query-metrics run_progress_percent
done

section "query_logs run_commands"
lab query-logs run_commands
section "query_logs api.log"
lab query-logs api.log
section "query_metric run_command_count"
lab query-metrics run_command_count

if [[ "$terminal" != "completed" && "$terminal" != "failed" && "$terminal" != "canceled" ]]; then
  section "start_task stop_run"
  lab stop "$run_id" || true
  section "get_task_status"
  lab get-task-status "$run_id"
fi

section "start_task delete_run"
lab start-task delete_run --input "$(printf '{"run_id":"%s"}' "$run_id")" || true

printf '\nrecovery tasks are advertised by list_tasks; this protocol does not enter awaiting-recovery\n'
printf 'a2a-lab-ot2 example ok (run_id %s)\n' "$run_id"
