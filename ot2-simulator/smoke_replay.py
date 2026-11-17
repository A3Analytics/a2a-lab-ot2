"""Verify a running replay archive and, optionally, the A2A Lab media gateway."""

import argparse
import base64
from datetime import datetime, timedelta
import json
from pathlib import Path
import re
from urllib.error import HTTPError
from urllib.parse import quote, urlencode
from urllib.request import Request, urlopen
from uuid import uuid4


def fetch(base, path, *, body=None, byte_range=None):
    headers = {"Opentrons-Version": "*", "Content-Type": "application/json", "A2A-Version": "1.0"}
    if byte_range:
        headers["Range"] = byte_range
    request = Request(base.rstrip("/") + path, headers=headers,
                      data=json.dumps(body).encode() if body is not None else None)
    with urlopen(request, timeout=20) as result:
        return result.status, result.headers, result.read()


def document(base, path):
    return json.loads(fetch(base, path)[2])


def timestamp(value):
    padded = re.sub(r"\.(\d+)(?=Z|[+-]\d\d:\d\d$)", lambda match: "." + match[1].ljust(6, "0")[:6], value)
    return datetime.fromisoformat(padded.replace("Z", "+00:00"))


def invoke(agent, operation, params):
    body = {"message": {"messageId": str(uuid4()), "role": "ROLE_USER", "parts": [
        {"mediaType": "application/json", "data": {"operation": operation, "params": params}},
    ]}}
    response = json.loads(fetch(agent, "/message:send", body=body)[2])
    task = response["task"]
    assert task["status"]["state"] == "TASK_STATE_COMPLETED", operation
    return [
        part["data"]["result"]
        for artifact in task["artifacts"] for part in artifact["parts"]
        if "data" in part
    ]


def original_exports(source_dir):
    if source_dir is None:
        return None
    exports = [json.loads(path.read_text()) for path in source_dir.glob("*.json")]
    return {export["data"]["id"]: export for export in exports}


def verify(simulator, agent, source_dir):
    archive = document(simulator, "/replay/v1/scenarios")
    scenario_ids = {scenario["run_id"] for scenario in archive["scenarios"]}
    listed_ids = {run["id"] for run in document(simulator, "/runs")["data"]}
    assert len(scenario_ids) == 3 and scenario_ids <= listed_ids
    originals = original_exports(source_dir)
    if originals is not None:
        assert scenario_ids == originals.keys(), "Simulator run IDs differ from the supplied exports"
    sources = document(simulator, "/replay/v1/sources")
    angles = {source["id"] for source in sources}
    assert angles == {"top", "side"}
    if agent:
        pages = invoke(agent, "list_video_sources", {"page": {"limit": 100}})
        assert {item["id"] for page in pages for item in page["items"]} == angles

    for scenario in archive["scenarios"]:
        run_path = "/runs/" + scenario["run_id"]
        run = document(simulator, run_path)["data"]
        assert run["status"] == "succeeded" and run["replay"]["title"] == scenario["title"]
        commands = [
            json.loads(command)
            for command in document(simulator, run_path + "/commandsAsPreSerializedList")["data"]
        ]
        assert len(commands) == 39 and commands == scenario["commands"]
        if originals is not None:
            exported = originals[run["id"]]
            assert {key: value for key, value in run.items() if key != "replay"} == exported["data"]
            assert commands == exported["commands"]["data"], f"Command history diverged for {run['id']}"
        focus = run["replay"].get("focusCommandId") or scenario["commands"][11]["id"]
        command = document(simulator, run_path + "/commands/" + focus)["data"]
        for angle in sorted(angles):
            at = command["completedAt"]
            query = urlencode({"at": at})
            _, headers, jpeg = fetch(simulator, f"/replay/v1/frame/{angle}?{query}")
            assert jpeg.startswith(b"\xff\xd8") and headers["x-captured-at"]
            timeline = document(simulator, f"/replay/v1/timeline/{angle}")
            assert not timeline["live_available"]
            status, headers_range, initial = fetch(
                simulator, f"/replay/v1/video/{scenario['slug']}/{angle}", byte_range="bytes=0-1",
            )
            assert status == 206 and len(initial) == 2 and headers_range["content-range"].startswith("bytes 0-1/")
            clip = document(simulator, f"/replay/v1/clip/{angle}?" + urlencode(
                {"start": command["startedAt"], "end": at},
            ))
            media = f"/replay/v1/media/{clip['id']}/"
            playlist = fetch(simulator, media + "index.m3u8")[2]
            assert b"#EXT-X-ENDLIST" in playlist and b"#EXT-X-PROGRAM-DATE-TIME" in playlist
            initial_segment = next(line for line in playlist.decode().splitlines() if line.endswith(".m4s"))
            assert fetch(simulator, media + "init.mp4")[2][4:8] == b"ftyp"
            assert fetch(simulator, media + initial_segment)[2][4:8] == b"styp"
            if agent:
                frame, = invoke(agent, "get_video_frame_at", {"source_id": angle, "at": at})
                assert base64.b64decode(frame["image"]["data"]) == jpeg
                assert timestamp(frame["actual_captured_at"]) == timestamp(headers["x-captured-at"])
                remote_clip, = invoke(agent, "get_video_clip", {
                    "source_id": angle, "range": clip["requested_range"],
                })
                assert fetch(agent, remote_clip["path"] + "index.m3u8")[2] == playlist
            print(f"{scenario['title']} · {angle} · {headers['x-captured-at']}")

    start = min(item["cameras"]["top"]["start"] for item in archive["scenarios"])
    before = (timestamp(start) - timedelta(seconds=1)).isoformat()
    try:
        fetch(simulator, "/replay/v1/frame/top?at=" + quote(before))
    except HTTPError as error:
        assert error.code == 404
    else:
        raise AssertionError("Archive returned a frame outside coverage")
    print("Verified archive", archive["revision"])
    if originals is not None:
        print("All three run records and 117 commands match the supplied OT-2 exports")


if __name__ == "__main__":
    args = argparse.ArgumentParser(description=__doc__)
    args.add_argument("--simulator", default="http://localhost:31950")
    args.add_argument("--agent")
    args.add_argument("--source-dir", type=Path, help="Compare replay records against the original OT-2 exports")
    options = args.parse_args()
    verify(options.simulator, options.agent, options.source_dir)
