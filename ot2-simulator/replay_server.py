"""HTTP archive and read-only run history layered over Opentrons robot-server."""

import asyncio
import json
import math
import os
from pathlib import Path
import re

from fastapi import APIRouter, HTTPException, Request
from fastapi.responses import FileResponse, HTMLResponse, JSONResponse, Response, StreamingResponse
from opentrons.protocol_engine.resources.camera_provider import CameraProvider
from robot_server.app import app
from robot_server.camera.fastapi_dependencies import get_camera_provider

from prepare_replay import timestamp_us, utc
from sample_pacing import SamplePacing, sample_pacing


class Replay:
    def __init__(self, directory):
        self.directory = directory
        manifest = json.loads((directory / "manifest.json").read_text())
        if manifest["version"] != 1:
            raise ValueError("Unsupported replay archive version")
        self.revision = manifest["revision"]
        expected = os.getenv("OT2_REPLAY_EXPECTED_REVISION")
        if expected and self.revision != expected:
            raise ValueError("Replay archive revision does not match the deployment")
        self.scenarios = {item["slug"]: item for item in manifest["scenarios"]}
        self.runs = {item["run"]["id"]: item for item in self.scenarios.values()}
        self.protocols = {item["protocol"]["id"]: item["protocol"] for item in self.scenarios.values()}

    @staticmethod
    def run_record(item):
        focus_step = {"bad_run_no_tip_pickup": 12, "bad_run_cover_on": 15}.get(item["slug"])
        replay = {"title": item["title"]}
        if focus_step is not None:
            replay["focusCommandId"] = item["commands"][focus_step - 1]["id"]
        return {**item["run"], "replay": replay}

    def camera(self, scenario, angle):
        try:
            return self.scenarios[scenario]["cameras"][angle]
        except KeyError as error:
            raise HTTPException(404, "Recorded camera unavailable") from error

    def sources(self):
        return [
            {"id": angle, "name": angle.title(), "description": f"Recorded replay · {angle} camera",
             "width": sample["width"], "height": sample["height"], "fps": sample["fps"],
             "media_type": "application/vnd.apple.mpegurl", "capabilities": ["h264"]}
            for angle, sample in next(iter(self.scenarios.values()))["cameras"].items()
        ]

    def timeline(self, angle):
        intervals = []
        for slug in self.scenarios:
            camera = self.camera(slug, angle)
            intervals.append({"start": camera["start"], "end": camera["end"]})
        return {"source_id": angle, "intervals": sorted(intervals, key=lambda item: item["start"]),
                "live_available": False}


def parse_time(text):
    try:
        return timestamp_us(text)
    except (ValueError, OverflowError) as error:
        raise HTTPException(422, "Expected a timestamp with a UTC offset") from error


def command_page(commands, cursor, page_length):
    start = max(0, cursor if cursor is not None else len(commands) - page_length)
    return {"data": commands[start:start + page_length],
            "meta": {"cursor": start, "totalLength": len(commands)}, "links": {}}


def video_response(path, request):
    size = path.stat().st_size
    headers = {"Accept-Ranges": "bytes"}
    if request.method == "HEAD":
        return Response(media_type="video/mp4", headers={**headers, "Content-Length": str(size)})
    byte_range = request.headers.get("range")
    if not byte_range:
        return FileResponse(path, media_type="video/mp4", headers=headers)
    match = re.fullmatch(r"bytes=(\d{0,20})-(\d{0,20})", byte_range)
    if match and (match[1] or match[2]):
        start = int(match[1]) if match[1] else max(0, size - int(match[2]))
        end = min(size - 1, int(match[2])) if match[1] and match[2] else size - 1
        if start <= end:
            length = end - start + 1

            def chunks():
                with path.open("rb") as stream:
                    stream.seek(start)
                    remaining = length
                    while remaining:
                        chunk = stream.read(min(remaining, 256 * 1024))
                        if not chunk:
                            break
                        remaining -= len(chunk)
                        yield chunk

            return StreamingResponse(chunks(), status_code=206, media_type="video/mp4", headers={
                **headers, "Content-Range": f"bytes {start}-{end}/{size}", "Content-Length": str(length),
            })
    return Response(status_code=416, headers={**headers, "Content-Range": f"bytes */{size}"})


def register(archive):
    router = APIRouter()
    # The Opentrons simulation provider records capture commands without opening a camera.
    app.dependency_overrides[get_camera_provider] = lambda: CameraProvider()

    @router.get("/replay")
    async def ui():
        return HTMLResponse(Path(__file__).with_name("replay.html").read_text())

    @router.get("/replay/v1/scenarios")
    async def scenarios():
        return {"revision": archive.revision, "fixr_url": os.getenv("OT2_REPLAY_FIXR_URL", ""),
                "scenarios": [
                    {"slug": item["slug"], "title": item["title"], "run_id": item["run"]["id"],
                     "status": item["run"]["status"], "commands": item["commands"],
                     "cameras": {angle: {"start": cam["start"], "end": cam["end"]}
                                 for angle, cam in item["cameras"].items()}}
                    for item in archive.scenarios.values()
                ]}

    @router.api_route("/replay/v1/video/{scenario}/{angle}", methods=["GET", "HEAD"])
    def video(scenario: str, angle: str, request: Request):
        archive.camera(scenario, angle)
        return video_response(archive.directory / scenario / angle / "video.mp4", request)

    @router.get("/replay/v1/sources")
    async def sources():
        return archive.sources()

    @router.get("/replay/v1/timeline/{source}")
    async def timeline(source: str):
        return archive.timeline(source)

    @router.get("/replay/v1/frame/{source}")
    async def frame(source: str, at: str):
        instant = parse_time(at)
        for slug in archive.scenarios:
            camera = archive.camera(slug, source)
            start = timestamp_us(camera["start"])
            if start <= instant < timestamp_us(camera["end"]):
                frame_index = (instant - start) * camera["fps"] // 1_000_000
                seconds = frame_index / camera["fps"]
                path = archive.directory / slug / source / "video.mp4"
                process = await asyncio.create_subprocess_exec(
                    "ffmpeg", "-nostdin", "-loglevel", "error",
                    "-ss", f"{max(0, seconds - 0.0001):.6f}", "-i", str(path),
                    "-frames:v", "1", "-f", "image2pipe", "-c:v", "mjpeg", "pipe:1",
                    stdout=asyncio.subprocess.PIPE, stderr=asyncio.subprocess.DEVNULL,
                )
                try:
                    jpeg, _ = await asyncio.wait_for(process.communicate(), timeout=8)
                except (TimeoutError, asyncio.CancelledError):
                    if process.returncode is None:
                        process.kill()
                    await process.wait()
                    raise
                if process.returncode or not jpeg:
                    raise HTTPException(502, "Could not decode recorded frame")
                return Response(jpeg, media_type="image/jpeg", headers={
                    "X-Captured-At": utc(start + round(seconds * 1_000_000)),
                    "X-Frame-Width": str(camera["width"]), "X-Frame-Height": str(camera["height"]),
                })
        raise HTTPException(404, "No recorded frame at that time")

    @router.get("/replay/v1/clip/{source}")
    async def clip(source: str, start: str, end: str):
        begin, stop = parse_time(start), parse_time(end)
        if not 0 < stop - begin <= 120_000_000:
            raise HTTPException(422, "Expected a positive interval of at most 120 seconds")
        for slug in archive.scenarios:
            camera = archive.camera(slug, source)
            if not (timestamp_us(camera["start"]) <= begin < timestamp_us(camera["end"])):
                continue
            segments = camera["segments"]
            if stop > timestamp_us(camera["end"]):
                raise HTTPException(404, "Clip extends outside recorded coverage")
            first = next(i for i, item in enumerate(segments) if timestamp_us(item["end"]) > begin)
            last = next(i for i, item in enumerate(segments[first:], first)
                        if timestamp_us(item["end"]) >= stop)
            return {"id": f"{archive.revision[:12]}-{slug}-{source}-{first}-{last}",
                    "source_id": source, "requested_range": {"start": start, "end": end},
                    "effective_range": {"start": segments[first]["start"], "end": segments[last]["end"]},
                    "codec": "h264", "media_type": "application/vnd.apple.mpegurl",
                    "path": f"/media/video-clips/{archive.revision[:12]}-{slug}-{source}-{first}-{last}/"}
        raise HTTPException(404, "No clip at that time")

    @router.get("/replay/v1/media/{clip_id}/{filename}")
    async def media(clip_id: str, filename: str):
        match = re.fullmatch(r"([0-9a-f]{12})-([a-z_]+)-(top|side)-(\d{1,5})-(\d{1,5})", clip_id)
        if not match or match[1] != archive.revision[:12]:
            raise HTTPException(404, "Unknown clip")
        _, slug, angle, first, last = match.groups()
        camera = archive.camera(slug, angle)
        first, last = int(first), int(last)
        if not 0 <= first <= last < len(camera["segments"]) or last - first > 120:
            raise HTTPException(404, "Unknown clip")
        segments = camera["segments"][first:last + 1]
        if filename == "index.m3u8":
            durations = [(timestamp_us(s["end"]) - timestamp_us(s["start"])) / 1_000_000 for s in segments]
            lines = [
                "#EXTM3U", "#EXT-X-VERSION:7", "#EXT-X-PLAYLIST-TYPE:VOD",
                f"#EXT-X-TARGETDURATION:{math.ceil(max(durations))}", "#EXT-X-MEDIA-SEQUENCE:0",
                "#EXT-X-INDEPENDENT-SEGMENTS", '#EXT-X-MAP:URI="init.mp4"',
            ]
            for segment, duration in zip(segments, durations):
                lines.extend([f"#EXT-X-PROGRAM-DATE-TIME:{segment['start']}",
                              f"#EXTINF:{duration:.6f},", segment["file"]])
            return Response("\n".join([*lines, "#EXT-X-ENDLIST", ""]), media_type="application/vnd.apple.mpegurl")
        if filename != "init.mp4" and filename not in {segment["file"] for segment in segments}:
            raise HTTPException(404, "Unknown clip asset")
        return FileResponse(archive.directory / slug / angle / filename,
                            media_type="video/mp4" if filename == "init.mp4" else "video/iso.segment")

    app.include_router(router)

    @app.middleware("http")
    async def recorded_history(request: Request, call_next):
        parts = request.url.path.strip("/").split("/")
        collection = parts[0]
        if collection not in ("runs", "protocols"):
            return await call_next(request)
        records = archive.runs if collection == "runs" else archive.protocols
        sample = request.headers.get("X-OT2-Sample")
        if request.method == "POST" and parts == ["runs"] and sample is not None:
            scenario = archive.scenarios.get(sample)
            if not scenario:
                return JSONResponse({"errors": [{"detail": "Unknown sample run"}]}, status_code=422)
            token = sample_pacing.set(SamplePacing(scenario["commands"]))
            try:
                response = await call_next(request)
                if response.status_code == 201:
                    response.headers["X-OT2-Sample"] = sample
                return response
            finally:
                sample_pacing.reset(token)
        if request.method == "GET" and len(parts) == 1:
            response = await call_next(request)
            if response.status_code != 200:
                return response
            payload = json.loads(b"".join([chunk async for chunk in response.body_iterator]))
            items = payload["data"]
            known = {item["id"] for item in items}
            items.extend(
                (archive.run_record(item) if collection == "runs" else item)
                for key, item in records.items() if key not in known
            )
            if collection == "runs":
                items.sort(key=lambda item: item["createdAt"], reverse=True)
            return JSONResponse(payload)
        if len(parts) < 2 or parts[1] not in records:
            return await call_next(request)
        if request.method != "GET":
            return JSONResponse({"errors": [{"detail": "Recorded history is read-only"}]}, status_code=405)
        item = records[parts[1]]
        if len(parts) == 2:
            return JSONResponse({"data": archive.run_record(item) if collection == "runs" else item})
        if collection == "runs":
            commands = item["commands"]
            if parts[2:] == ["commandsAsPreSerializedList"]:
                return JSONResponse({"data": [json.dumps(command) for command in commands]})
            if parts[2:] == ["commands"]:
                cursor = request.query_params.get("cursor")
                try:
                    return JSONResponse(command_page(
                        commands, int(cursor) if cursor is not None else None,
                        min(1000, max(1, int(request.query_params.get("pageLength", "50")))),
                    ))
                except ValueError:
                    return JSONResponse({"errors": [{"detail": "Invalid command page"}]}, status_code=422)
            if len(parts) == 4 and parts[2] == "commands":
                for command in commands:
                    if command["id"] == parts[3]:
                        return JSONResponse({"data": command})
        return await call_next(request)


if archive_directory := os.getenv("OT2_REPLAY_ARCHIVE"):
    register(Replay(Path(archive_directory)))
