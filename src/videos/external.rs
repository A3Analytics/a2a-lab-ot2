//! Continuous MJPEG from the configured native external camera.

use std::time::Duration;

use a2a_lab_dev_kit::A2aLabError;
use nokhwa::Camera;
use nokhwa::utils::{CameraFormat, CameraIndex, FrameFormat, RequestedFormat, RequestedFormatType};

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
                "external camera index {index}: exact MJPEG {}x{} at {} fps is unavailable ({error})",
                crate::camera::EXTERNAL_CAMERA_WIDTH,
                crate::camera::EXTERNAL_CAMERA_HEIGHT,
                crate::camera::EXTERNAL_CAMERA_FPS
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

    /// Opens `index` at the closest MJPEG mode to 1280×720 at 5 fps.
    pub fn open_mjpeg(index: u32, max_bytes: u64) -> Result<Self, A2aLabError> {
        let wanted = CameraFormat::new_from(1280, 720, FrameFormat::MJPEG, 5);
        let request = RequestedFormat::with_formats(
            RequestedFormatType::Closest(wanted),
            &[FrameFormat::MJPEG],
        );
        let mut camera = Camera::new(CameraIndex::Index(index), request).map_err(|error| {
            A2aLabError::unavailable(format!(
                "camera index {index}: MJPEG is unavailable ({error})"
            ))
        })?;
        let format = camera.camera_format();
        if format.format() != FrameFormat::MJPEG {
            return Err(A2aLabError::unavailable(format!(
                "camera index {index}: backend selected {format}"
            )));
        }
        camera.open_stream().map_err(|error| {
            A2aLabError::unavailable(format!(
                "camera index {index}: native backend capture failed ({error})"
            ))
        })?;
        eprintln!(
            "camera index {index}: {}x{} MJPEG at {} fps",
            format.width(),
            format.height(),
            format.frame_rate()
        );
        Ok(Self {
            camera,
            capture: NativeCameraCapture { max_bytes },
        })
    }

    fn read(&mut self) -> Result<Vec<u8>, A2aLabError> {
        self.capture.read_open(&mut self.camera).map_err(live_error)
    }

    pub(crate) fn next_frame(&mut self) -> Result<Vec<u8>, A2aLabError> {
        let bytes = self.read()?;
        remember_external_frame(bytes.clone());
        Ok(bytes)
    }

    /// Reads one JPEG without storing it as the external-camera still.
    pub(crate) fn read_jpeg(&mut self) -> Result<Vec<u8>, A2aLabError> {
        self.read()
    }
}

fn live_error(error: CameraCaptureError) -> A2aLabError {
    match error {
        CameraCaptureError::ExactFormatUnavailable(detail) => A2aLabError::unavailable(format!(
            "external camera: exact MJPEG {}x{} at {} fps is unavailable ({detail})",
            crate::camera::EXTERNAL_CAMERA_WIDTH,
            crate::camera::EXTERNAL_CAMERA_HEIGHT,
            crate::camera::EXTERNAL_CAMERA_FPS
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
