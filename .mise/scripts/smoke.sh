#!/usr/bin/env bash
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
cd "$root"

# shellcheck source=ot2-simulator-docker.sh
source "$root/.mise/scripts/ot2-simulator-docker.sh"

server_log="$(mktemp)"
agent_log="$(mktemp)"
mcp_body=""
sim_pid=""
agent_pid=""
container="$(ot2_simulator_container)"

cleanup() {
  if [[ -n "$agent_pid" ]] && kill -0 "$agent_pid" 2>/dev/null; then
    kill "$agent_pid" 2>/dev/null || true
    wait "$agent_pid" 2>/dev/null || true
  fi
  if [[ -n "$sim_pid" ]] && kill -0 "$sim_pid" 2>/dev/null; then
    kill "$sim_pid" 2>/dev/null || true
    wait "$sim_pid" 2>/dev/null || true
  fi
  docker rm -f "$container" >/dev/null 2>&1 || true
  rm -f "$server_log" "$agent_log"
  if [[ -n "$mcp_body" ]]; then
    rm -f "$mcp_body"
  fi
}
trap cleanup EXIT

bash "$root/.mise/scripts/ot2-simulator-setup.sh"
docker rm -f "$container" >/dev/null 2>&1 || true
for _ in $(seq 1 30); do
  if ! curl -fsS -H 'Opentrons-Version: *' http://127.0.0.1:31950/health >/dev/null 2>&1; then
    break
  fi
  sleep 0.5
done
if curl -fsS -H 'Opentrons-Version: *' http://127.0.0.1:31950/health >/dev/null 2>&1; then
  printf '127.0.0.1:31950 stayed occupied after removing the simulator container\n' >&2
  exit 1
fi
bash "$root/.mise/scripts/ot2-simulator.sh" >"$server_log" 2>&1 &
sim_pid=$!

for _ in $(seq 1 90); do
  if curl -fsS -H 'Opentrons-Version: *' http://127.0.0.1:31950/health >/dev/null 2>&1; then
    break
  fi
  if ! kill -0 "$sim_pid" 2>/dev/null; then
    cat "$server_log" >&2
    exit 1
  fi
  sleep 2
done
curl -fsS -H 'Opentrons-Version: *' http://127.0.0.1:31950/health >/dev/null

SILA_PORT=0 cargo run --quiet --bin a2a-lab-ot2 -- serve >"$agent_log" 2>&1 &
agent_pid=$!

for _ in $(seq 1 120); do
  if curl -fsS http://127.0.0.1:31000/.well-known/agent-card.json >/dev/null 2>&1; then
    break
  fi
  if ! kill -0 "$agent_pid" 2>/dev/null; then
    cat "$agent_log" >&2
    exit 1
  fi
  sleep 1
done
card="$(curl -fsS http://127.0.0.1:31000/.well-known/agent-card.json)"
printf '%s' "$card" | python3 -c '
import json, sys
card = json.load(sys.stdin)
ids = [skill["id"] for skill in card["skills"]]
expected = [
    "list-log-sources",
    "query-logs",
    "list-metrics",
    "query-metric",
    "list-tasks",
    "start-task",
    "get-task-status",
    "agent-message",
]
if ids != expected:
    raise SystemExit(f"agent card skills {ids}")
'

a2a_tasks="$(curl -fsS \
  -H 'Content-Type: application/a2a+json' \
  -H 'A2A-Version: 1.0' \
  --data-binary @- \
  http://127.0.0.1:31000/message:send <<'EOF'
{"message":{"messageId":"smoke-list-tasks","role":"ROLE_USER","parts":[{"mediaType":"application/json","data":{"operation":"list_tasks","params":{"page":{"limit":1000}}}}]}}
EOF
)"
printf '%s' "$a2a_tasks" | python3 -c '
import json, sys
body = json.load(sys.stdin)
task = body.get("task", body)
parts = [
    part
    for artifact in task.get("artifacts", [])
    for part in artifact.get("parts", [])
]
data = next(part["data"] for part in parts if "data" in part)
operation = data.get("operation")
if operation != "list_tasks":
    raise SystemExit("a2a operation " + str(operation))
ids = [item["id"] for item in data["result"]["items"]]
if "run_serial_dilution" not in ids:
    raise SystemExit("a2a list_tasks missing run_serial_dilution")
'

mcp_body="$(mktemp)"
mcp_session=""
mcp_post() {
  local payload="$1"
  local args=(-fsS -D - -o "$mcp_body" -H 'Content-Type: application/json' -H 'Accept: application/json, text/event-stream')
  if [[ -n "$mcp_session" ]]; then
    args+=(-H "Mcp-Session-Id: $mcp_session")
  fi
  local headers
  headers="$(curl "${args[@]}" --data-binary "$payload" http://127.0.0.1:31001/mcp)"
  local found
  found="$(printf '%s\n' "$headers" | awk 'tolower($1) == "mcp-session-id:" { print $2; exit }' | tr -d '\r')"
  if [[ -n "$found" ]]; then
    mcp_session="$found"
  fi
}
mcp_post '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2026-07-28","capabilities":{},"clientInfo":{"name":"a2a-lab-ot2-smoke","version":"0"}}}'
mcp_post '{"jsonrpc":"2.0","method":"notifications/initialized"}'
mcp_post '{"jsonrpc":"2.0","id":2,"method":"tools/list","params":{}}'
python3 - "$mcp_body" <<'PY'
import json, pathlib, sys
raw = pathlib.Path(sys.argv[1]).read_text()
payloads = [line.removeprefix("data:").strip() for line in raw.splitlines() if line.startswith("data:")]
if not payloads:
    payloads = [raw]
required = {
    "list_log_sources",
    "query_logs",
    "list_metrics",
    "query_metric",
    "list_tasks",
    "start_task",
    "get_task_status",
}
found = set()
for payload in payloads:
    if not payload:
        continue
    try:
        value = json.loads(payload)
    except json.JSONDecodeError:
        continue
    for tool in value.get("result", {}).get("tools", []):
        found.add(tool["name"])
missing = sorted(required - found)
if missing:
    raise SystemExit(f"mcp tools missing {missing}\n{raw}")
PY
rm -f "$mcp_body"

reply_id() {
  local text="$1"
  local key="$2"
  printf '%s\n' "$text" | awk -v key="$key" '$1 == key { id = $2 } END { if (id == "") exit 1; print id }'
}
reply_body() {
  printf '%s\n' "$1" | awk '!/^task_id / && !/^context_id / { print }'
}
assert_reply() {
  local text="$1"
  local body
  body="$(reply_body "$text")"
  if [[ -z "${body//[[:space:]]/}" ]]; then
    printf 'agent-message reply was empty\n%s\n' "$text" >&2
    exit 1
  fi
}
first="$(cargo run --quiet --bin a2a-lab-ot2 -- agent-message "which tasks can I run?")"
assert_reply "$first"
first_task="$(reply_id "$first" task_id)"
context_id="$(reply_id "$first" context_id)"
second="$(cargo run --quiet --bin a2a-lab-ot2 -- agent-message --context-id "$context_id" "what did you find?")"
assert_reply "$second"
second_task="$(reply_id "$second" task_id)"
second_context="$(reply_id "$second" context_id)"
if [[ "$second_context" != "$context_id" || "$second_task" == "$first_task" ]]; then
  printf 'conversation did not continue with a new task\n%s\n%s\n' "$first" "$second" >&2
  exit 1
fi

cargo run --quiet --bin a2a-lab-ot2 -- list-tasks
cargo run --quiet --bin a2a-lab-ot2 -- list-log-sources
cargo run --quiet --bin a2a-lab-ot2 -- list-metrics
run_out="$(cargo run --quiet --bin a2a-lab-ot2 -- start-task)"
printf '%s\n' "$run_out"
run_id="$(printf '%s\n' "$run_out" | awk '/^run_id / { print $2; exit }')"
if [[ -z "$run_id" ]]; then
  printf 'a2a-lab-ot2 start-task did not print a run_id\n' >&2
  exit 1
fi
cargo run --quiet --bin a2a-lab-ot2 -- get-task-status "$run_id"
cargo run --quiet --bin a2a-lab-ot2 -- pause "$run_id" || true
cargo run --quiet --bin a2a-lab-ot2 -- resume "$run_id" || true
cargo run --quiet --bin a2a-lab-ot2 -- stop "$run_id" || true
cargo run --quiet --bin a2a-lab-ot2 -- home
cargo run --quiet --bin a2a-lab-ot2 -- query-logs
cargo run --quiet --bin a2a-lab-ot2 -- query-metrics run_command_count
curl -fsS -H 'Opentrons-Version: *' http://127.0.0.1:31950/health >/dev/null
printf 'smoke ok\n'
