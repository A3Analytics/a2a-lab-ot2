//! Shared capture session: one pipeline, RGB-only stereo gaps.

use std::collections::VecDeque;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use a2a_lab_ot2::cameras::{SharedSession, StreamingDevice};
use a2a_lab_ot2::frame_sets::{
    CaptureRequest, CapturedFrameSet, ChannelCalibration, ChannelProfile, ChannelRole, DepthScale,
    DepthUnit, FrameChannel, FrameChannelId, FrameChannelParts, FrameSetCapture, FrameSetError,
    FrameSetSource, FrameSetSourceId, frame_channel,
};
use image::ImageEncoder;
use serde_json::Map;

struct Script {
    source: FrameSetSource,
    opens: AtomicU64,
    frames: Mutex<VecDeque<CapturedFrameSet>>,
}

impl Script {
    fn new(source: FrameSetSource, frames: Vec<CapturedFrameSet>) -> Arc<Self> {
        Arc::new(Self {
            source,
            opens: AtomicU64::new(0),
            frames: Mutex::new(frames.into()),
        })
    }

    fn opens(&self) -> u64 {
        self.opens.load(Ordering::SeqCst)
    }
}

impl StreamingDevice for Script {
    fn describe(&self) -> Result<FrameSetSource, FrameSetError> {
        self.opens.fetch_add(1, Ordering::SeqCst);
        Ok(self.source.clone())
    }

    fn next(&self, _request: &CaptureRequest) -> Result<CapturedFrameSet, FrameSetError> {
        if self.opens() == 0 {
            self.opens.fetch_add(1, Ordering::SeqCst);
        }
        self.frames
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .pop_front()
            .ok_or_else(|| FrameSetError::Timeout {
                device: self.source.serial().to_owned(),
                detail: "no scripted frame".to_owned(),
            })
    }
}

fn jpeg() -> Vec<u8> {
    vec![0xFF, 0xD8, 0xFF, 0xD9]
}

fn png(bit_depth: u8) -> Vec<u8> {
    let mut bytes = Vec::new();
    let samples: Vec<u8> = if bit_depth == 16 { vec![0, 1] } else { vec![0] };
    let color = if bit_depth == 16 {
        image::ExtendedColorType::L16
    } else {
        image::ExtendedColorType::L8
    };
    image::codecs::png::PngEncoder::new(&mut bytes)
        .write_image(&samples, 1, 1, color)
        .unwrap();
    bytes
}

fn profile(id: &str, role: ChannelRole, media: &str, depth: u8) -> ChannelProfile {
    ChannelProfile::new(
        FrameChannelId::new(id).unwrap(),
        role,
        media,
        1,
        1,
        depth,
        5.0,
    )
    .unwrap()
}

fn stereo_source() -> FrameSetSource {
    FrameSetSource::new(
        FrameSetSourceId::new("shared-camera").unwrap(),
        "generic",
        "ABC",
        "stereo",
        vec![
            profile("color", ChannelRole::color(), "image/jpeg", 8),
            profile("depth", ChannelRole::depth(), "image/png", 16),
            profile(
                "infrared-left",
                ChannelRole::infrared_left(),
                "image/png",
                8,
            ),
            profile(
                "infrared-right",
                ChannelRole::infrared_right(),
                "image/png",
                8,
            ),
        ],
        "device:ABC",
        100_000,
    )
    .unwrap()
}

fn channel(
    id: &str,
    role: ChannelRole,
    media: &str,
    depth: u8,
    data: Vec<u8>,
    timestamp: u64,
    scale: Option<DepthScale>,
) -> FrameChannel {
    frame_channel(FrameChannelParts {
        id: FrameChannelId::new(id).unwrap(),
        role,
        media_type: media.to_owned(),
        width: 1,
        height: 1,
        bit_depth: depth,
        device_timestamp_us: timestamp,
        frame_number: 1,
        depth: scale,
        calibration: ChannelCalibration::default(),
        attributes: Map::new(),
        data,
    })
}

fn color_only(source: &FrameSetSource, timestamp: u64) -> CapturedFrameSet {
    CapturedFrameSet::new(
        source,
        "2026-11-05T00:00:00Z",
        vec![channel(
            "color",
            ChannelRole::color(),
            "image/jpeg",
            8,
            jpeg(),
            timestamp,
            None,
        )],
        &[FrameChannelId::new("color").unwrap()],
        Map::new(),
        1024,
    )
    .unwrap()
}

fn stereo_set(source: &FrameSetSource, timestamp: u64) -> CapturedFrameSet {
    let scale = DepthScale::new(1.0, DepthUnit::Millimeter).unwrap();
    CapturedFrameSet::new(
        source,
        "2026-11-05T00:00:00Z",
        vec![
            channel(
                "color",
                ChannelRole::color(),
                "image/jpeg",
                8,
                jpeg(),
                timestamp,
                None,
            ),
            channel(
                "depth",
                ChannelRole::depth(),
                "image/png",
                16,
                png(16),
                timestamp,
                Some(scale),
            ),
            channel(
                "infrared-left",
                ChannelRole::infrared_left(),
                "image/png",
                8,
                png(8),
                timestamp,
                None,
            ),
            channel(
                "infrared-right",
                ChannelRole::infrared_right(),
                "image/png",
                8,
                png(8),
                timestamp,
                None,
            ),
        ],
        &[],
        Map::new(),
        4096,
    )
    .unwrap()
}

#[test]
fn shared_consumers_use_one_pipeline() {
    let source = stereo_source();
    let frame = stereo_set(&source, 1_000);
    let device = Script::new(source.clone(), vec![frame.clone(), frame]);
    let session = SharedSession::new(device.clone());
    let request = CaptureRequest::new(source.id().clone(), Vec::new(), 1_000, 4096).unwrap();
    assert_eq!(session.sources().unwrap()[0].serial(), "ABC");
    assert_eq!(
        session.capture(&request).unwrap().color_jpeg(),
        Some(jpeg().as_slice())
    );
    assert_eq!(
        session.color_jpeg(Duration::from_millis(1)).unwrap(),
        jpeg()
    );
    assert_eq!(session.pipeline_starts(), 1);
    assert_eq!(device.opens(), 1);
}

#[test]
fn rgb_fallback_records_stereo_gap() {
    let source = stereo_source();
    let device = Script::new(
        source.clone(),
        vec![color_only(&source, 1_000), stereo_set(&source, 2_000)],
    );
    let session = SharedSession::new(device);
    let color_req = CaptureRequest::new(
        source.id().clone(),
        vec![FrameChannelId::new("color").unwrap()],
        1_000,
        4096,
    )
    .unwrap();
    session.pump_once(&color_req).unwrap();
    let gaps = session.stereo_gaps();
    assert_eq!(gaps.len(), 1);
    assert_eq!(gaps[0].started_us, 1_000);
    assert_eq!(gaps[0].ended_us, None);
    session
        .pump_once(&CaptureRequest::new(source.id().clone(), Vec::new(), 1_000, 4096).unwrap())
        .unwrap();
    assert_eq!(session.stereo_gaps()[0].ended_us, Some(2_000));
}
