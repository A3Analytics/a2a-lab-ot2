//! Serve-mode video configuration for native cameras.

use std::path::{Path, PathBuf};

use a2a_lab_dev_kit::A2aLabError;

use crate::camera::DEFAULT_EXTERNAL_CAMERA_DESCRIPTION;
use crate::lab_config::ResolvedVideoSettings;
use crate::videos::catalog::{
    ExternalVideoConfig, LiveVideoCatalog, LiveVideoInjections, WebcamVideoConfig,
    start_live_video_catalog, start_live_videos,
};
use crate::videos::dvr::DEFAULT_DISK_VIDEO_RETENTION_SECS;
use crate::videos::external::LiveJpegSource;
use crate::videos::source::LiveCameraInputFormat;

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
                LiveCameraInputFormat::Mjpeg => "mjpeg",
                LiveCameraInputFormat::Yuy2 => "yuy2",
            };
            format!(
                "{} index {} {input} name {}",
                source.id, source.device_index, source.name
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

/// Starts the live catalog from resolved configuration when any source is listed.
pub async fn open_resolved_video_catalog(
    settings: &ResolvedVideoSettings,
    injections: LiveVideoInjections,
) -> Result<Option<LiveVideoCatalog>, A2aLabError> {
    if settings.sources.is_empty() {
        return Ok(None);
    }
    Ok(Some(
        start_live_video_catalog(
            &settings.sources,
            settings.fps,
            settings.live_retention_secs,
            settings.disk_retention_secs,
            &settings.dvr_storage.path,
            settings.max_image_bytes,
            injections,
        )
        .await?,
    ))
}

/// Starts the live catalog when at least one camera source is configured.
pub async fn open_video_catalog(
    config: &VideoServeConfig,
    external_source: Option<Box<dyn LiveJpegSource>>,
    webcam_source: Option<Box<dyn LiveJpegSource>>,
) -> Result<Option<LiveVideoCatalog>, A2aLabError> {
    let external = config.camera_index.map(|index| ExternalVideoConfig {
        index,
        description: &config.description,
    });
    let webcam = config.webcam_index.map(|index| WebcamVideoConfig { index });
    if external.is_none() && webcam.is_none() {
        return Ok(None);
    }
    Ok(Some(
        start_live_videos(
            external,
            webcam,
            config.fps,
            config.live_retention_secs,
            config.disk_retention_secs,
            &config.disk_root,
            config.max_image_bytes,
            LiveVideoInjections::legacy_jpeg(external_source, webcam_source),
        )
        .await?,
    ))
}
