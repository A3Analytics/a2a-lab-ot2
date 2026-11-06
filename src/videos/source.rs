//! Operator-defined live video source descriptors.

use std::collections::HashSet;

use a2a_lab_dev_kit::A2aLabError;
use a2a_lab_dev_kit::videos::{CaptureFormatPreference, CaptureProfileRequest, CaptureRate};

use crate::camera::{
    CAMERA_SOURCE_ID, EXTERNAL_CAMERA_HEIGHT, EXTERNAL_CAMERA_SOURCE_ID, EXTERNAL_CAMERA_WIDTH,
};

/// Raw camera input negotiated before delivery encoding.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum LiveCameraInputFormat {
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
            Self::Mjpeg => CaptureFormatPreference::Mjpeg,
            Self::Yuy2 => CaptureFormatPreference::Yuy2,
        }
    }
}

/// How a source publishes live and DVR samples after raw capture.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LiveVideoDelivery {
    /// MJPEG live follow and MJPEG DVR, as for legacy `camera`.
    Mjpeg,
    /// H.264 live follow and H.264 DVR, as for legacy `external-camera`.
    H264,
}

/// One configured live source before capture starts.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LiveVideoSourceDescriptor {
    /// Stable operator-defined id used for discovery and DVR.
    pub id: String,
    pub name: String,
    pub description: String,
    /// Deployment-specific capture-library index.
    pub device_index: u32,
    pub input: LiveCameraInputFormat,
    pub width: u32,
    pub height: u32,
    pub delivery: LiveVideoDelivery,
}

impl LiveVideoSourceDescriptor {
    /// Legacy `external-camera`: MJPEG in, H.264 out, 1920×1080.
    #[must_use]
    pub fn legacy_external(index: u32, description: impl Into<String>) -> Self {
        Self {
            id: EXTERNAL_CAMERA_SOURCE_ID.to_owned(),
            name: "External camera".to_owned(),
            description: description.into(),
            device_index: index,
            input: LiveCameraInputFormat::Mjpeg,
            width: EXTERNAL_CAMERA_WIDTH,
            height: EXTERNAL_CAMERA_HEIGHT,
            delivery: LiveVideoDelivery::H264,
        }
    }

    /// Legacy `camera`: MJPEG in and out at 1280×720.
    #[must_use]
    pub fn legacy_camera(index: u32) -> Self {
        Self {
            id: CAMERA_SOURCE_ID.to_owned(),
            name: "Camera".to_owned(),
            description: "Live MJPEG from the configured camera.".to_owned(),
            device_index: index,
            input: LiveCameraInputFormat::Mjpeg,
            width: 1280,
            height: 720,
            delivery: LiveVideoDelivery::Mjpeg,
        }
    }

    /// Profile request for opening this source. Input format is raw capture only.
    pub fn capture_request(&self, output_fps: u32) -> Result<CaptureProfileRequest, A2aLabError> {
        CaptureProfileRequest::new(
            self.input.capture_preference(),
            self.width,
            self.height,
            CaptureRate::fps(output_fps.max(1))?,
        )
    }

    #[must_use]
    pub fn remembers_external_still(&self) -> bool {
        self.id == EXTERNAL_CAMERA_SOURCE_ID
    }

    #[must_use]
    pub fn remembers_camera_still(&self) -> bool {
        self.id == CAMERA_SOURCE_ID
    }
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
        if !devices.insert(source.device_index) {
            return Err(A2aLabError::invalid(
                "video",
                format!(
                    "device index {} is already assigned to another live video source",
                    source.device_index
                ),
            ));
        }
    }
    Ok(())
}
