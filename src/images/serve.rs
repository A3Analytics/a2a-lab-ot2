//! Serve-mode image sources for the on-board and external cameras.

use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use a2a_lab_dev_kit::videos::WarmStillCache;
use a2a_lab_dev_kit::{A2aLabError, ImageSource, ImageSourceId, ImageTransportConfig};

use crate::camera::{
    CAMERA_SOURCE_ID, CameraCapture, CameraStill, ConfiguredNativeStill,
    DEFAULT_EXTERNAL_CAMERA_DESCRIPTION, EXTERNAL_CAMERA_SOURCE_ID, ExternalCamera,
    PumpLinkedStill, camera_source_named, configured_still_image_source,
    external_camera_source_named,
};
use crate::images::{
    DEFAULT_IMAGE_HISTORY_DB, IMAGE_HISTORY_MAX_AGE, ImageCapture, ImageCatalogConfig,
    LiveImageCatalog,
};
use crate::opentrons::{OPENTRONS_CAMERA_SOURCE_ID, OpentronsCamera, opentrons_camera_source};
use crate::orbbec::{
    ORBBEC_CAMERA_SOURCE_ID, ORBBEC_FIRST_FRAME_WAIT, OrbbecCamera, OrbbecCapture,
    SdkOrbbecCapture, orbbec_camera_source,
};
use crate::videos::LiveVideoSourceDescriptor;

/// Default still-capture budget for both cameras.
pub const DEFAULT_CAPTURE_TIMEOUT_MS: u64 = 10_000;

/// Timeout, retention, payload limit, and the optional cameras.
///
/// Building this value does not open a camera or contact robot-server.
/// `camera_index` is the external camera. `None` leaves that source off.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImageServeConfig {
    camera_index: Option<u32>,
    description: String,
    external_name: String,
    webcam_index: Option<u32>,
    webcam_name: String,
    native_video_sources: Vec<LiveVideoSourceDescriptor>,
    orbbec: Option<OrbbecSelection>,
    transport: ImageTransportConfig,
    capture_timeout: Duration,
    retention_per_source: u64,
}

/// Selected Orbbec device. `None` uses the only connected device at capture time.
#[derive(Debug, Clone, PartialEq, Eq)]
struct OrbbecSelection {
    serial: Option<String>,
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
            external_name: "External camera".to_owned(),
            webcam_index: None,
            webcam_name: "Camera".to_owned(),
            native_video_sources: Vec::new(),
            orbbec: None,
            transport,
            capture_timeout,
            retention_per_source,
        })
    }

    /// Registers still sources from resolved live video configuration.
    #[must_use]
    pub fn with_native_video_sources(mut self, sources: &[LiveVideoSourceDescriptor]) -> Self {
        self.native_video_sources = sources.to_vec();
        self
    }

    /// Whether still/image sources come from resolved video configuration.
    #[must_use]
    pub fn uses_resolved_native_sources(&self) -> bool {
        !self.native_video_sources.is_empty()
    }

    /// Enables `orbbec-camera` without opening a device.
    ///
    /// `None` selects the only connected device when a frame is requested.
    /// An empty serial is rejected.
    pub fn with_orbbec(mut self, serial: Option<&str>) -> Result<Self, A2aLabError> {
        if serial.is_some_and(str::is_empty) {
            return Err(A2aLabError::invalid("orbbec_serial", "must not be empty"));
        }
        self.orbbec = Some(OrbbecSelection {
            serial: serial.map(str::to_owned),
        });
        Ok(self)
    }

    /// Uses a configured display name for legacy schema-default ids only.
    #[must_use]
    pub fn with_source_label(mut self, id: &str, name: &str) -> Self {
        match id {
            CAMERA_SOURCE_ID => name.clone_into(&mut self.webcam_name),
            EXTERNAL_CAMERA_SOURCE_ID => name.clone_into(&mut self.external_name),
            _ => {}
        }
        self
    }

    /// Enables source `camera` without opening a device.
    ///
    /// Still capture reuses the latest JPEG from the live video pump.
    #[must_use]
    pub fn with_camera_index(mut self, index: u32) -> Self {
        self.webcam_index = Some(index);
        self
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

    /// Whether source `camera` is advertised.
    #[must_use]
    pub const fn camera_enabled(&self) -> bool {
        self.webcam_index.is_some()
    }

    /// Whether `orbbec-camera` is advertised.
    #[must_use]
    pub const fn orbbec_enabled(&self) -> bool {
        self.orbbec.is_some()
    }

    /// Serial pinned for `orbbec-camera`, when one was configured.
    #[must_use]
    pub fn orbbec_serial(&self) -> Option<&str> {
        self.orbbec
            .as_ref()
            .and_then(|selection| selection.serial.as_deref())
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

    /// One startup line naming configured sources and limits.
    #[must_use]
    pub fn summary(&self) -> String {
        let native = if self.uses_resolved_native_sources() {
            self.native_video_sources
                .iter()
                .map(|source| {
                    if source.id == EXTERNAL_CAMERA_SOURCE_ID {
                        format!(
                            "{} camera_id {} description {}",
                            source.id,
                            source.camera_id_label(),
                            source.description
                        )
                    } else if source.id == CAMERA_SOURCE_ID {
                        format!("{} camera_id {}", source.id, source.camera_id_label())
                    } else {
                        format!(
                            "{} camera_id {} ({})",
                            source.id,
                            source.camera_id_label(),
                            source.name
                        )
                    }
                })
                .collect::<Vec<_>>()
                .join("; ")
        } else {
            let external = match self.camera_index {
                Some(index) => {
                    format!(
                        "external-camera index {index} description {}",
                        self.description
                    )
                }
                None => "external-camera off".to_owned(),
            };
            let camera = match self.webcam_index {
                Some(index) => format!("camera index {index}"),
                None => "camera off".to_owned(),
            };
            format!("{external}; {camera}")
        };
        let orbbec = match &self.orbbec {
            None => "orbbec-camera off".to_owned(),
            Some(OrbbecSelection { serial: None }) => "orbbec-camera sole-device".to_owned(),
            Some(OrbbecSelection {
                serial: Some(serial),
            }) => format!("orbbec-camera serial {serial}"),
        };
        format!(
            "images       opentrons-camera asset opentrons-ot2; {native}; {orbbec} timeout {}ms retention {} max-bytes {} history {DEFAULT_IMAGE_HISTORY_DB} age {}d",
            self.capture_timeout.as_millis(),
            self.retention_per_source,
            self.max_image_bytes(),
            IMAGE_HISTORY_MAX_AGE.as_secs() / 86_400
        )
    }
}

/// Live cameras for this process. `external_capture` replaces native capture in tests.
pub fn image_catalog(
    opentrons_url: &str,
    config: &ImageServeConfig,
    external_capture: Option<Arc<dyn CameraCapture>>,
    orbbec_capture: Option<Arc<dyn OrbbecCapture>>,
    still_cache: Option<&WarmStillCache>,
) -> Result<LiveImageCatalog, A2aLabError> {
    let opentrons = OpentronsCamera::new(
        opentrons_url,
        config.capture_timeout,
        config.max_image_bytes(),
    )?;
    let mut sources: Vec<(ImageSource, Arc<dyn ImageCapture>)> =
        vec![(opentrons_camera_source()?, Arc::new(opentrons))];
    if config.uses_resolved_native_sources() {
        for descriptor in &config.native_video_sources {
            let still = match &still_cache {
                Some(cache) => Arc::new(PumpLinkedStill::new(
                    &descriptor.id,
                    &descriptor.name,
                    config.max_image_bytes(),
                    (*cache).clone(),
                )?) as Arc<dyn ImageCapture>,
                None => Arc::new(ConfiguredNativeStill::from_descriptor(
                    descriptor,
                    config.capture_timeout(),
                    config.max_image_bytes(),
                    external_capture.clone(),
                    None,
                )?) as Arc<dyn ImageCapture>,
            };
            sources.push((configured_still_image_source(descriptor)?, still));
        }
    } else if let Some(cache) = still_cache {
        if config.camera_index().is_some() {
            sources.push((
                external_camera_source_named(&config.description, &config.external_name)?,
                Arc::new(PumpLinkedStill::new(
                    crate::camera::EXTERNAL_CAMERA_SOURCE_ID,
                    &config.description,
                    config.max_image_bytes(),
                    (*cache).clone(),
                )?) as Arc<dyn ImageCapture>,
            ));
        }
        if config.camera_enabled() {
            sources.push((
                camera_source_named(&config.webcam_name)?,
                Arc::new(CameraStill::with_still_cache(
                    config.max_image_bytes(),
                    (*cache).clone(),
                )?) as Arc<dyn ImageCapture>,
            ));
        }
    } else {
        if let Some(index) = config.camera_index() {
            let external = match external_capture {
                Some(capture) => ExternalCamera::with_shared_capture(
                    index,
                    config.capture_timeout,
                    config.max_image_bytes(),
                    capture,
                )?,
                None => {
                    ExternalCamera::new(index, config.capture_timeout, config.max_image_bytes())?
                }
            };
            sources.push((
                external_camera_source_named(&config.description, &config.external_name)?,
                Arc::new(external),
            ));
        }
        if config.camera_enabled() {
            sources.push((
                camera_source_named(&config.webcam_name)?,
                Arc::new(CameraStill::new(config.max_image_bytes())?),
            ));
        }
    }
    if config.orbbec_enabled() {
        let (capture, timeout) = match orbbec_capture {
            Some(capture) => (capture, config.capture_timeout),
            None => (
                Arc::new(SdkOrbbecCapture) as Arc<dyn OrbbecCapture>,
                config.capture_timeout.max(ORBBEC_FIRST_FRAME_WAIT),
            ),
        };
        let orbbec = OrbbecCamera::with_shared_capture(
            config.orbbec_serial(),
            timeout,
            config.max_image_bytes(),
            capture,
        )?;
        sources.push((orbbec_camera_source()?, Arc::new(orbbec)));
    }
    LiveImageCatalog::new(
        sources,
        ImageCatalogConfig::new(config.retention_per_source, config.max_image_bytes())?,
    )
}

/// Builds the serve catalog and opens history for every configured camera.
pub async fn open_image_catalog(
    opentrons_url: &str,
    config: &ImageServeConfig,
    external_capture: Option<Arc<dyn CameraCapture>>,
    orbbec_capture: Option<Arc<dyn OrbbecCapture>>,
    still_cache: Option<&WarmStillCache>,
) -> Result<LiveImageCatalog, A2aLabError> {
    let catalog = image_catalog(
        opentrons_url,
        config,
        external_capture,
        orbbec_capture,
        still_cache,
    )?;
    let mut sources = vec![ImageSourceId::new(OPENTRONS_CAMERA_SOURCE_ID)?];
    if config.uses_resolved_native_sources() {
        for descriptor in &config.native_video_sources {
            sources.push(ImageSourceId::new(&descriptor.id)?);
        }
    } else {
        if config.camera_index().is_some() {
            sources.push(ImageSourceId::new(EXTERNAL_CAMERA_SOURCE_ID)?);
        }
        if config.camera_enabled() {
            sources.push(ImageSourceId::new(CAMERA_SOURCE_ID)?);
        }
    }
    if config.orbbec_enabled() {
        sources.push(ImageSourceId::new(ORBBEC_CAMERA_SOURCE_ID)?);
    }
    catalog
        .with_persisted_sources(&sources, Path::new(DEFAULT_IMAGE_HISTORY_DB))
        .await
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
