//! Native camera capture through Nokhwa.

use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, LazyLock, Mutex};
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
pub const EXTERNAL_CAMERA_WIDTH: u32 = 1920;
/// Required encoded format for external-camera capture.
pub const EXTERNAL_CAMERA_HEIGHT: u32 = 1080;
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
    /// Exact MJPEG 1920x1080 at 30 fps.
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

    /// Negotiated MJPEG when an exact [`Self::capture`] profile is unavailable.
    fn capture_relaxed_mjpeg(
        &self,
        _index: u32,
        _frames: u32,
    ) -> Option<Result<Vec<u8>, CameraCaptureError>> {
        None
    }
}

/// Nokhwa automatic native-backend capture.
#[derive(Debug, Clone, Copy)]
pub struct NativeCameraCapture {
    pub(crate) max_bytes: u64,
}

impl NativeCameraCapture {
    fn capture_mjpeg_stream(
        index: u32,
        request: RequestedFormat<'_>,
        frames: u32,
        max_bytes: u64,
    ) -> Result<Vec<u8>, CameraCaptureError> {
        let mut camera = Camera::new(CameraIndex::Index(index), request)
            .map_err(|error| CameraCaptureError::ExactFormatUnavailable(error.to_string()))?;
        camera
            .open_stream()
            .map_err(|error| CameraCaptureError::Backend(error.to_string()))?;
        let mut kept = None;
        for frame_number in 1..=frames {
            let frame = camera
                .frame_raw()
                .map_err(|error| CameraCaptureError::Backend(error.to_string()))?;
            if frame_number == frames {
                kept = Some(owned_mjpeg(&frame, max_bytes)?);
            }
        }
        kept.ok_or(CameraCaptureError::Empty)
    }
}

impl CameraCapture for NativeCameraCapture {
    fn capture_relaxed_mjpeg(
        &self,
        index: u32,
        frames: u32,
    ) -> Option<Result<Vec<u8>, CameraCaptureError>> {
        let request =
            RequestedFormat::with_formats(RequestedFormatType::None, &[FrameFormat::MJPEG]);
        Some(Self::capture_mjpeg_stream(
            index,
            request,
            frames,
            self.max_bytes,
        ))
    }

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

static LATEST_STILL_BY_SOURCE: LazyLock<Mutex<HashMap<String, Vec<u8>>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

/// Stores the newest live JPEG for `source_id` so still capture can reuse the video session.
pub fn remember_live_still(source_id: &str, bytes: Vec<u8>) {
    if let Ok(mut slots) = LATEST_STILL_BY_SOURCE.lock() {
        slots.insert(source_id.to_owned(), bytes);
    }
}

/// Returns the JPEG last stored for `source_id` by the live video pump.
#[must_use]
pub fn latest_live_still(source_id: &str) -> Option<Vec<u8>> {
    LATEST_STILL_BY_SOURCE
        .lock()
        .ok()
        .and_then(|slots| slots.get(source_id).cloned())
}

/// Drops remembered still data for one configured source. Tests use this between cases.
pub fn clear_live_still(source_id: &str) {
    if let Ok(mut slots) = LATEST_STILL_BY_SOURCE.lock() {
        slots.remove(source_id);
    }
}

/// Stores the newest live JPEG for the legacy `external-camera` id.
pub fn remember_external_frame(bytes: Vec<u8>) {
    remember_live_still(EXTERNAL_CAMERA_SOURCE_ID, bytes);
}

/// Stores the newest live JPEG for the legacy `camera` id.
pub fn remember_camera_frame(bytes: Vec<u8>) {
    remember_live_still(CAMERA_SOURCE_ID, bytes);
}

/// Drops remembered still data for the legacy `camera` id.
pub fn clear_camera_frame() {
    clear_live_still(CAMERA_SOURCE_ID);
}

/// Clears every remembered live still buffer. Integration tests call this for isolation.
pub fn clear_all_live_stills() {
    if let Ok(mut slots) = LATEST_STILL_BY_SOURCE.lock() {
        slots.clear();
    }
}

fn displayed_name(name: &str, fallback: &str) -> String {
    let trimmed = name.trim();
    if trimmed.is_empty() {
        fallback.to_owned()
    } else {
        trimmed.to_owned()
    }
}

/// Builds an [`ImageSource`] from a resolved live video descriptor.
pub fn configured_still_image_source(
    descriptor: &crate::videos::LiveVideoSourceDescriptor,
) -> Result<ImageSource, A2aLabError> {
    Ok(ImageSource {
        id: ImageSourceId::new(&descriptor.id)?,
        name: descriptor.name.clone(),
        description: descriptor.description.clone(),
        asset_id: None,
        semantic_id: None,
    })
}

/// Source descriptor for the optional second webcam.
pub fn camera_source() -> Result<ImageSource, A2aLabError> {
    camera_source_named("Camera")
}

/// Source descriptor for the optional second webcam with an operator-supplied name.
pub fn camera_source_named(name: &str) -> Result<ImageSource, A2aLabError> {
    let name = displayed_name(name, "Camera");
    Ok(ImageSource {
        id: ImageSourceId::new(CAMERA_SOURCE_ID)?,
        name,
        description: "Still frame from the configured camera.".to_owned(),
        asset_id: None,
        semantic_id: None,
    })
}

/// Still capture for one configured source id.
///
/// Reads the JPEG stored by the live video pump. This does not open a device.
pub struct PumpLinkedStill {
    source_id: String,
    label: String,
    transport: ImageTransportConfig,
}

impl PumpLinkedStill {
    /// Rejects an invalid payload limit. No device is opened.
    pub fn new(
        source_id: impl Into<String>,
        label: impl Into<String>,
        max_image_bytes: u64,
    ) -> Result<Self, A2aLabError> {
        Ok(Self {
            source_id: source_id.into(),
            label: label.into(),
            transport: ImageTransportConfig::new(max_image_bytes)?,
        })
    }

    fn take_frame(&self) -> Result<CapturedFrame, A2aLabError> {
        let source = self.source_id.as_str();
        let Some(body) = latest_live_still(source) else {
            return Err(A2aLabError::unavailable(format!(
                "{source} has no frame yet"
            )));
        };
        if body.is_empty() {
            return Err(A2aLabError::invalid(
                "source_id",
                format!("{source} returned an empty frame"),
            ));
        }
        let (width, height) = jpeg_dimensions(&body).map_err(|_| {
            A2aLabError::invalid(
                "source_id",
                format!("{source} returned a malformed MJPEG frame"),
            )
        })?;
        self.transport.check_payload(&body).map_err(|_| {
            A2aLabError::invalid(
                "source_id",
                format!("{source} returned a frame larger than the configured maximum"),
            )
        })?;
        Ok(CapturedFrame::new(
            now()?,
            "image/jpeg",
            width,
            height,
            Some(self.label.clone()),
            JsonObject::empty(),
            body,
        ))
    }
}

impl ImageCapture for PumpLinkedStill {
    fn capture(
        &self,
    ) -> Pin<Box<dyn Future<Output = Result<CapturedFrame, A2aLabError>> + Send + '_>> {
        Box::pin(async move { self.take_frame() })
    }
}

/// Still capture for one configured live video source.
///
/// Reuses the live video JPEG when present, otherwise opens the configured device index.
pub struct ConfiguredNativeStill {
    descriptor: crate::videos::LiveVideoSourceDescriptor,
    format: ExternalCameraFormat,
    timeout: Duration,
    transport: ImageTransportConfig,
    capture: Arc<dyn CameraCapture>,
}

impl ConfiguredNativeStill {
    /// Builds still capture from a resolved live video descriptor.
    pub fn from_descriptor(
        descriptor: &crate::videos::LiveVideoSourceDescriptor,
        timeout: Duration,
        max_image_bytes: u64,
        capture: Option<Arc<dyn CameraCapture>>,
    ) -> Result<Self, A2aLabError> {
        if timeout.is_zero() {
            return Err(A2aLabError::invalid("timeout", "must be greater than zero"));
        }
        let capture = capture.unwrap_or_else(|| {
            Arc::new(NativeCameraCapture {
                max_bytes: max_image_bytes,
            })
        });
        Ok(Self {
            descriptor: descriptor.clone(),
            format: ExternalCameraFormat {
                width: descriptor.width.unwrap_or(640),
                height: descriptor.height.unwrap_or(480),
                fps: descriptor.capture_rate.unwrap_or(EXTERNAL_CAMERA_FPS),
            },
            timeout,
            transport: ImageTransportConfig::new(max_image_bytes)?,
            capture,
        })
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
            Some(self.descriptor.name.clone()),
            JsonObject::empty(),
            body,
        ))
    }

    async fn read_frame(&self) -> Result<Vec<u8>, A2aLabError> {
        if let Some(bytes) = latest_live_still(self.descriptor.id.as_str()) {
            return Ok(bytes);
        }
        let capture = Arc::clone(&self.capture);
        let format = self.format;
        let descriptor = self.descriptor.clone();
        let source_id = descriptor.id.clone();
        let task = tokio::task::spawn_blocking(move || {
            capture.capture(
                descriptor.device_index,
                format,
                EXTERNAL_CAMERA_SETTLING_FRAMES,
            )
        });
        match tokio::time::timeout(self.timeout, task).await {
            Ok(Ok(Ok(bytes))) => Ok(bytes),
            Ok(Ok(Err(error))) => Err(self.capture_error(error)),
            Ok(Err(error)) => Err(self.unavailable(&format!("capture task failed: {error}"))),
            Err(_) => Err(self.unavailable(&format!(
                "{source_id} timed out before the live pump or device produced a frame"
            ))),
        }
    }

    fn capture_error(&self, error: CameraCaptureError) -> A2aLabError {
        match error {
            CameraCaptureError::ExactFormatUnavailable(detail) => self.unavailable(&format!(
                "exact MJPEG {}x{} at {} fps is unavailable ({detail})",
                self.format.width, self.format.height, self.format.fps
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
        A2aLabError::unavailable(format!(
            "{} (device index {}): {reason}",
            self.descriptor.id, self.descriptor.device_index
        ))
    }

    fn invalid(&self, reason: &str) -> A2aLabError {
        A2aLabError::invalid(
            "source_id",
            format!(
                "{} (device index {}): {reason}",
                self.descriptor.id, self.descriptor.device_index
            ),
        )
    }
}

impl ImageCapture for ConfiguredNativeStill {
    fn capture(
        &self,
    ) -> Pin<Box<dyn Future<Output = Result<CapturedFrame, A2aLabError>> + Send + '_>> {
        Box::pin(async move { self.take_frame().await })
    }
}

/// Legacy still capture for schema-default source `camera`.
pub struct CameraStill(PumpLinkedStill);

impl CameraStill {
    /// Rejects an invalid payload limit. No device is opened.
    pub fn new(max_image_bytes: u64) -> Result<Self, A2aLabError> {
        Ok(Self(PumpLinkedStill::new(
            CAMERA_SOURCE_ID,
            "Camera",
            max_image_bytes,
        )?))
    }
}

impl ImageCapture for CameraStill {
    fn capture(
        &self,
    ) -> Pin<Box<dyn Future<Output = Result<CapturedFrame, A2aLabError>> + Send + '_>> {
        self.0.capture()
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
    external_camera_source_named(description, "External camera")
}

/// Source descriptor with an operator-supplied description and name.
pub fn external_camera_source_named(
    description: &str,
    name: &str,
) -> Result<ImageSource, A2aLabError> {
    if description.is_empty() {
        return Err(A2aLabError::invalid("description", "must not be empty"));
    }
    Ok(ImageSource {
        id: ImageSourceId::new(EXTERNAL_CAMERA_SOURCE_ID)?,
        name: displayed_name(name, "External camera"),
        description: description.to_owned(),
        asset_id: None,
        semantic_id: None,
    })
}

/// Live capture of a platform-native external camera.
pub struct ExternalCamera {
    index: u32,
    still_source_id: String,
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
        Self::with_shared_capture_for_source(
            index,
            EXTERNAL_CAMERA_SOURCE_ID,
            timeout,
            max_image_bytes,
            capture,
        )
    }

    /// Captures through a shared injected backend keyed to `still_source_id`.
    pub fn with_shared_capture_for_source(
        index: u32,
        still_source_id: impl Into<String>,
        timeout: Duration,
        max_image_bytes: u64,
        capture: Arc<dyn CameraCapture>,
    ) -> Result<Self, A2aLabError> {
        if timeout.is_zero() {
            return Err(A2aLabError::invalid("timeout", "must be greater than zero"));
        }
        Ok(Self {
            index,
            still_source_id: still_source_id.into(),
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
        if let Some(bytes) = latest_live_still(&self.still_source_id) {
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
                "exact MJPEG {EXTERNAL_CAMERA_WIDTH}x{EXTERNAL_CAMERA_HEIGHT} at {EXTERNAL_CAMERA_FPS} fps is unavailable ({detail})"
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
