//! Pinned robot-server `GET /camera/stream` contract for OT-2.
//!
//! The endpoint returns JSON URL templates. It is not a frame source. The Flex
//! `opentrons-live-stream` service those templates name is absent on OT-2.

use a2a_lab_dev_kit::A2aLabError;

use super::model::LiveStreamData;

/// Relative HLS playlist advertised by `GET /camera/stream` in robot-server v10.0.0.
pub const OT2_HLS_RELATIVE_PATH: &str = "/hls/stream.m3u";

/// Relative RTMP path advertised by `GET /camera/stream` in robot-server v10.0.0.
pub const OT2_RTMP_RELATIVE_PATH: &str = "/live/stream";

/// Error text used when `POST`/`GET /camera/stream/settings` is called on OT-2.
pub const OT2_STREAM_SETTINGS_UNAVAILABLE: &str =
    "Opentrons Live Stream service is not available on OT-2.";

/// Sanitized success body from pinned robot-server `LiveStreamData`.
pub const STREAM_METADATA_FIXTURE: &str =
    r#"{"enabled":false,"hls":"/hls/stream.m3u","rtmp":"/live/stream"}"#;

/// Same payload wrapped in the JSON:API `data` envelope some robot-server routes use.
pub const STREAM_METADATA_ENVELOPE_FIXTURE: &str =
    r#"{"data":{"enabled":false,"hls":"/hls/stream.m3u","rtmp":"/live/stream"}}"#;

/// Flex-shaped payload with streaming enabled. Still not ingestible on OT-2.
pub const STREAM_METADATA_ENABLED_FIXTURE: &str =
    r#"{"enabled":true,"hls":"/hls/stream.m3u","rtmp":"/live/stream"}"#;

/// Robot-server 503 when `/dev/ot_system_camera` is missing.
pub const STREAM_CAMERA_MISSING_FIXTURE: &str = r#"{"message":"No video device found with device path: /dev/ot_system_camera","errorCode":"4000"}"#;

/// Robot-server 400 for live-stream settings on OT-2.
pub const STREAM_SETTINGS_OT2_FIXTURE: &str =
    r#"{"message":"Opentrons Live Stream service is not available on OT-2.","errorCode":"4000"}"#;

/// True when `info` matches the pinned v10.0.0 URL-template shape.
#[must_use]
pub fn is_stream_metadata(info: &LiveStreamData) -> bool {
    info.hls == OT2_HLS_RELATIVE_PATH && info.rtmp == OT2_RTMP_RELATIVE_PATH
}

/// Rejects OT-2 live video. `GET /camera/stream` is metadata; picture polling is forbidden.
#[must_use]
pub fn reject_native_video_stream(info: &LiveStreamData) -> A2aLabError {
    A2aLabError::unavailable(format!(
        "OT-2 GET /camera/stream returns metadata only (enabled={}, hls={}, rtmp={}); the Flex live-stream service is not present on OT-2 and POST /camera/picture is not a video source",
        info.enabled, info.hls, info.rtmp
    ))
}
