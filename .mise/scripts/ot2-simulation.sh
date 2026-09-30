#!/usr/bin/env bash
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
cd "$root"

ot_url="${OPENTRONS_URL:-http://127.0.0.1:31950}"
protocol="$root/protocols/serial_dilution.py"
header="Opentrons-Version: *"

ot() {
  local mixed code body
  mixed="$(curl -sS -w '\n%{http_code}' -H "$header" "$@")"
  code="$(printf '%s\n' "$mixed" | tail -n 1)"
  body="$(printf '%s\n' "$mixed" | sed '$d')"
  if [[ "$code" != 2* ]]; then
    printf 'robot-server HTTP %s\n%s\n' "$code" "$body" >&2
    return 1
  fi
  printf '%s\n' "$body"
}

data_id() {
  python3 -c 'import json,sys; print(json.load(sys.stdin)["data"]["id"])'
}

data_status() {
  python3 -c 'import json,sys; print(json.load(sys.stdin)["data"]["status"])'
}

analysis_ready() {
  python3 -c 'import json,sys
p=json.load(sys.stdin)["data"]
summaries=p.get("analysisSummaries") or []
raise SystemExit(0 if summaries and all(s.get("status")!="pending" for s in summaries) else 1)'
}

current_runs() {
  python3 -c 'import json,sys
for run in json.load(sys.stdin).get("data") or []:
    if run.get("current"):
        print("%s\t%s" % (run["id"], run.get("status", "")))'
}

wait_health() {
  local _
  for _ in $(seq 1 90); do
    if curl -fsS -H "$header" "$ot_url/health" >/dev/null 2>&1; then
      return 0
    fi
    sleep 2
  done
  printf 'OT-2 simulator is not running at %s. Start it with: mise run ot2-simulator\n' "$ot_url" >&2
  return 1
}

wait_run() {
  local run_id="$1"
  shift
  local status="" _
  local want
  for _ in $(seq 1 80); do
    status="$(ot "$ot_url/runs/$run_id" | data_status)"
    for want in "$@"; do
      if [[ "$status" == "$want" ]]; then
        return 0
      fi
    done
    sleep 0.25
  done
  printf 'run %s status is %s, wanted one of: %s\n' "$run_id" "$status" "$*" >&2
  return 1
}

play() {
  local run_id="$1"
  ot -H 'Content-Type: application/json' \
    -d '{"data":{"actionType":"play"}}' \
    "$ot_url/runs/$run_id/actions" >/dev/null
}

release_current() {
  local id status
  while IFS=$'\t' read -r id status; do
    [[ -z "$id" ]] && continue
    case "$status" in
      running | paused | pause-requested | blocked-by-open-door | awaiting-recovery)
        ot -H 'Content-Type: application/json' \
          -d '{"data":{"actionType":"stop"}}' \
          "$ot_url/runs/$id/actions" >/dev/null || true
        ;;
    esac
    ot -X PATCH -H 'Content-Type: application/json' \
      -d '{"data":{"current":false}}' \
      "$ot_url/runs/$id" >/dev/null
  done < <(ot "$ot_url/runs?pageLength=50" | current_runs)
}

wait_health
release_current

upload="$(ot -F "files=@${protocol};type=text/x-python" "$ot_url/protocols")"
protocol_id="$(printf '%s\n' "$upload" | data_id)"

for _ in $(seq 1 60); do
  body="$(ot "$ot_url/protocols/$protocol_id")"
  if printf '%s\n' "$body" | analysis_ready; then
    break
  fi
  sleep 0.5
done
if ! printf '%s\n' "$(ot "$ot_url/protocols/$protocol_id")" | analysis_ready; then
  printf 'protocol analysis did not finish\n' >&2
  exit 1
fi

run_id="$(
  ot -H 'Content-Type: application/json' \
    -d "$(printf '{"data":{"protocolId":"%s"}}' "$protocol_id")" \
    "$ot_url/runs" | data_id
)"

play "$run_id"
wait_run "$run_id" failed awaiting-recovery awaiting-recovery-paused

status="$(ot "$ot_url/runs/$run_id" | data_status)"
printf 'run_id %s\n' "$run_id"
printf 'status %s\n' "$status"
printf 'query with: mise run a2a-lab-ot2 -- query-logs\n'
printf '             mise run a2a-lab-ot2 -- get-workflow-status %s\n' "$run_id"
