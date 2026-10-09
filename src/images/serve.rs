//! Serve-mode image sources for the on-board and external cameras.

use std::sync::Arc;
use std::time::Duration;

use a2a_lab_dev_kit::{A2aLabError, ImageSource, ImageTransportConfig};

use crate::camera::{
    CameraCapture, DEFAULT_EXTERNAL_CAMERA_DESCRIPTION, ExternalCamera,
    external_camera_source_with_description,
};
use crate::images::{ImageCapture, ImageCatalogConfig, LiveImageCatalog};
use crate::opentrons::{OpentronsCamera, opentrons_camera_source};

/// Default still-capture budget for both cameras.
pub const DEFAULT_CAPTURE_TIMEOUT_MS: u64 = 10_000;

/// Timeout, retention, payload limit, and the optional external camera.
///
/// Building this value does not open a camera or contact robot-server.
/// `camera_index` is the extra camera. `None` serves only the on-board camera.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImageServeConfig {
    camera_index: Option<u32>,
    description: String,
    transport: ImageTransportConfig,
    capture_timeout: Duration,
    retention_per_source: u64,
}

impl ImageServeConfig {
    /// Rejects a zero timeout or retention and an invalid payload limit.
    ///
    /// `None` leaves the extra camera off. An index enables `external-camera`.
    /// `description` of `None` uses
    /// [`DEFAULT_EXTERNAL_CAMERA_DESCRIPTION`]. An empty description is rejected.
    pub fn new(
        camera_index: Option<u32>,
        description: Option<&str>,
        max_image_bytes: u64,
        capture_timeout_ms: u64,
        retention_per_source: u64,
    ) -> Result<Self, A2aLabError> {
        let description = match description {
            None => DEFAULT_EXTERNAL_CAMERA_DESCRIPTION.to_owned(),
            Some("") => return Err(A2aLabError::invalid("description", "must not be empty")),
            Some(text) => text.to_owned(),
        };
        let capture_timeout = capture_timeout(capture_timeout_ms)?;
        let transport = ImageTransportConfig::new(max_image_bytes)?;
        ImageCatalogConfig::new(retention_per_source, transport.max_image_bytes())?;
        Ok(Self {
            camera_index,
            description,
            transport,
            capture_timeout,
            retention_per_source,
        })
    }

    /// Native camera index when the extra camera is enabled.
    #[must_use]
    pub const fn camera_index(&self) -> Option<u32> {
        self.camera_index
    }

    /// Description advertised for `external-camera` when that source is enabled.
    #[must_use]
    pub fn description(&self) -> &str {
        &self.description
    }

    /// Decoded-byte maximum applied to both cameras and protocol adapters.
    #[must_use]
    pub const fn max_image_bytes(&self) -> u64 {
        self.transport.max_image_bytes()
    }

    /// Budget for one current-image capture.
    #[must_use]
    pub const fn capture_timeout(&self) -> Duration {
        self.capture_timeout
    }

    /// Frames kept for each source.
    #[must_use]
    pub const fn retention_per_source(&self) -> u64 {
        self.retention_per_source
    }

    /// Transport limit shared by the lab service and the A2A MCP connection.
    #[must_use]
    pub const fn transport(&self) -> ImageTransportConfig {
        self.transport
    }

    /// One startup line naming the on-board camera, the extra camera, and the limits.
    #[must_use]
    pub fn summary(&self) -> String {
        let external = match self.camera_index {
            Some(index) => {
                format!(
                    "external-camera index {index} description {}",
                    self.description
                )
            }
            None => "external-camera off".to_owned(),
        };
        format!(
            "images       opentrons-camera asset opentrons-ot2; {external} timeout {}ms retention {} max-bytes {}",
            self.capture_timeout.as_millis(),
            self.retention_per_source,
            self.max_image_bytes()
        )
    }
}

/// Live cameras for this process. `external_capture` replaces native capture in tests.
///
/// The on-board camera is always registered. The extra camera is registered only
/// when [`ImageServeConfig::camera_index`] is set. Neither camera is probed. A
/// missing or failing camera surfaces on the current-image call for that source.
pub fn image_catalog(
    opentrons_url: &str,
    config: &ImageServeConfig,
    external_capture: Option<Arc<dyn CameraCapture>>,
) -> Result<LiveImageCatalog, A2aLabError> {
    let opentrons = OpentronsCamera::new(
        opentrons_url,
        config.capture_timeout,
        config.max_image_bytes(),
    )?;
    let mut sources: Vec<(ImageSource, Arc<dyn ImageCapture>)> =
        vec![(opentrons_camera_source()?, Arc::new(opentrons))];
    if let Some(index) = config.camera_index() {
        let external = match external_capture {
            Some(capture) => ExternalCamera::with_shared_capture(
                index,
                config.capture_timeout,
                config.max_image_bytes(),
                capture,
            )?,
            None => ExternalCamera::new(index, config.capture_timeout, config.max_image_bytes())?,
        };
        sources.push((
            external_camera_source_with_description(&config.description)?,
            Arc::new(external),
        ));
    }
    LiveImageCatalog::new(
        sources,
        ImageCatalogConfig::new(config.retention_per_source, config.max_image_bytes())?,
    )
}

fn capture_timeout(millis: u64) -> Result<Duration, A2aLabError> {
    if millis == 0 {
        return Err(A2aLabError::invalid(
            "capture_timeout_ms",
            "must be greater than zero",
        ));
    }
    Ok(Duration::from_millis(millis))
}
