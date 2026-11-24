//! Serve-mode video configuration for native cameras.

use std::path::{Path, PathBuf};

use a2a_lab_dev_kit::{
    A2aLabError, A2aLabService, ImageProvider, LogProvider, MetricProvider, TaskProvider,
    VideoProvider,
};

use crate::camera::DEFAULT_EXTERNAL_CAMERA_DESCRIPTION;
use crate::lab_config::{
    LabConfigInputs, ResolvedDvrStorage, ResolvedSemanticIndexing, ResolvedVideoSettings,
    SettingProvenance,
};
use crate::videos::declarative_serve::{
    DeclarativeVideoServe, start_legacy_declarative_serve, start_resolved_declarative_serve,
};
use crate::videos::dvr::DEFAULT_DISK_VIDEO_RETENTION_SECS;
use crate::videos::external::LiveJpegSource;
use crate::videos::injections::{ExternalVideoConfig, LiveVideoInjections, WebcamVideoConfig};
use crate::videos::semantic_indexing::SemanticServeInjections;
use crate::videos::source::{LiveCameraInputFormat, LiveVideoDelivery};
use crate::videos::source::{LiveVideoSourceDescriptor, validate_live_sources};

/// Default live catalog frame rate.
pub const DEFAULT_VIDEO_FPS: u32 = 5;
/// Default in-memory live window (distinct from disk retention).
pub const DEFAULT_LIVE_VIDEO_MEMORY_SECS: u64 = 5;
/// Default in-memory retention passed to CLI (`--video-retention-secs`).
pub const DEFAULT_VIDEO_RETENTION_SECS: u64 = DEFAULT_LIVE_VIDEO_MEMORY_SECS;

/// Timeout-free video limits. Building this does not open a camera.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VideoServeConfig {
    camera_index: Option<u32>,
    description: String,
    webcam_index: Option<u32>,
    fps: u32,
    live_retention_secs: u64,
    disk_retention_secs: u64,
    disk_root: PathBuf,
    max_image_bytes: u64,
}

impl VideoServeConfig {
    /// Rejects a zero fps or retention. `None` leaves the external source off.
    pub fn new(
        camera_index: Option<u32>,
        description: Option<&str>,
        fps: u32,
        live_retention_secs: u64,
        max_image_bytes: u64,
    ) -> Result<Self, A2aLabError> {
        Self::with_disk(
            camera_index,
            description,
            fps,
            live_retention_secs,
            DEFAULT_DISK_VIDEO_RETENTION_SECS,
            PathBuf::from(crate::videos::dvr::DEFAULT_VIDEO_DVR_ROOT),
            max_image_bytes,
        )
    }

    /// Full configuration including indexed disk retention and storage root.
    pub fn with_disk(
        camera_index: Option<u32>,
        description: Option<&str>,
        fps: u32,
        live_retention_secs: u64,
        disk_retention_secs: u64,
        disk_root: PathBuf,
        max_image_bytes: u64,
    ) -> Result<Self, A2aLabError> {
        if fps == 0 {
            return Err(A2aLabError::invalid(
                "video_fps",
                "must be greater than zero",
            ));
        }
        if live_retention_secs == 0 {
            return Err(A2aLabError::invalid(
                "video_retention_secs",
                "must be greater than zero",
            ));
        }
        if disk_retention_secs == 0 {
            return Err(A2aLabError::invalid(
                "video_disk_retention_secs",
                "must be greater than zero",
            ));
        }
        if disk_root.as_os_str().is_empty() {
            return Err(A2aLabError::invalid("video_dvr_root", "must not be empty"));
        }
        let description = match description {
            None => DEFAULT_EXTERNAL_CAMERA_DESCRIPTION.to_owned(),
            Some("") => return Err(A2aLabError::invalid("description", "must not be empty")),
            Some(text) => text.to_owned(),
        };
        Ok(Self {
            camera_index,
            description,
            webcam_index: None,
            fps,
            live_retention_secs,
            disk_retention_secs,
            disk_root,
            max_image_bytes,
        })
    }

    /// Enables source `camera` at `index` without opening a device.
    #[must_use]
    pub fn with_camera_index(mut self, index: u32) -> Self {
        self.webcam_index = Some(index);
        self
    }

    /// V4L2 index for source `camera`.
    #[must_use]
    pub const fn webcam_index(&self) -> Option<u32> {
        self.webcam_index
    }

    /// Native camera index when the external source is enabled.
    #[must_use]
    pub const fn camera_index(&self) -> Option<u32> {
        self.camera_index
    }

    #[must_use]
    pub fn disk_root(&self) -> &Path {
        &self.disk_root
    }

    #[must_use]
    pub const fn disk_retention_secs(&self) -> u64 {
        self.disk_retention_secs
    }

    #[must_use]
    pub const fn live_retention_secs(&self) -> u64 {
        self.live_retention_secs
    }

    #[must_use]
    pub const fn fps(&self) -> u32 {
        self.fps
    }

    #[must_use]
    pub const fn max_image_bytes(&self) -> u64 {
        self.max_image_bytes
    }

    /// Advertised description for legacy `external-camera`.
    #[must_use]
    pub fn description(&self) -> &str {
        &self.description
    }

    /// One startup line.
    #[must_use]
    pub fn summary(&self) -> String {
        let mut parts = Vec::new();
        if let Some(index) = self.camera_index {
            parts.push(format!(
                "external-camera index {index} fps {} memory {}s disk {}s",
                self.fps, self.live_retention_secs, self.disk_retention_secs
            ));
        }
        if let Some(index) = self.webcam_index {
            parts.push(format!(
                "camera index {index} fps {} memory {}s disk {}s",
                self.fps, self.live_retention_secs, self.disk_retention_secs
            ));
        }
        if parts.is_empty() {
            "videos       off".to_owned()
        } else {
            format!(
                "videos       {} dvr {}",
                parts.join("; "),
                self.disk_root.display()
            )
        }
    }
}

/// One startup line for resolved video settings.
#[must_use]
pub fn resolved_video_summary(settings: &ResolvedVideoSettings) -> String {
    if settings.sources.is_empty() {
        return "videos       off".to_owned();
    }
    let parts = settings
        .sources
        .iter()
        .map(|source| {
            let input = match source.input {
                LiveCameraInputFormat::Auto => "auto",
                LiveCameraInputFormat::Mjpeg => "mjpeg",
                LiveCameraInputFormat::Yuy2 => "yuy2",
            };
            let delivery = match source.delivery {
                LiveVideoDelivery::Mjpeg => "mjpeg",
                LiveVideoDelivery::H264 => "h264",
            };
            format!(
                "{} camera_id {} input {input} delivery {delivery} name {}",
                source.id,
                source.camera_id_label(),
                source.name
            )
        })
        .collect::<Vec<_>>();
    format!(
        "videos       {} fps {} memory {}s disk {}s dvr {}",
        parts.join("; "),
        settings.fps,
        settings.live_retention_secs,
        settings.disk_retention_secs,
        settings.dvr_storage.path.display()
    )
}

/// Starts declarative runtime for injected integration tests.
#[allow(clippy::too_many_arguments)]
pub async fn start_fixture_declarative_serve(
    sources: Vec<LiveVideoSourceDescriptor>,
    fps: u32,
    live_retention_secs: u64,
    disk_retention_secs: u64,
    disk_root: &Path,
    max_image_bytes: u64,
    dvr_save_interval_nanos: Option<u64>,
    injections: LiveVideoInjections,
) -> Result<DeclarativeVideoServe, A2aLabError> {
    Box::pin(start_fixture_declarative_serve_inner(
        sources,
        fps,
        live_retention_secs,
        disk_retention_secs,
        disk_root,
        max_image_bytes,
        dvr_save_interval_nanos,
        injections,
    ))
    .await
}

#[allow(clippy::too_many_arguments)]
async fn start_fixture_declarative_serve_inner(
    sources: Vec<LiveVideoSourceDescriptor>,
    fps: u32,
    live_retention_secs: u64,
    disk_retention_secs: u64,
    disk_root: &Path,
    max_image_bytes: u64,
    dvr_save_interval_nanos: Option<u64>,
    injections: LiveVideoInjections,
) -> Result<DeclarativeVideoServe, A2aLabError> {
    validate_live_sources(&sources)?;
    let settings = ResolvedVideoSettings {
        sources,
        fps,
        live_retention_secs,
        disk_retention_secs,
        dvr_save_interval_nanos,
        dvr_storage: ResolvedDvrStorage {
            path: disk_root.to_path_buf(),
            provenance: SettingProvenance::ApplicationDefault,
        },
        max_image_bytes,
    };
    open_resolved_video_catalog(&settings, injections)
        .await?
        .ok_or_else(|| A2aLabError::invalid("video", "no live video sources configured"))
}

/// Starts declarative runtime from resolved configuration when any source is listed.
pub async fn open_resolved_video_catalog(
    settings: &ResolvedVideoSettings,
    injections: LiveVideoInjections,
) -> Result<Option<DeclarativeVideoServe>, A2aLabError> {
    open_resolved_video_catalog_with_semantic(settings, injections, None, None).await
}

/// Starts declarative runtime and wires optional v2 semantic indexing before capture runs.
pub async fn open_resolved_video_catalog_with_semantic(
    settings: &ResolvedVideoSettings,
    injections: LiveVideoInjections,
    inputs: Option<&LabConfigInputs>,
    semantic_injections: Option<SemanticServeInjections>,
) -> Result<Option<DeclarativeVideoServe>, A2aLabError> {
    if settings.sources.is_empty() {
        return Ok(None);
    }
    let semantic = match inputs {
        Some(inputs) => crate::lab_config::resolve_semantic_indexing(inputs, settings)?,
        None => None,
    };
    Ok(Some(
        start_resolved_declarative_serve(
            settings,
            injections,
            semantic.as_ref(),
            semantic_injections.unwrap_or_default(),
        )
        .await?,
    ))
}

/// Registers the devkit generic image-decision client when configuration enables it.
///
/// The typed frame-decision ledger and evaluation worker are not changed.
pub async fn attach_configured_image_decision<L, M, W, I, V>(
    service: A2aLabService<L, M, W, I, V>,
    inputs: &LabConfigInputs,
) -> Result<A2aLabService<L, M, W, I, V>, A2aLabError>
where
    L: LogProvider,
    M: MetricProvider,
    W: TaskProvider,
    I: ImageProvider,
    V: VideoProvider,
{
    let Some(path) = crate::lab_config::selected_config_path(inputs)? else {
        return Ok(service);
    };
    let loaded = a2a_lab_dev_kit::LoadedLabConfig::load(&path)?;
    let Some(client_config) = loaded.config.image_decision.clone() else {
        return Ok(service);
    };
    let Some(settings) = client_config.agentcore().cloned() else {
        return service.resolve_image_decision_from_loaded(&loaded);
    };
    let client = a2a_lab_dev_kit::ResolvedImageDecisionClient::from_config(client_config);
    let executor = crate::images::AgentCoreImageDecisionExecutor::<
        crate::images::SdkAgentCoreInvoker,
    >::connect(
        &settings,
        client.config().timeout(),
        client.config().max_image_bytes(),
        client.config().max_request_bytes(),
        client.config().max_response_bytes(),
    )
    .await?;
    Ok(service
        .with_image_decision_client(client)
        .with_image_decision_executor(std::sync::Arc::new(executor)))
}

/// Resolved semantic settings for serve when a configuration file is selected.
pub async fn resolve_serve_semantic_indexing(
    inputs: &LabConfigInputs,
    settings: &ResolvedVideoSettings,
) -> Result<Option<ResolvedSemanticIndexing>, A2aLabError> {
    crate::lab_config::resolve_semantic_indexing(inputs, settings)
}

/// Legacy two-slot declarative startup for integration tests.
#[allow(clippy::too_many_arguments)]
pub async fn start_legacy_fixture_videos(
    external: Option<ExternalVideoConfig<'_>>,
    webcam: Option<WebcamVideoConfig>,
    fps: u32,
    live_retention_secs: u64,
    disk_retention_secs: u64,
    disk_root: &Path,
    max_image_bytes: u64,
    injections: LiveVideoInjections,
) -> Result<DeclarativeVideoServe, A2aLabError> {
    let mut config = VideoServeConfig::with_disk(
        external.as_ref().map(|entry| entry.index),
        external.as_ref().map(|entry| entry.description),
        fps,
        live_retention_secs,
        disk_retention_secs,
        disk_root.to_path_buf(),
        max_image_bytes,
    )?;
    if let Some(webcam) = webcam {
        config = config.with_camera_index(webcam.index);
    }
    start_legacy_declarative_serve(&config, injections).await
}

/// Starts declarative runtime when at least one camera source is configured.
pub async fn open_video_catalog(
    config: &VideoServeConfig,
    external_source: Option<Box<dyn LiveJpegSource>>,
    webcam_source: Option<Box<dyn LiveJpegSource>>,
) -> Result<Option<DeclarativeVideoServe>, A2aLabError> {
    let external = config.camera_index.map(|index| ExternalVideoConfig {
        index,
        description: config.description(),
    });
    let webcam = config.webcam_index.map(|index| WebcamVideoConfig { index });
    if external.is_none() && webcam.is_none() {
        return Ok(None);
    }
    Ok(Some(
        start_legacy_declarative_serve(
            config,
            LiveVideoInjections::legacy_jpeg(external_source, webcam_source),
        )
        .await?,
    ))
}
