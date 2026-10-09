#!/usr/bin/env bash
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
tck_url="https://github.com/A3Analytics/a2a-lab-tck"
tck_revision="1fd47d5c73e6fa3f7bd6505d75e43ee68cae33fd"
report="${A2ALAB_COMPLIANCE_REPORT:-$root/target/compliance/a2a-lab-ot2.json}"
variant="${A2ALAB_COMPLIANCE_FIXTURE_VARIANT:-standard}"
suite="${A2ALAB_COMPLIANCE_SUITE:-full}"
llm_check="${A2ALAB_COMPLIANCE_LLM_CHECK:-true}"
work="$(mktemp -d)"
readiness="$work/readiness.json"
fixture_log="$work/fixture.log"
fixtures="$work/fixtures.json"
fixture_pid=""

cleanup() {
  if [[ -n "$fixture_pid" ]] && kill -0 "$fixture_pid" 2>/dev/null; then
    kill -INT "$fixture_pid" 2>/dev/null || true
    wait "$fixture_pid" 2>/dev/null || true
  fi
  rm -rf "$work"
}
trap cleanup EXIT

if [[ -n "${A2ALAB_TCK_ROOT:-}" ]]; then
  tck="$A2ALAB_TCK_ROOT"
else
  tck="$work/a2a-lab-tck"
  git init --quiet "$tck"
  git -C "$tck" fetch --quiet --depth 1 "$tck_url" "$tck_revision"
  git -C "$tck" checkout --quiet --detach FETCH_HEAD
  resolved_revision="$(git -C "$tck" rev-parse HEAD)"
  if [[ "$resolved_revision" != "$tck_revision" ]]; then
    printf 'A2A-LAB TCK resolved to %s, expected %s\n' \
      "$resolved_revision" "$tck_revision" >&2
    exit 2
  fi
fi
if [[ ! -f "$tck/Cargo.toml" ]]; then
  printf 'A2A-LAB TCK not found at %s\n' "$tck" >&2
  exit 2
fi
if [[ "$suite" != "basic" && "$suite" != "full" ]]; then
  printf 'A2ALAB_COMPLIANCE_SUITE must be basic or full\n' >&2
  exit 2
fi
if [[ "$llm_check" != "true" && "$llm_check" != "false" ]]; then
  printf 'A2ALAB_COMPLIANCE_LLM_CHECK must be true or false\n' >&2
  exit 2
fi

cd "$root"
cargo build --quiet --bin a2a-lab-ot2
"$root/target/debug/a2a-lab-ot2" fixture --variant "$variant" >"$readiness" 2>"$fixture_log" &
fixture_pid=$!

for _ in $(seq 1 200); do
  if [[ -s "$readiness" ]]; then
    break
  fi
  if ! kill -0 "$fixture_pid" 2>/dev/null; then
    cat "$fixture_log" >&2
    exit 1
  fi
  sleep 0.05
done
if [[ ! -s "$readiness" ]]; then
  printf 'fixture did not become ready\n' >&2
  cat "$fixture_log" >&2
  exit 1
fi

python3 - "$readiness" "$fixtures" <<'PY'
import json
import pathlib
import sys

ready = json.loads(pathlib.Path(sys.argv[1]).read_text().splitlines()[0])
pathlib.Path(sys.argv[2]).write_text(json.dumps(ready["fixtures"]) + "\n")
PY
a2a_url="$(python3 -c 'import json,sys; print(json.load(open(sys.argv[1]))["a2a_url"])' "$readiness")"
mcp_url="$(python3 -c 'import json,sys; print(json.load(open(sys.argv[1]))["mcp_url"])' "$readiness")"

mkdir -p "$(dirname "$report")"
rm -f "$report"
cd "$tck"
arguments=(
  --a2a-url "$a2a_url"
  --mcp-url "$mcp_url"
  --fixtures "$fixtures"
  --report "$report"
  --implementation-name a2a-lab-ot2
  --implementation-version 0.1.0
  --profile 1.1.0
  --suite "$suite"
)
if [[ "$llm_check" == "false" ]]; then
  arguments+=(--no-llm-check)
fi
cargo run --quiet --bin a2a-lab-compliance -- "${arguments[@]}"
printf 'A2A-LAB compliance report: %s\n' "$report"
