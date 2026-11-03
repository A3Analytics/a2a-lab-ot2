//! Native external-camera capture through Nokhwa.

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;

use a2a_lab_dev_kit::{A2aLabError, ImageSource, ImageSourceId, ImageTransportConfig, JsonObject};
use nokhwa::Camera;
use nokhwa::utils::{CameraFormat, CameraIndex, FrameFormat, RequestedFormat, RequestedFormatType};

use crate::images::{
    CapturedFrame, ImageCapture, ImageCatalogConfig, LiveImageCatalog, jpeg_dimensions,
};
use crate::time::now;

/// Stable image-source id for the external camera.
pub const EXTERNAL_CAMERA_SOURCE_ID: &str = "external-camera";

/// Stable image-source id for the optional second webcam.
pub const CAMERA_SOURCE_ID: &str = "camera";

/// Platform-neutral description advertised when none is configured.
pub const DEFAULT_EXTERNAL_CAMERA_DESCRIPTION: &str =
    "Still frame from the configured external camera.";

/// Required encoded format for external-camera capture.
pub const EXTERNAL_CAMERA_WIDTH: u32 = 3840;
/// Required encoded format for external-camera capture.
pub const EXTERNAL_CAMERA_HEIGHT: u32 = 2160;
/// Required frame rate for external-camera capture.
pub const EXTERNAL_CAMERA_FPS: u32 = 30;
/// Number of frames read for each current-image request.
pub const EXTERNAL_CAMERA_SETTLING_FRAMES: u32 = 30;

/// Format requested from an injectable camera backend.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ExternalCameraFormat {
    /// Encoded image width.
    pub width: u32,
    /// Encoded image height.
    pub height: u32,
    /// Frames per second.
    pub fps: u32,
}

impl ExternalCameraFormat {
    /// Exact MJPEG 3840x2160 at 30 fps.
    pub const REQUIRED: Self = Self {
        width: EXTERNAL_CAMERA_WIDTH,
        height: EXTERNAL_CAMERA_HEIGHT,
        fps: EXTERNAL_CAMERA_FPS,
    };

    pub(crate) fn nokhwa(self) -> CameraFormat {
        CameraFormat::new_from(self.width, self.height, FrameFormat::MJPEG, self.fps)
    }
}

/// Failure from the native capture boundary before JPEG validation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CameraCaptureError {
    /// The backend could not provide the required exact format.
    ExactFormatUnavailable(String),
    /// The backend failed to open or capture.
    Backend(String),
    /// The backend returned no bytes.
    Empty,
    /// The encoded frame exceeded the configured maximum.
    Oversize,
}

/// Captures encoded frames from a platform-native camera index.
pub trait CameraCapture: Send + Sync {
    /// Opens `index`, requests `format`, discards all but the last of `frames`,
    /// and returns the final encoded frame untouched.
    fn capture(
        &self,
        index: u32,
        format: ExternalCameraFormat,
        frames: u32,
    ) -> Result<Vec<u8>, CameraCaptureError>;
}

/// Nokhwa automatic native-backend capture.
#[derive(Debug, Clone, Copy)]
pub struct NativeCameraCapture {
    pub(crate) max_bytes: u64,
}

impl CameraCapture for NativeCameraCapture {
    fn capture(
        &self,
        index: u32,
        format: ExternalCameraFormat,
        frames: u32,
    ) -> Result<Vec<u8>, CameraCaptureError> {
        let exact = format.nokhwa();
        let request =
            RequestedFormat::with_formats(RequestedFormatType::Exact(exact), &[FrameFormat::MJPEG]);
        let mut camera = Camera::new(CameraIndex::Index(index), request)
            .map_err(|error| CameraCaptureError::ExactFormatUnavailable(error.to_string()))?;
        if camera.camera_format() != exact {
            return Err(CameraCaptureError::ExactFormatUnavailable(format!(
                "backend selected {} instead of {exact}",
                camera.camera_format()
            )));
        }
        camera
            .open_stream()
            .map_err(|error| CameraCaptureError::Backend(error.to_string()))?;
        let mut kept = None;
        for frame_number in 1..=frames {
            let frame = camera
                .frame_raw()
                .map_err(|error| CameraCaptureError::Backend(error.to_string()))?;
            if frame_number == frames {
                kept = Some(owned_mjpeg(&frame, self.max_bytes)?);
            }
        }
        kept.ok_or(CameraCaptureError::Empty)
    }
}

static LATEST_EXTERNAL_JPEG: std::sync::Mutex<Option<Vec<u8>>> = std::sync::Mutex::new(None);

/// Stores the newest live JPEG so still capture can reuse the video session.
pub fn remember_external_frame(bytes: Vec<u8>) {
    if let Ok(mut slot) = LATEST_EXTERNAL_JPEG.lock() {
        *slot = Some(bytes);
    }
}

fn latest_external_frame() -> Option<Vec<u8>> {
    LATEST_EXTERNAL_JPEG
        .lock()
        .ok()
        .and_then(|slot| slot.clone())
}

static LATEST_CAMERA_JPEG: std::sync::Mutex<Option<Vec<u8>>> = std::sync::Mutex::new(None);

/// Stores the newest JPEG from the `camera` video pump.
pub fn remember_camera_frame(bytes: Vec<u8>) {
    if let Ok(mut slot) = LATEST_CAMERA_JPEG.lock() {
        *slot = Some(bytes);
    }
}

/// Drops a remembered `camera` JPEG. Tests use this between cases.
pub fn clear_camera_frame() {
    if let Ok(mut slot) = LATEST_CAMERA_JPEG.lock() {
        *slot = None;
    }
}

fn latest_camera_frame() -> Option<Vec<u8>> {
    LATEST_CAMERA_JPEG.lock().ok().and_then(|slot| slot.clone())
}

/// Source descriptor for the optional second webcam.
pub fn camera_source() -> Result<ImageSource, A2aLabError> {
    Ok(ImageSource {
        id: ImageSourceId::new(CAMERA_SOURCE_ID)?,
        name: "Camera".to_owned(),
        description: "Still frame from the configured camera.".to_owned(),
        asset_id: None,
        semantic_id: None,
    })
}

/// Still capture for source `camera`.
///
/// Reads the JPEG stored by the live video pump. This does not open a device.
pub struct CameraStill {
    transport: ImageTransportConfig,
}

impl CameraStill {
    /// Rejects an invalid payload limit. No device is opened.
    pub fn new(max_image_bytes: u64) -> Result<Self, A2aLabError> {
        Ok(Self {
            transport: ImageTransportConfig::new(max_image_bytes)?,
        })
    }

    fn take_frame(&self) -> Result<CapturedFrame, A2aLabError> {
        let Some(body) = latest_camera_frame() else {
            return Err(A2aLabError::unavailable("camera has no frame yet"));
        };
        if body.is_empty() {
            return Err(A2aLabError::invalid(
                "camera",
                "camera returned an empty frame",
            ));
        }
        let (width, height) = jpeg_dimensions(&body).map_err(|_| {
            A2aLabError::invalid("camera", "camera returned a malformed MJPEG frame")
        })?;
        self.transport.check_payload(&body).map_err(|_| {
            A2aLabError::invalid(
                "camera",
                "camera returned a frame larger than the configured maximum",
            )
        })?;
        Ok(CapturedFrame::new(
            now()?,
            "image/jpeg",
            width,
            height,
            Some("Camera".to_owned()),
            JsonObject::empty(),
            body,
        ))
    }
}

impl ImageCapture for CameraStill {
    fn capture(
        &self,
    ) -> Pin<Box<dyn Future<Output = Result<CapturedFrame, A2aLabError>> + Send + '_>> {
        Box::pin(async move { self.take_frame() })
    }
}

/// Source descriptor for the external camera.
pub fn external_camera_source() -> Result<ImageSource, A2aLabError> {
    external_camera_source_with_description(DEFAULT_EXTERNAL_CAMERA_DESCRIPTION)
}

/// Source descriptor with an operator-supplied description.
pub fn external_camera_source_with_description(
    description: &str,
) -> Result<ImageSource, A2aLabError> {
    if description.is_empty() {
        return Err(A2aLabError::invalid("description", "must not be empty"));
    }
    Ok(ImageSource {
        id: ImageSourceId::new(EXTERNAL_CAMERA_SOURCE_ID)?,
        name: "External camera".to_owned(),
        description: description.to_owned(),
        asset_id: None,
        semantic_id: None,
    })
}

/// Live capture of a platform-native external camera.
pub struct ExternalCamera {
    index: u32,
    timeout: Duration,
    transport: ImageTransportConfig,
    capture: Arc<dyn CameraCapture>,
}

impl ExternalCamera {
    /// Captures from `index` using Nokhwa's automatic native backend.
    pub fn new(index: u32, timeout: Duration, max_image_bytes: u64) -> Result<Self, A2aLabError> {
        Self::with_capture(
            index,
            timeout,
            max_image_bytes,
            NativeCameraCapture {
                max_bytes: max_image_bytes,
            },
        )
    }

    /// Captures from `index` through an injected backend.
    pub fn with_capture(
        index: u32,
        timeout: Duration,
        max_image_bytes: u64,
        capture: impl CameraCapture + 'static,
    ) -> Result<Self, A2aLabError> {
        Self::with_shared_capture(index, timeout, max_image_bytes, Arc::new(capture))
    }

    /// Captures through a shared injected backend.
    pub fn with_shared_capture(
        index: u32,
        timeout: Duration,
        max_image_bytes: u64,
        capture: Arc<dyn CameraCapture>,
    ) -> Result<Self, A2aLabError> {
        if timeout.is_zero() {
            return Err(A2aLabError::invalid("timeout", "must be greater than zero"));
        }
        Ok(Self {
            index,
            timeout,
            transport: ImageTransportConfig::new(max_image_bytes)?,
            capture,
        })
    }

    /// Configured native camera index.
    #[must_use]
    pub const fn index(&self) -> u32 {
        self.index
    }

    /// Registers this camera as the only source in a process-local catalog.
    pub fn catalog(self, retention_per_source: u64) -> Result<LiveImageCatalog, A2aLabError> {
        let max_image_bytes = self.transport.max_image_bytes();
        LiveImageCatalog::new(
            vec![(external_camera_source()?, Arc::new(self))],
            ImageCatalogConfig::new(retention_per_source, max_image_bytes)?,
        )
    }

    async fn take_frame(&self) -> Result<CapturedFrame, A2aLabError> {
        let body = self.read_frame().await?;
        if body.is_empty() {
            return Err(self.invalid("returned an empty frame"));
        }
        let (width, height) =
            jpeg_dimensions(&body).map_err(|_| self.invalid("returned a malformed MJPEG frame"))?;
        self.transport
            .check_payload(&body)
            .map_err(|_| self.invalid("returned a frame larger than the configured maximum"))?;
        Ok(CapturedFrame::new(
            now()?,
            "image/jpeg",
            width,
            height,
            Some("External camera".to_owned()),
            JsonObject::empty(),
            body,
        ))
    }

    async fn read_frame(&self) -> Result<Vec<u8>, A2aLabError> {
        if let Some(bytes) = latest_external_frame() {
            return Ok(bytes);
        }
        let capture = Arc::clone(&self.capture);
        let index = self.index;
        let task = tokio::task::spawn_blocking(move || {
            capture.capture(
                index,
                ExternalCameraFormat::REQUIRED,
                EXTERNAL_CAMERA_SETTLING_FRAMES,
            )
        });
        match tokio::time::timeout(self.timeout, task).await {
            Ok(Ok(Ok(bytes))) => Ok(bytes),
            Ok(Ok(Err(error))) => Err(self.capture_error(error)),
            Ok(Err(error)) => Err(self.unavailable(&format!("capture task failed: {error}"))),
            Err(_) => Err(self.unavailable("timed out")),
        }
    }

    fn capture_error(&self, error: CameraCaptureError) -> A2aLabError {
        match error {
            CameraCaptureError::ExactFormatUnavailable(detail) => self.unavailable(&format!(
                "exact MJPEG 3840x2160 at 30 fps is unavailable ({detail})"
            )),
            CameraCaptureError::Backend(detail) => {
                self.unavailable(&format!("native backend capture failed ({detail})"))
            }
            CameraCaptureError::Empty => self.invalid("returned an empty frame"),
            CameraCaptureError::Oversize => {
                self.invalid("returned a frame larger than the configured maximum")
            }
        }
    }

    fn unavailable(&self, reason: &str) -> A2aLabError {
        A2aLabError::unavailable(format!("external camera index {}: {reason}", self.index))
    }

    fn invalid(&self, reason: &str) -> A2aLabError {
        A2aLabError::invalid(
            "camera",
            format!("external camera index {}: {reason}", self.index),
        )
    }
}

impl NativeCameraCapture {
    pub(crate) fn read_open(self, camera: &mut Camera) -> Result<Vec<u8>, CameraCaptureError> {
        let frame = camera
            .frame_raw()
            .map_err(|error| CameraCaptureError::Backend(error.to_string()))?;
        owned_mjpeg(&frame, self.max_bytes)
    }
}

/// V4L2 MJPEG buffers are often padded to `width * height`. Keep the JPEG through
/// its end marker so live playback and disk storage do not carry the padding.
pub(crate) fn trim_mjpeg_buffer(bytes: &[u8]) -> &[u8] {
    let Some(end) = jpeg_eoi(bytes) else {
        return bytes;
    };
    &bytes[..end]
}

fn owned_mjpeg(frame: &[u8], max_bytes: u64) -> Result<Vec<u8>, CameraCaptureError> {
    let jpeg = trim_mjpeg_buffer(frame);
    if jpeg.is_empty() {
        return Err(CameraCaptureError::Empty);
    }
    let cap = usize::try_from(max_bytes).map_err(|_| CameraCaptureError::Oversize)?;
    if jpeg.len() > cap {
        return Err(CameraCaptureError::Oversize);
    }
    Ok(jpeg.to_vec())
}

fn jpeg_eoi(bytes: &[u8]) -> Option<usize> {
    if bytes.len() < 4 || bytes[0] != 0xFF || bytes[1] != 0xD8 {
        return None;
    }
    let mut index = 2;
    while index + 1 < bytes.len() {
        if bytes[index] != 0xFF {
            return None;
        }
        while index < bytes.len() && bytes[index] == 0xFF {
            index += 1;
        }
        if index >= bytes.len() {
            return None;
        }
        let marker = bytes[index];
        index += 1;
        if marker == 0xD9 {
            return Some(index);
        }
        if marker == 0x00 || marker == 0x01 || (0xD0..=0xD7).contains(&marker) {
            continue;
        }
        if marker == 0xDA {
            if index + 1 >= bytes.len() {
                return None;
            }
            let len = usize::from(u16::from_be_bytes([bytes[index], bytes[index + 1]]));
            if len < 2 || index.checked_add(len)? > bytes.len() {
                return None;
            }
            return jpeg_scan_eoi(bytes, index + len);
        }
        if index + 1 >= bytes.len() {
            return None;
        }
        let len = usize::from(u16::from_be_bytes([bytes[index], bytes[index + 1]]));
        if len < 2 || index.checked_add(len)? > bytes.len() {
            return None;
        }
        index += len;
    }
    None
}

fn jpeg_scan_eoi(bytes: &[u8], mut index: usize) -> Option<usize> {
    while index + 1 < bytes.len() {
        if bytes[index] != 0xFF {
            index += 1;
            continue;
        }
        let next = bytes[index + 1];
        if next == 0x00 || next == 0xFF {
            index += 1 + usize::from(next == 0x00);
            continue;
        }
        if next == 0xD9 {
            return Some(index + 2);
        }
        if (0xD0..=0xD7).contains(&next) {
            index += 2;
            continue;
        }
        return None;
    }
    None
}

impl ImageCapture for ExternalCamera {
    fn capture(
        &self,
    ) -> Pin<Box<dyn Future<Output = Result<CapturedFrame, A2aLabError>> + Send + '_>> {
        Box::pin(async move { self.take_frame().await })
    }
}

#[cfg(test)]
mod tests {
    use super::trim_mjpeg_buffer;

    #[test]
    fn trims_v4l2_padding_after_the_scan_end() {
        let mut jpeg = vec![
            0xFF, 0xD8, 0xFF, 0xE0, 0x00, 0x02, 0xFF, 0xDA, 0x00, 0x02, 0x12, 0xFF, 0x00, 0x34,
            0xFF, 0xD9,
        ];
        let end = jpeg.len();
        jpeg.extend_from_slice(&[0, 0, 0, 0]);
        assert_eq!(trim_mjpeg_buffer(&jpeg), &jpeg[..end]);
    }

    #[test]
    fn keeps_bytes_that_are_not_a_jpeg() {
        let bytes = [1, 2, 3, 4];
        assert_eq!(trim_mjpeg_buffer(&bytes), &bytes);
    }
}
