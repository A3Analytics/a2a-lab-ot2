"""Build the private simulator archive from the recorded OT-2 exports."""

import argparse
import ast
from datetime import datetime, timedelta, timezone
import hashlib
import json
from pathlib import Path
import shutil
from statistics import median
import subprocess
import tempfile


SCENARIOS = (
    ("good_run", "Good run", "good run - ", "good run test protocol.py"),
    ("bad_run_no_tip_pickup", "Missed tip pickup", "bad run 1 - ", "bad run test protocol.py"),
    ("bad_run_cover_on", "Reservoir cover left on", "bad run cover on - ", "good run test protocol.py"),
)
ANGLES = ("top", "side")
EPOCH = datetime(1970, 1, 1, tzinfo=timezone.utc)
FPS = 30


def timestamp_us(text):
    value = datetime.fromisoformat(text.replace("Z", "+00:00"))
    if value.tzinfo is None:
        raise ValueError("Timestamp must include a UTC offset")
    return (value - EPOCH) // timedelta(microseconds=1)


def utc(micros):
    return (EPOCH + timedelta(microseconds=micros)).isoformat().replace("+00:00", "Z")


def json_bytes(value):
    return json.dumps(value, sort_keys=True, separators=(",", ":"), ensure_ascii=False).encode()


def export_from(source, prefix):
    files = list(source.glob(f"{prefix}*.json"))
    if len(files) != 1:
        raise ValueError(f"Expected one run export starting with {prefix!r}")
    export = json.loads(files[0].read_text())
    commands = export["commands"]["data"]
    if len(commands) != export["commands"]["meta"]["totalLength"]:
        raise ValueError(f"Incomplete command history in {files[0]}")
    return export


def protocol_metadata(path):
    parsed = ast.parse(path.read_text())
    for node in parsed.body:
        if isinstance(node, ast.Assign) and any(
            isinstance(target, ast.Name) and target.id == "metadata" for target in node.targets
        ):
            return ast.literal_eval(node.value)
    raise ValueError(f"Protocol metadata missing in {path}")


def camera_alignment(anchors, commands):
    offsets = []
    events = []
    for anchor in anchors:
        step = anchor["step"]
        boundary = anchor["boundary"]
        if step < 1 or boundary not in ("startedAt", "completedAt"):
            raise ValueError("An alignment anchor needs a valid command step and boundary")
        command = commands[step - 1]
        offset_us = round(anchor["video_seconds"] * 1_000_000)
        command_us = timestamp_us(command[boundary])
        offsets.append(command_us - offset_us)
        events.append(
            {"command_id": command["id"], "timestamp": utc(command_us), "offset_us": offset_us}
        )
    if len({event["command_id"] for event in events}) < 2:
        raise ValueError("Each camera must be aligned against multiple recorded commands")
    start_us = round(median(offsets))
    if max(abs(offset - start_us) for offset in offsets) > 750_000:
        raise ValueError("Camera anchors disagree by more than 750 ms from the fitted start")
    return start_us, events


def ffmpeg(*args):
    subprocess.run(["ffmpeg", "-nostdin", "-hide_banner", "-loglevel", "error", *args], check=True)


def package_camera(source, output, start_us, events):
    output.mkdir(parents=True)
    video = output / "video.mp4"
    ffmpeg(
        "-fflags", "+discardcorrupt", "-i", str(source), "-map", "0:v:0", "-an",
        "-vf", f"fps={FPS}", "-c:v", "libx264", "-pix_fmt", "yuv420p",
        "-preset", "fast", "-crf", "21", "-g", str(FPS), "-keyint_min", str(FPS),
        "-sc_threshold", "0", "-movflags", "+faststart", str(video),
    )
    ffmpeg("-xerror", "-i", str(video), "-map", "0:v:0", "-f", "null", "-")
    ffmpeg(
        "-i", str(video), "-c", "copy", "-f", "hls", "-hls_time", "1",
        "-hls_playlist_type", "vod", "-hls_segment_type", "fmp4",
        "-hls_flags", "independent_segments", "-hls_fmp4_init_filename", "init.mp4",
        "-hls_segment_filename", str(output / "seg-%05d.m4s"), str(output / "index.m3u8"),
    )
    segments = []
    current = start_us
    duration_us = None
    for line in (output / "index.m3u8").read_text().splitlines():
        if line.startswith("#EXTINF:"):
            duration_us = round(float(line[8:].split(",", 1)[0]) * 1_000_000)
        elif line and not line.startswith("#") and duration_us:
            segments.append({"file": line, "start": utc(current), "end": utc(current + duration_us)})
            current += duration_us
            duration_us = None
    if not segments or any(start_us + event["offset_us"] >= current for event in events):
        raise ValueError(f"Packaged video does not cover alignment anchors for {source}")
    stream = json.loads(subprocess.check_output([
        "ffprobe", "-v", "error", "-select_streams", "v:0", "-show_entries",
        "stream=width,height", "-of", "json", str(video),
    ]))["streams"][0]
    return {
        "start": utc(start_us), "end": utc(current), "fps": FPS, **stream,
        "anchors": events, "segments": segments,
    }


def build_archive(source, alignment, destination):
    if destination.exists():
        raise ValueError(f"Destination already exists: {destination}")
    with tempfile.TemporaryDirectory(prefix=".ot2-replay-", dir=destination.parent) as working:
        output = Path(working) / "bundle"
        output.mkdir()
        scenarios = []
        for slug, title, prefix, protocol_file in SCENARIOS:
            export = export_from(source, prefix)
            run = export["data"]
            commands = export["commands"]["data"]
            cameras = {}
            for angle in ANGLES:
                start_us, events = camera_alignment(alignment[slug][angle], commands)
                cameras[angle] = package_camera(
                    source / slug / f"{angle}.mkv", output / slug / angle, start_us, events,
                )
            scenarios.append({
                "slug": slug, "title": title, "run": run, "commands": commands, "cameras": cameras,
                "protocol": {"id": run["protocolId"], "metadata": protocol_metadata(source / protocol_file)},
            })
        manifest = {"version": 1, "scenarios": scenarios}
        files = {
            str(path.relative_to(output)): hashlib.file_digest(path.open("rb"), "sha256").hexdigest()
            for path in sorted(output.rglob("*")) if path.is_file()
        }
        revision = hashlib.sha256(json_bytes({"manifest": manifest, "files": files})).hexdigest()
        (output / "manifest.json").write_bytes(json_bytes({**manifest, "revision": revision, "files": files}))
        shutil.move(output, destination)
    return revision


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("source", type=Path)
    parser.add_argument("alignment", type=Path)
    parser.add_argument("destination", type=Path)
    args = parser.parse_args()
    args.destination.parent.mkdir(parents=True, exist_ok=True)
    revision = build_archive(args.source, json.loads(args.alignment.read_text()), args.destination)
    print(revision)


if __name__ == "__main__":
    main()
