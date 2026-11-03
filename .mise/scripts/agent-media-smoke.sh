#!/usr/bin/env bash
set -euo pipefail

if [[ "${A2ALAB_MEDIA_SMOKE:-}" != 1 ]]; then
  printf 'skipping dual-source media smoke; set A2ALAB_MEDIA_SMOKE=1 with camera access configured\n'
  exit 0
fi

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
cd "$root"

# shellcheck source=agent-image-lib.sh
source "$root/.mise/scripts/agent-image-lib.sh"
# shellcheck source=agent-media-smoke-lib.sh
source "$root/.mise/scripts/agent-media-smoke-lib.sh"

native_mode=0
if [[ "${A2ALAB_MEDIA_SMOKE_NATIVE:-}" == 1 ]]; then
  native_mode=1
fi

origin="$(media_smoke_a2a_origin)"
tmp="$(mktemp -d)"
if [[ "$native_mode" == 1 ]]; then
  trap 'rm -rf "$tmp"' EXIT
  media_smoke_wait_card "$origin" || exit 1
else
  require_docker
  arch="${A2ALAB_AGENT_ARCH:-$(docker info -f '{{.Architecture}}')}"
  arch="$(normalize_architecture "$arch")"
  image="$(agent_image_name "$arch")"
  if ! docker image inspect "$image" >/dev/null 2>&1; then
    media_smoke_diagnose "agent image" "missing ${image}; run mise run agent-image -- ${arch}"
    exit 1
  fi

  state="$(agent_default_state_dir)"
  mkdir -p "$state"
  if ! validate_agent_run "$state"; then
    print_agent_rollback_instructions "$state" >&2
    exit 1
  fi

  trap 'docker rm -f "$(agent_container_name)" >/dev/null 2>&1 || true; rm -rf "$tmp"' EXIT

  args=()
  while IFS= read -r line; do
    args+=("$line")
  done < <(build_agent_run_args "$image" "$state")
  run_args=()
  for arg in "${args[@]}"; do
    if [[ "$arg" == run ]]; then
      run_args+=(run -d)
      continue
    fi
    if [[ "$arg" == --rm ]]; then
      continue
    fi
    run_args+=("$arg")
  done
  run_args+=(serve)
  docker "${run_args[@]}" >/dev/null

  media_smoke_wait_card "$origin" || {
    docker logs "$(agent_container_name)" >&2 || true
    exit 1
  }
fi

python3 - "$origin" "$tmp" <<'PY'
import base64
import json
import sys
import time
import urllib.error
import urllib.request
from pathlib import Path

origin = sys.argv[1]
tmpdir = Path(sys.argv[2])


def post(path, payload):
    req = urllib.request.Request(
        f"{origin}{path}",
        data=json.dumps(payload).encode(),
        headers={
            "Content-Type": "application/a2a+json",
            "A2A-Version": "1.0",
        },
        method="POST",
    )
    with urllib.request.urlopen(req, timeout=120) as resp:
        return json.load(resp)


def a2a_results(body, operation):
    task = body.get("task", body)
    found = []
    for artifact in task.get("artifacts", []):
        for part in artifact.get("parts", []):
            data = part.get("data")
            if not isinstance(data, dict) or "result" not in data:
                continue
            if data.get("operation") != operation:
                raise RuntimeError(f"expected {operation}, got {data.get('operation')}")
            found.append(data["result"])
    if not found:
        raise RuntimeError(f"{operation} returned no result")
    return found


def a2a_result(body, operation):
    return a2a_results(body, operation)[0]


def list_sources(kind):
    if kind == "image":
        op = "list_image_sources"
    else:
        op = "list_video_sources"
    body = post(
        "/message:send",
        {
            "message": {
                "messageId": f"smoke-{kind}-sources",
                "role": "ROLE_USER",
                "parts": [
                    {
                        "mediaType": "application/json",
                        "data": {
                            "operation": op,
                            "params": {"page": {"limit": 100}},
                        },
                    }
                ],
            }
        },
    )
    return {
        item["id"]
        for result in a2a_results(body, op)
        for item in result.get("items", [])
    }


def decode_image(result):
    data_b64 = result.get("data")
    if not data_b64:
        raise RuntimeError("get_current_image did not return base64 data")
    media = result.get("descriptor", {}).get("media_type")
    data = base64.b64decode(data_b64)
    if media != "image/jpeg" or len(data) < 4 or data[:3] != b"\xff\xd8\xff":
        raise RuntimeError("get_current_image did not return JPEG bytes")
    return data


def image_id(result):
    return result["descriptor"]["id"]


def get_current(source_id, wait_secs):
    deadline = time.time() + wait_secs
    last = None
    while time.time() < deadline:
        try:
            body = post(
                "/message:send",
                {
                    "message": {
                        "messageId": f"smoke-current-{source_id}",
                        "role": "ROLE_USER",
                        "parts": [
                            {
                                "mediaType": "application/json",
                                "data": {
                                    "operation": "get_current_image",
                                    "params": {"source_id": source_id},
                                },
                            }
                        ],
                    }
                },
            )
            result = a2a_result(body, "get_current_image")
            return result, decode_image(result)
        except Exception as error:  # noqa: BLE001
            last = error
            time.sleep(1)
    raise RuntimeError(str(last))


def get_stored(image_id):
    body = post(
        "/message:send",
        {
            "message": {
                "messageId": f"smoke-get-{image_id}",
                "role": "ROLE_USER",
                "parts": [
                    {
                        "mediaType": "application/json",
                        "data": {
                            "operation": "get_image",
                            "params": {"id": image_id},
                        },
                    }
                ],
            }
        },
    )
    result = a2a_result(body, "get_image")
    return decode_image(result)


def follow_live(source_id):
    body = post(
        "/message:send",
        {
            "message": {
                "messageId": f"smoke-follow-{source_id}",
                "role": "ROLE_USER",
                "parts": [
                    {
                        "mediaType": "application/json",
                        "data": {
                            "operation": "follow_video",
                            "params": {"source_id": source_id},
                        },
                    }
                ],
            }
        },
    )
    return a2a_result(body, "follow_video")


def tight_jpeg(data):
    start = data.find(b"\xff\xd8\xff")
    end = data.rfind(b"\xff\xd9")
    if start < 0 or end < start:
        raise RuntimeError("payload is not a JPEG")
    return data[start : end + 2]


class LiveFrames:
    def __init__(self, path):
        self.resp = urllib.request.urlopen(f"{origin}{path.rstrip('/')}/", timeout=90)
        self.buf = b""

    def frame(self):
        while True:
            split = self.buf.find(b"\r\n\r\n")
            if split >= 0:
                header = self.buf[:split].decode("latin1", "replace")
                length = None
                for line in header.split("\r\n"):
                    if line.lower().startswith("content-length:"):
                        length = int(line.split(":", 1)[1].strip())
                if length is not None and len(self.buf) >= split + 4 + length:
                    payload = self.buf[split + 4 : split + 4 + length]
                    self.buf = self.buf[split + 4 + length :]
                    return tight_jpeg(payload)
            chunk = self.resp.read(1024 * 1024)
            if not chunk:
                raise RuntimeError("live gateway stream did not contain a JPEG")
            self.buf += chunk


image_ids = list_sources("image")
video_ids = list_sources("video")
expected = [item.strip() for item in __import__("os").environ.get(
    "A2ALAB_MEDIA_SMOKE_SOURCES", "external-camera,camera"
).split(",") if item.strip()]
failures = []
verified = {}

for source_id in expected:
    try:
        if source_id not in image_ids:
            raise RuntimeError("missing from list_image_sources")
        if source_id not in video_ids:
            raise RuntimeError("missing from list_video_sources")
        wait = 15
        meta, live = get_current(source_id, wait)
        stored_id = image_id(meta)
        stored = get_stored(stored_id)
        if stored != live:
            raise RuntimeError("stored get_image bytes differ from get_current_image")
        follow = follow_live(source_id)
        path = follow.get("path") or ""
        if not path.startswith("/media/video-live/"):
            raise RuntimeError(f"unexpected follow path {path!r}")
        stream = LiveFrames(path)
        previous = stream.frame()
        reused_meta, reused = get_current(source_id, 5)
        still = tight_jpeg(reused)
        frames = [previous]
        matched = previous == still
        advanced = False
        for _ in range(8):
            nxt = stream.frame()
            if nxt != frames[-1]:
                advanced = True
            frames.append(nxt)
            if nxt == still:
                matched = True
            if matched and advanced:
                break
        if not matched:
            raise RuntimeError("still JPEG was not reused on the live gateway")
        if not advanced:
            raise RuntimeError("live gateway frame did not advance")
        verified[source_id] = {
            "jpeg_bytes": len(tight_jpeg(live)),
            "media_type": "image/jpeg",
            "stored_id": image_id(reused_meta),
            "live_frame_bytes": [len(frame) for frame in frames[:2]],
        }
        print(f"{source_id}: verified JPEG {len(tight_jpeg(live))} bytes; live frames advanced")
    except Exception as error:  # noqa: BLE001
        failures.append(f"{source_id}: {error}")
        print(f"{source_id}: FAILED ({error})", file=sys.stderr)

if failures:
    for line in failures:
        print(line, file=sys.stderr)
    sys.exit(1)

print(json.dumps({"verified_sources": verified}, indent=2))
Path(tmpdir, "verified.json").write_text(json.dumps(list(verified.keys())))
PY

verified_file="$tmp/verified.json"
if [[ ! -s "$verified_file" ]]; then
  media_smoke_diagnose "media smoke" "no camera sources verified"
  exit 1
fi

media_smoke_report_policy

if [[ "$native_mode" == 1 ]]; then
  printf 'agent media smoke ok (native serve)\n'
  exit 0
fi

name="$(agent_container_name)"
docker stop --time 30 "$name" >/dev/null
docker start "$name" >/dev/null
media_smoke_wait_card "$origin" 120

python3 - "$origin" "$verified_file" <<'PY'
import json
import sys
import urllib.request

origin = sys.argv[1]
sources = json.loads(open(sys.argv[2]).read())

def post(payload):
    req = urllib.request.Request(
        f"{origin}/message:send",
        data=json.dumps(payload).encode(),
        headers={"Content-Type": "application/a2a+json", "A2A-Version": "1.0"},
        method="POST",
    )
    with urllib.request.urlopen(req, timeout=60) as resp:
        return json.load(resp)

for source_id in sources:
    body = post(
        {
            "message": {
                "messageId": f"smoke-timeline-{source_id}",
                "role": "ROLE_USER",
                "parts": [
                    {
                        "mediaType": "application/json",
                        "data": {
                            "operation": "get_video_timeline",
                            "params": {"source_id": source_id},
                        },
                    }
                ],
            }
        }
    )
    task = body.get("task", body)
    parts = [
        part
        for artifact in task.get("artifacts", [])
        for part in artifact.get("parts", [])
        if "data" in part
    ]
    payload = parts[0]["data"]
    if payload.get("operation") != "get_video_timeline":
        raise SystemExit("unexpected timeline operation")
    timeline = payload["result"]
    if not timeline.get("intervals"):
        raise SystemExit(f"{source_id}: timeline empty after restart")
    print(f"{source_id}: DVR timeline restored after restart")
PY

printf 'agent media smoke ok\n'
