//! Native and injected capture session negotiation.

use std::sync::Mutex;

use a2a_lab_dev_kit::A2aLabError;
use a2a_lab_dev_kit::VideoSourceId;
use a2a_lab_dev_kit::videos::{
    CaptureFormatPreference, CaptureSession, ColorCamera, format_capture_format_label,
    format_capture_preference_label,
};

use crate::videos::nokhwa_camera::NokhwaColorCamera;
use crate::videos::session_capture::SessionColorCapture;
use crate::videos::source::{LiveCameraInputFormat, LiveVideoSourceDescriptor};

/// Opens a capture profile that matches the configured constraints.
pub fn open_configured_session<C: ColorCamera>(
    camera: C,
    descriptor: &LiveVideoSourceDescriptor,
    fps: u32,
) -> Result<CaptureSession<C>, A2aLabError> {
    let request = descriptor.capture_request(fps)?;
    let mut session = CaptureSession::new(camera, request);
    session.open()?;
    log_negotiated_capture(descriptor, &session);
    Ok(session)
}

/// Logs requested and negotiated capture parameters without changing delivery codec.
pub fn log_negotiated_capture<C: ColorCamera>(
    descriptor: &LiveVideoSourceDescriptor,
    session: &CaptureSession<C>,
) {
    let Some(selected) = session.selected() else {
        return;
    };
    let preference = descriptor.input.capture_preference();
    let requested = if preference == CaptureFormatPreference::Auto {
        "auto".to_owned()
    } else {
        format_capture_preference_label(preference).to_owned()
    };
    let capture = selected.capture_rate();
    let capture_fps = if capture.denominator() == 1 {
        capture.numerator().to_string()
    } else {
        format!("{}/{}", capture.numerator(), capture.denominator())
    };
    let requested_size = match (descriptor.width, descriptor.height) {
        (Some(width), Some(height)) => format!("{width}x{height}"),
        _ => "max".to_owned(),
    };
    let requested_rate = descriptor
        .capture_rate
        .map_or_else(|| "min".to_owned(), |rate| rate.to_string());
    eprintln!(
        "{}: requested {requested} {requested_size}@{requested_rate}fps output {}fps; negotiated {} {}x{} capture {capture_fps}fps output {}fps",
        descriptor.id,
        selected.output_rate().numerator(),
        format_capture_format_label(selected.mode().format()),
        selected.mode().width(),
        selected.mode().height(),
        selected.output_rate().numerator(),
    );
}

fn native_frame_format(descriptor: &LiveVideoSourceDescriptor) -> nokhwa::utils::FrameFormat {
    match descriptor.input {
        LiveCameraInputFormat::Auto | LiveCameraInputFormat::Mjpeg => {
            nokhwa::utils::FrameFormat::MJPEG
        }
        LiveCameraInputFormat::Yuy2 => nokhwa::utils::FrameFormat::YUYV,
    }
}

/// Opens one native indexed session for the bounded drain runner.
pub(crate) fn open_native_session_capture(
    descriptor: &LiveVideoSourceDescriptor,
    source_id: VideoSourceId,
    fps: u32,
) -> Result<SessionColorCapture<NokhwaColorCamera>, A2aLabError> {
    // AVFoundation aborts if two devices are configured at the same time.
    static OPEN: Mutex<()> = Mutex::new(());
    let _guard = OPEN
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let format = native_frame_format(descriptor);
    let camera = NokhwaColorCamera::open_index(descriptor.device_index, format)?;
    SessionColorCapture::open(camera, descriptor, source_id, fps)
}
