//! Operator-defined live video source descriptors.

use std::collections::HashSet;

use a2a_lab_dev_kit::A2aLabError;
use a2a_lab_dev_kit::CameraSelector;
use a2a_lab_dev_kit::VideoInputFormat;
use a2a_lab_dev_kit::VideoSourceConfig;
use a2a_lab_dev_kit::VideoSourceId;
use a2a_lab_dev_kit::videos::{CaptureFormatPreference, CaptureProfileRequest, CaptureRate};

use crate::camera::{
    CAMERA_SOURCE_ID, EXTERNAL_CAMERA_HEIGHT, EXTERNAL_CAMERA_SOURCE_ID, EXTERNAL_CAMERA_WIDTH,
};

/// Raw camera input negotiated before delivery encoding.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum LiveCameraInputFormat {
    /// Same-resolution MJPEG when advertised, otherwise same-resolution YUY2.
    Auto,
    /// Encoded MJPEG access units from the device.
    Mjpeg,
    /// Packed YUY2/YUYV from the device.
    Yuy2,
}

impl LiveCameraInputFormat {
    /// Dev-kit preference matching this input. Never selects a delivery codec.
    #[must_use]
    pub const fn capture_preference(self) -> CaptureFormatPreference {
        match self {
            Self::Auto => CaptureFormatPreference::Auto,
            Self::Mjpeg => CaptureFormatPreference::Mjpeg,
            Self::Yuy2 => CaptureFormatPreference::Yuy2,
        }
    }
}

/// How a source publishes live and DVR samples after raw capture.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LiveVideoDelivery {
    /// Multipart JPEG delivery for test and compatibility fixtures.
    Mjpeg,
    /// H.264 live follow and H.264 DVR.
    H264,
}

/// One configured live source before capture starts.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LiveVideoSourceDescriptor {
    /// Stable operator-defined id used for discovery and DVR.
    pub id: String,
    pub name: String,
    pub description: String,
    /// Declarative selector from file or legacy index flags.
    pub camera_id: Option<CameraSelector>,
    /// Capture-library index when [`Self::camera_id`] is index-based.
    pub device_index: Option<u32>,
    pub input: LiveCameraInputFormat,
    pub width: Option<u32>,
    pub height: Option<u32>,
    /// Optional capture frame rate. Independent of live output `videos.fps`.
    pub capture_rate: Option<u32>,
    pub delivery: LiveVideoDelivery,
}

/// Index-based `camera_id` fields for fixtures and manual descriptors.
#[must_use]
pub fn index_camera_fields(index: u32) -> (Option<CameraSelector>, Option<u32>) {
    (Some(CameraSelector::Index(index)), Some(index))
}

impl LiveVideoSourceDescriptor {
    /// Legacy `external-camera`: MJPEG in, H.264 out, 1920×1080.
    #[must_use]
    pub fn legacy_external(index: u32, description: impl Into<String>) -> Self {
        Self {
            id: EXTERNAL_CAMERA_SOURCE_ID.to_owned(),
            name: "External camera".to_owned(),
            description: description.into(),
            camera_id: Some(CameraSelector::Index(index)),
            device_index: Some(index),
            input: LiveCameraInputFormat::Mjpeg,
            width: Some(EXTERNAL_CAMERA_WIDTH),
            height: Some(EXTERNAL_CAMERA_HEIGHT),
            capture_rate: None,
            delivery: LiveVideoDelivery::H264,
        }
    }

    /// Legacy `camera`: MJPEG in and H.264 out at 1280×720.
    #[must_use]
    pub fn legacy_camera(index: u32) -> Self {
        Self {
            id: CAMERA_SOURCE_ID.to_owned(),
            name: "Camera".to_owned(),
            description: "Live MJPEG from the configured camera.".to_owned(),
            camera_id: Some(CameraSelector::Index(index)),
            device_index: Some(index),
            input: LiveCameraInputFormat::Mjpeg,
            width: Some(1280),
            height: Some(720),
            capture_rate: None,
            delivery: LiveVideoDelivery::H264,
        }
    }

    /// Profile request for opening this source. Input format is raw capture only.
    pub fn capture_request(&self, output_fps: u32) -> Result<CaptureProfileRequest, A2aLabError> {
        let capture_rate = self.capture_rate.map(CaptureRate::fps).transpose()?;
        CaptureProfileRequest::new(
            self.input.capture_preference(),
            self.width,
            self.height,
            CaptureRate::fps(output_fps.max(1))?,
            capture_rate,
        )
    }

    /// Declarative devkit source entry for [`DeclarativeVideoRuntime`].
    pub fn to_video_source_config(&self) -> Result<VideoSourceConfig, A2aLabError> {
        Ok(VideoSourceConfig {
            id: VideoSourceId::new(&self.id)?,
            name: Some(self.name.clone()),
            description: Some(self.description.clone()),
            format: match self.input {
                LiveCameraInputFormat::Auto => VideoInputFormat::Auto,
                LiveCameraInputFormat::Mjpeg => VideoInputFormat::Mjpeg,
                LiveCameraInputFormat::Yuy2 => VideoInputFormat::Yuy2,
            },
            camera_id: self.camera_id.clone(),
            width: self.width,
            height: self.height,
            capture_rate: self.capture_rate,
            semantic_indexing: None,
        })
    }

    /// Resolved camera label for logs and operator diagnostics.
    #[must_use]
    pub fn camera_id_label(&self) -> String {
        match &self.camera_id {
            Some(CameraSelector::Text(text)) => text.clone(),
            Some(CameraSelector::Index(index)) => index.to_string(),
            None => self
                .device_index
                .map_or_else(|| "default".to_owned(), |index| index.to_string()),
        }
    }

    /// Capture-library index for native open. Text selectors fail here until runtime resolution.
    pub fn index_for_capture(&self) -> Result<u32, A2aLabError> {
        self.device_index.ok_or_else(|| {
            A2aLabError::invalid(
                "video",
                format!(
                    "source `{}` requires an index-based camera_id for native capture",
                    self.id
                ),
            )
        })
    }

    /// Returns true for configuration-defined ids (not legacy schema defaults).
    #[must_use]
    pub fn is_configured_identity(&self) -> bool {
        !matches!(
            self.id.as_str(),
            CAMERA_SOURCE_ID | EXTERNAL_CAMERA_SOURCE_ID
        )
    }
}

/// Single explicit capture profile request for this source.
pub fn native_capture_profile_candidates(
    descriptor: &LiveVideoSourceDescriptor,
    output_fps: u32,
) -> Result<Vec<CaptureProfileRequest>, A2aLabError> {
    Ok(vec![descriptor.capture_request(output_fps)?])
}

/// Rejects duplicate ids or device indices before any producer starts.
pub fn validate_live_sources(sources: &[LiveVideoSourceDescriptor]) -> Result<(), A2aLabError> {
    let mut ids = HashSet::new();
    let mut devices = HashSet::new();
    for source in sources {
        if !ids.insert(source.id.as_str()) {
            return Err(A2aLabError::invalid(
                "video",
                format!("duplicate live video source id `{}`", source.id),
            ));
        }
        if let Some(index) = source.device_index
            && !devices.insert(index)
        {
            return Err(A2aLabError::invalid(
                "video",
                format!("device index {index} is already assigned to another live video source"),
            ));
        }
    }
    Ok(())
}
