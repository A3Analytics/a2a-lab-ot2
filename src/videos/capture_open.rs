//! Native and injected capture session negotiation (exact configured profiles only).

use std::sync::Mutex;

use a2a_lab_dev_kit::A2aLabError;
use a2a_lab_dev_kit::VideoSourceId;
use a2a_lab_dev_kit::videos::{
    CaptureClock, CaptureFormat, CaptureIdentity, CaptureSession, CaptureTiming, ColorCamera,
    ColorMatrix, ColorProperties, ColorRange,
};

use crate::time;
use crate::videos::nokhwa_camera::NokhwaColorCamera;
use crate::videos::raw_jpeg::captured_frame_jpeg;
use crate::videos::source::{LiveCameraInputFormat, LiveVideoSourceDescriptor};

/// Opens exactly the configured capture profile. No automatic fallback.
pub fn open_configured_session<C: ColorCamera>(
    camera: C,
    descriptor: &LiveVideoSourceDescriptor,
    fps: u32,
) -> Result<CaptureSession<C>, A2aLabError> {
    let request = descriptor.capture_request(fps)?;
    let mut session = CaptureSession::new(camera, request);
    session.open()?;
    Ok(session)
}

/// Opens a native indexed camera using the configured profile request.
pub fn open_native_session(
    descriptor: &LiveVideoSourceDescriptor,
    fps: u32,
) -> Result<CaptureSession<NokhwaColorCamera>, A2aLabError> {
    // AVFoundation aborts if two devices are configured at the same time.
    static OPEN: Mutex<()> = Mutex::new(());
    let _guard = OPEN.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    let format = match descriptor.input {
        LiveCameraInputFormat::Yuy2 => nokhwa::utils::FrameFormat::YUYV,
        LiveCameraInputFormat::Mjpeg => nokhwa::utils::FrameFormat::MJPEG,
    };
    let camera = NokhwaColorCamera::open_index(
        descriptor.device_index,
        format,
        descriptor.width,
        descriptor.height,
        fps,
    )?;
    open_configured_session(camera, descriptor, fps)
}

pub(crate) fn read_session_jpeg(
    session: &mut CaptureSession<impl ColorCamera>,
    source_id: &VideoSourceId,
    sequence: &mut u64,
    max_bytes: u64,
    descriptor: &LiveVideoSourceDescriptor,
) -> Result<Vec<u8>, A2aLabError> {
    let captured_at = time::now()?;
    let identity = CaptureIdentity::new(source_id.clone(), *sequence);
    *sequence = sequence.saturating_add(1);
    let timing = CaptureTiming::new(captured_at, CaptureClock::monotonic(0));
    let selected = session.selected().ok_or_else(|| {
        A2aLabError::invalid("camera", "capture session has no negotiated profile")
    })?;
    let negotiated = selected.mode().format();
    let color = match descriptor.input {
        LiveCameraInputFormat::Yuy2 => {
            ColorProperties::new(ColorMatrix::Bt601, ColorRange::Limited)
        }
        LiveCameraInputFormat::Mjpeg => {
            ColorProperties::new(ColorMatrix::Unknown, ColorRange::Unknown)
        }
    };
    let frame = session.read(identity, timing, color)?;
    let expected = descriptor.input.capture_preference();
    let matches = match expected {
        a2a_lab_dev_kit::videos::CaptureFormatPreference::Mjpeg => {
            negotiated == CaptureFormat::Mjpeg
        }
        a2a_lab_dev_kit::videos::CaptureFormatPreference::Yuy2 => negotiated == CaptureFormat::Yuy2,
        a2a_lab_dev_kit::videos::CaptureFormatPreference::Auto => true,
    };
    if !matches {
        return Err(A2aLabError::invalid(
            "format",
            "negotiated raw layout does not match the configured input format",
        ));
    }
    if frame.format() != negotiated {
        return Err(A2aLabError::invalid(
            "format",
            "raw sample layout does not match the negotiated input format",
        ));
    }
    captured_frame_jpeg(&frame, max_bytes)
}
