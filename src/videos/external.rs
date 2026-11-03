//! Continuous MJPEG from the configured native external camera.

use std::time::Duration;

use a2a_lab_dev_kit::A2aLabError;
use nokhwa::Camera;
use nokhwa::utils::{CameraIndex, FrameFormat, RequestedFormat, RequestedFormatType};

use crate::camera::{
    CameraCaptureError, EXTERNAL_CAMERA_SETTLING_FRAMES, ExternalCameraFormat, NativeCameraCapture,
    remember_external_frame,
};

/// Pulls encoded JPEG frames for the live video pump.
pub trait LiveJpegSource: Send {
    /// Returns one encoded JPEG.
    fn next_jpeg(&mut self) -> Result<Vec<u8>, A2aLabError>;
}

/// Native Nokhwa session that stays open after the still-capture settle period.
pub struct NativeLiveJpeg {
    camera: Camera,
    capture: NativeCameraCapture,
}

impl NativeLiveJpeg {
    /// Opens `index` at the required MJPEG format and discards settling frames.
    pub fn open(index: u32, max_bytes: u64) -> Result<Self, A2aLabError> {
        let exact = ExternalCameraFormat::REQUIRED.nokhwa();
        let request =
            RequestedFormat::with_formats(RequestedFormatType::Exact(exact), &[FrameFormat::MJPEG]);
        let mut camera = Camera::new(CameraIndex::Index(index), request).map_err(|error| {
            A2aLabError::unavailable(format!(
                "external camera index {index}: exact MJPEG 3840x2160 at 30 fps is unavailable ({error})"
            ))
        })?;
        if camera.camera_format() != exact {
            return Err(A2aLabError::unavailable(format!(
                "external camera index {index}: backend selected {} instead of {exact}",
                camera.camera_format()
            )));
        }
        camera.open_stream().map_err(|error| {
            A2aLabError::unavailable(format!(
                "external camera index {index}: native backend capture failed ({error})"
            ))
        })?;
        let mut session = Self {
            camera,
            capture: NativeCameraCapture { max_bytes },
        };
        for _ in 1..EXTERNAL_CAMERA_SETTLING_FRAMES {
            let _ = session.read()?;
        }
        Ok(session)
    }

    fn read(&mut self) -> Result<Vec<u8>, A2aLabError> {
        self.capture.read_open(&mut self.camera).map_err(live_error)
    }

    pub(crate) fn next_frame(&mut self) -> Result<Vec<u8>, A2aLabError> {
        let bytes = self.read()?;
        remember_external_frame(bytes.clone());
        Ok(bytes)
    }
}

fn live_error(error: CameraCaptureError) -> A2aLabError {
    match error {
        CameraCaptureError::ExactFormatUnavailable(detail) => A2aLabError::unavailable(format!(
            "external camera: exact MJPEG 3840x2160 at 30 fps is unavailable ({detail})"
        )),
        CameraCaptureError::Backend(detail) => A2aLabError::unavailable(format!(
            "external camera: native backend capture failed ({detail})"
        )),
        CameraCaptureError::Empty => {
            A2aLabError::invalid("camera", "external camera returned an empty frame")
        }
        CameraCaptureError::Oversize => A2aLabError::invalid(
            "camera",
            "external camera returned a frame larger than the configured maximum",
        ),
    }
}

/// Sleeps between live frames so the catalog stays near `fps`.
#[must_use]
pub fn frame_budget(fps: u32) -> Duration {
    Duration::from_millis(1_000 / u64::from(fps.max(1)))
}
