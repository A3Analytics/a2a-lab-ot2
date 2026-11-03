//! Portable frame-set contract.

use std::fs;
use std::path::Path;
use std::sync::Mutex;

use a2a_lab_ot2::frame_sets::{
    CaptureRequest, ChannelCalibration, ChannelProfile, ChannelRole, DepthScale, DepthUnit,
    Distortion, Extrinsics, FrameChannelId, FrameChannelParts, FrameSet, FrameSetCapture,
    FrameSetError, FrameSetId, FrameSetSource, FrameSetSourceId, FrameSetStore, Intrinsics,
    MemoryFrameSetStore, frame_channel, publish_capture,
};
use serde_json::json;

fn id_source(value: &str) -> FrameSetSourceId {
    FrameSetSourceId::new(value).unwrap()
}

fn id_channel(value: &str) -> FrameChannelId {
    FrameChannelId::new(value).unwrap()
}

fn id_set(value: &str) -> FrameSetId {
    FrameSetId::new(value).unwrap()
}

fn camera(channels: Vec<ChannelProfile>) -> Result<FrameSetSource, FrameSetError> {
    FrameSetSource::new(
        id_source("cam-1"),
        "generic",
        "serial-1",
        "stereo-camera",
        channels,
        "device:serial-1",
        1_000,
    )
}

fn color_profile() -> ChannelProfile {
    ChannelProfile::new(
        id_channel("color"),
        ChannelRole::color(),
        "image/jpeg",
        2,
        2,
        8,
        30.0,
    )
    .unwrap()
}

fn depth_profile() -> ChannelProfile {
    ChannelProfile::new(
        id_channel("depth"),
        ChannelRole::depth(),
        "image/png",
        2,
        1,
        16,
        30.0,
    )
    .unwrap()
}

fn jpeg() -> Vec<u8> {
    vec![0xFF, 0xD8, 0xFF, 0xD9]
}

fn png(width: u32, height: u32, bit_depth: u8) -> Vec<u8> {
    let mut data = b"\x89PNG\r\n\x1a\n".to_vec();
    data.extend_from_slice(&13u32.to_be_bytes());
    data.extend_from_slice(b"IHDR");
    data.extend_from_slice(&width.to_be_bytes());
    data.extend_from_slice(&height.to_be_bytes());
    data.extend_from_slice(&[bit_depth, 0, 0, 0, 0]);
    data.extend_from_slice(&[0, 0, 0, 0]);
    data
}

fn channel(
    id: &str,
    role: ChannelRole,
    media_type: &str,
    width: u32,
    height: u32,
    bit_depth: u8,
    timestamp: u64,
    depth: Option<DepthScale>,
    data: Vec<u8>,
) -> a2a_lab_ot2::frame_sets::FrameChannel {
    frame_channel(FrameChannelParts {
        id: id_channel(id),
        role,
        media_type: media_type.to_owned(),
        width,
        height,
        bit_depth,
        device_timestamp_us: timestamp,
        frame_number: 7,
        depth,
        calibration: ChannelCalibration::new(
            Some(Intrinsics::new(100.0, 100.0, 1.0, 1.0, width, height).unwrap()),
            Some(Distortion::new("brown-conrady", vec![0.0, 0.1]).unwrap()),
            Some(
                Extrinsics::new(
                    [1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0],
                    [0.0, 0.0, 0.0],
                    id_channel("color"),
                )
                .unwrap(),
            ),
        ),
        attributes: json!({"firmware": "1"}).as_object().cloned().unwrap(),
        data,
    })
}

fn mixed_source() -> FrameSetSource {
    camera(vec![color_profile(), depth_profile()]).unwrap()
}

fn mixed_set(timestamps: (u64, u64)) -> FrameSet {
    let source = mixed_source();
    FrameSet::new(
        id_set("set-1"),
        &source,
        "2026-10-30T00:00:00Z",
        vec![
            channel(
                "color",
                ChannelRole::color(),
                "image/jpeg",
                2,
                2,
                8,
                timestamps.0,
                None,
                jpeg(),
            ),
            channel(
                "depth",
                ChannelRole::depth(),
                "image/png",
                2,
                1,
                16,
                timestamps.1,
                Some(DepthScale::new(1.0, DepthUnit::Millimeter).unwrap()),
                png(2, 1, 16),
            ),
        ],
        &[],
        json!({"pipeline": "sync"}).as_object().cloned().unwrap(),
        1024,
    )
    .unwrap()
}

#[test]
fn identifiers_reject_unsafe_text_and_round_trip() {
    for value in ["", " ", "has space", "../x", "a/b", "ü", &"a".repeat(129)] {
        assert!(FrameSetSourceId::new(value).is_err(), "{value}");
        assert!(FrameChannelId::new(value).is_err(), "{value}");
        assert!(FrameSetId::new(value).is_err(), "{value}");
    }
    let source = id_source("cam.left:1");
    let channel = id_channel("infrared-left");
    let set = id_set("set_1");
    let restored: (FrameSetSourceId, FrameChannelId, FrameSetId) = serde_json::from_value(
        serde_json::to_value((source.clone(), channel.clone(), set.clone())).unwrap(),
    )
    .unwrap();
    assert_eq!(restored.0.as_str(), source.as_str());
    assert_eq!(restored.1.as_str(), channel.as_str());
    assert_eq!(restored.2.as_str(), set.as_str());
}

#[test]
fn source_describes_device_profiles_without_vendor_fields() {
    let source = mixed_source();
    let value = serde_json::to_value(&source).unwrap();
    assert_eq!(value["backend"], "generic");
    assert_eq!(value["serial"], "serial-1");
    assert_eq!(value["model"], "stereo-camera");
    assert_eq!(value["sync_group"], "device:serial-1");
    assert_eq!(value["channels"][0]["media_type"], "image/jpeg");
    assert_eq!(value["channels"][1]["frame_rate_hz"], 30.0);
    let text = value.to_string();
    assert!(!text.contains("orbbec"));
    assert!(!text.contains("realsense"));
}

#[test]
fn known_and_custom_roles_share_one_string_shape() {
    for role in [
        ChannelRole::color(),
        ChannelRole::depth(),
        ChannelRole::infrared_left(),
        ChannelRole::infrared_right(),
        ChannelRole::confidence(),
        ChannelRole::new("raw-phase").unwrap(),
    ] {
        let value = serde_json::to_value(&role).unwrap();
        assert!(value.is_string());
        let restored: ChannelRole = serde_json::from_value(value).unwrap();
        assert_eq!(restored, role);
    }
    assert!(ChannelRole::new("Depth").is_err());
    assert!(ChannelRole::new("").is_err());
}

#[test]
fn mixed_set_accepts_jpeg_and_sixteen_bit_png_and_rejects_bad_sets() {
    let set = mixed_set((100, 200));
    assert_eq!(set.channels().len(), 2);
    let scale = set
        .channel(&id_channel("depth"))
        .unwrap()
        .depth()
        .unwrap()
        .scale();
    assert!((scale - 1.0).abs() < f64::EPSILON);
    let source = mixed_source();
    let mut channels = set.channels().to_vec();
    channels.pop();
    let partial = FrameSet::new(
        id_set("partial"),
        &source,
        "2026-10-30T00:00:00Z",
        channels,
        &[],
        serde_json::Map::new(),
        1024,
    );
    assert_eq!(partial.unwrap_err().class(), "invalid");
    let duplicate = camera(vec![color_profile(), color_profile()]);
    assert!(duplicate.is_err());
    let bad_payload = FrameSet::new(
        id_set("bad"),
        &source,
        "2026-10-30T00:00:00Z",
        vec![
            channel(
                "color",
                ChannelRole::color(),
                "image/jpeg",
                2,
                2,
                8,
                100,
                None,
                png(2, 2, 8),
            ),
            channel(
                "depth",
                ChannelRole::depth(),
                "image/png",
                2,
                1,
                16,
                100,
                Some(DepthScale::new(1.0, DepthUnit::Millimeter).unwrap()),
                png(2, 1, 16),
            ),
        ],
        &[],
        serde_json::Map::new(),
        1024,
    );
    assert_eq!(bad_payload.unwrap_err().class(), "malformed");
}

#[test]
fn metadata_and_malformed_calibration_are_typed() {
    let set = mixed_set((10, 10));
    let metadata = set.metadata();
    assert!(metadata.channels()[1].depth().is_some());
    let focal = metadata.channels()[0]
        .calibration()
        .intrinsics()
        .unwrap()
        .fx;
    assert!((focal - 100.0).abs() < f64::EPSILON);
    assert_eq!(metadata.channels()[0].frame_number(), 7);
    assert!(Intrinsics::new(f64::NAN, 1.0, 0.0, 0.0, 1, 1).is_err());
    assert!(DepthScale::new(0.0, DepthUnit::Meter).is_err());
    assert!(Distortion::new("", vec![1.0]).is_err());
    let text = serde_json::to_value(metadata).unwrap().to_string();
    assert!(!text.contains("orbbec"));
    assert!(!text.contains("realsense"));
}

struct OneCapture {
    source: FrameSetSource,
    set: FrameSet,
}

impl FrameSetCapture for OneCapture {
    fn sources(&self) -> Result<Vec<FrameSetSource>, FrameSetError> {
        Ok(vec![self.source.clone()])
    }

    fn capture(
        &self,
        request: &CaptureRequest,
    ) -> Result<a2a_lab_ot2::frame_sets::CapturedFrameSet, FrameSetError> {
        a2a_lab_ot2::frame_sets::CapturedFrameSet::new(
            &self.source,
            self.set.captured_at(),
            self.set.channels().to_vec(),
            request.channel_ids(),
            self.set.attributes().clone(),
            request.max_channel_bytes(),
        )
    }
}

struct RollbackStore {
    inner: MemoryFrameSetStore,
    fail: Mutex<bool>,
}

impl FrameSetStore for RollbackStore {
    async fn publish(&self, set: &FrameSet) -> Result<(), FrameSetError> {
        if *self.fail.lock().unwrap() {
            return Err(FrameSetError::Unavailable("write failed".to_owned()));
        }
        self.inner.publish(set).await
    }

    async fn read(&self, id: &FrameSetId) -> Result<FrameSet, FrameSetError> {
        self.inner.read(id).await
    }
}

#[tokio::test]
async fn publication_keeps_a_complete_set_or_nothing() {
    let source = mixed_source();
    let set = mixed_set((10, 12));
    let capture = OneCapture {
        source: source.clone(),
        set: set.clone(),
    };
    let store = RollbackStore {
        inner: MemoryFrameSetStore::new(),
        fail: Mutex::new(true),
    };
    let request = CaptureRequest::new(source.id().clone(), Vec::new(), 1000, 1024).unwrap();
    let failed = publish_capture(&capture, &store, &source, &request, id_set("set-1")).await;
    assert_eq!(failed.unwrap_err().class(), "unavailable");
    assert_eq!(
        store.read(&id_set("set-1")).await.unwrap_err().class(),
        "not_found"
    );
    *store.fail.lock().unwrap() = false;
    let published = publish_capture(&capture, &store, &source, &request, id_set("set-1"))
        .await
        .unwrap();
    assert_eq!(published.channels()[0].data(), jpeg());
    assert_eq!(
        store.read(&id_set("set-1")).await.unwrap().channels()[1].data(),
        png(2, 1, 16)
    );
    let rejected = FrameSet::new(
        id_set("late"),
        &source,
        "2026-10-30T00:00:00Z",
        vec![
            channel(
                "color",
                ChannelRole::color(),
                "image/jpeg",
                2,
                2,
                8,
                0,
                None,
                jpeg(),
            ),
            channel(
                "depth",
                ChannelRole::depth(),
                "image/png",
                2,
                1,
                16,
                50_000,
                Some(DepthScale::new(0.001, DepthUnit::Meter).unwrap()),
                png(2, 1, 16),
            ),
        ],
        &[],
        serde_json::Map::new(),
        1024,
    );
    assert_eq!(rejected.unwrap_err().class(), "unsynchronized");
}

#[test]
fn frame_sets_module_imports_only_the_portable_boundary() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("src/frame_sets");
    let mut files = Vec::new();
    collect_rs(&root, &mut files);
    assert!(files.len() >= 4);
    for path in files {
        let text = fs::read_to_string(&path).unwrap();
        for line in text.lines() {
            let trimmed = line.trim();
            if !trimmed.starts_with("use ") && !trimmed.starts_with("extern ") {
                continue;
            }
            let lower = trimmed.to_ascii_lowercase();
            for forbidden in [
                "a2a_lab_dev_kit",
                "crate::opentrons",
                "crate::orbbec",
                "realsense",
                "librealsense",
                "rmcp",
            ] {
                assert!(
                    !lower.contains(forbidden),
                    "{} imports {forbidden}",
                    path.display()
                );
            }
        }
    }
}

fn collect_rs(path: &Path, files: &mut Vec<std::path::PathBuf>) {
    for entry in fs::read_dir(path).unwrap() {
        let entry = entry.unwrap();
        let path = entry.path();
        if path.is_dir() {
            collect_rs(&path, files);
        } else if path.extension().is_some_and(|extension| extension == "rs") {
            files.push(path);
        }
    }
}

#[test]
fn custom_role_profile_round_trips() {
    let profile = ChannelProfile::new(
        id_channel("aux"),
        ChannelRole::new("raw-phase").unwrap(),
        "image/png",
        1,
        1,
        8,
        15.5,
    )
    .unwrap();
    let source = camera(vec![profile]).unwrap();
    let set = FrameSet::new(
        id_set("custom"),
        &source,
        "2026-10-30T00:00:01Z",
        vec![channel(
            "aux",
            ChannelRole::new("raw-phase").unwrap(),
            "image/png",
            1,
            1,
            8,
            4,
            None,
            png(1, 1, 8),
        )],
        &[],
        serde_json::Map::new(),
        64,
    )
    .unwrap();
    let value = serde_json::to_value(&set).unwrap();
    assert_eq!(value["channels"][0]["role"], "raw-phase");
}
