//! Serve-mode video configuration for native cameras.

use std::path::{Path, PathBuf};

use a2a_lab_dev_kit::A2aLabError;

use crate::camera::DEFAULT_EXTERNAL_CAMERA_DESCRIPTION;
use crate::videos::catalog::{
    ExternalVideoConfig, LiveVideoCatalog, LiveVideoInjections, OrbbecVideoConfig,
    start_live_videos,
};
use crate::videos::dvr::DEFAULT_DISK_VIDEO_RETENTION_SECS;
use crate::videos::external::LiveJpegSource;

/// Default live catalog frame rate.
pub const DEFAULT_VIDEO_FPS: u32 = 4;
/// Default in-memory live window (distinct from disk retention).
pub const DEFAULT_LIVE_VIDEO_MEMORY_SECS: u64 = 5;
/// Default in-memory retention passed to CLI (`--video-retention-secs`).
pub const DEFAULT_VIDEO_RETENTION_SECS: u64 = DEFAULT_LIVE_VIDEO_MEMORY_SECS;

/// Orbbec device selection for live video.
#[derive(Debug, Clone, PartialEq, Eq)]
struct OrbbecVideoSelection {
    serial: Option<String>,
}

/// Timeout-free video limits. Building this does not open a camera.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VideoServeConfig {
    camera_index: Option<u32>,
    description: String,
    orbbec: Option<OrbbecVideoSelection>,
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
            orbbec: None,
            fps,
            live_retention_secs,
            disk_retention_secs,
            disk_root,
            max_image_bytes,
        })
    }

    /// Enables `orbbec-camera` live video without opening a device.
    pub fn with_orbbec(mut self, serial: Option<&str>) -> Result<Self, A2aLabError> {
        if serial.is_some_and(str::is_empty) {
            return Err(A2aLabError::invalid("orbbec_serial", "must not be empty"));
        }
        self.orbbec = Some(OrbbecVideoSelection {
            serial: serial.map(str::to_owned),
        });
        Ok(self)
    }

    /// Native camera index when the external source is enabled.
    #[must_use]
    pub const fn camera_index(&self) -> Option<u32> {
        self.camera_index
    }

    /// Whether `orbbec-camera` live video is configured.
    #[must_use]
    pub const fn orbbec_enabled(&self) -> bool {
        self.orbbec.is_some()
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
        if self.orbbec.is_some() {
            let serial = self
                .orbbec
                .as_ref()
                .and_then(|selection| selection.serial.as_deref())
                .map(|value| format!(" serial {value}"))
                .unwrap_or_default();
            parts.push(format!(
                "orbbec-camera{serial} fps {} memory {}s disk {}s",
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

/// Starts the live catalog when at least one camera source is configured.
pub async fn open_video_catalog(
    config: &VideoServeConfig,
    external_source: Option<Box<dyn LiveJpegSource>>,
    orbbec_source: Option<Box<dyn LiveJpegSource>>,
) -> Result<Option<LiveVideoCatalog>, A2aLabError> {
    let external = config.camera_index.map(|index| ExternalVideoConfig {
        index,
        description: &config.description,
    });
    let orbbec = config.orbbec.as_ref().map(|selection| OrbbecVideoConfig {
        serial: selection.serial.clone(),
    });
    if external.is_none() && orbbec.is_none() {
        return Ok(None);
    }
    Ok(Some(
        start_live_videos(
            external,
            orbbec,
            config.fps,
            config.live_retention_secs,
            config.disk_retention_secs,
            &config.disk_root,
            config.max_image_bytes,
            LiveVideoInjections {
                external: external_source,
                orbbec: orbbec_source,
            },
        )
        .await?,
    ))
}
