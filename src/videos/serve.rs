//! Serve-mode video configuration for the external camera.

use a2a_lab_dev_kit::A2aLabError;

use crate::camera::DEFAULT_EXTERNAL_CAMERA_DESCRIPTION;
use crate::videos::catalog::{LiveVideoCatalog, start_external_video};
use crate::videos::external::LiveJpegSource;

/// Default live catalog frame rate.
pub const DEFAULT_VIDEO_FPS: u32 = 4;
/// Default in-memory retention window.
pub const DEFAULT_VIDEO_RETENTION_SECS: u64 = 120;

/// Timeout-free video limits. Building this does not open a camera.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VideoServeConfig {
    camera_index: Option<u32>,
    description: String,
    fps: u32,
    retention_secs: u64,
    max_image_bytes: u64,
}

impl VideoServeConfig {
    /// Rejects a zero fps or retention. `None` leaves video off.
    pub fn new(
        camera_index: Option<u32>,
        description: Option<&str>,
        fps: u32,
        retention_secs: u64,
        max_image_bytes: u64,
    ) -> Result<Self, A2aLabError> {
        if fps == 0 {
            return Err(A2aLabError::invalid(
                "video_fps",
                "must be greater than zero",
            ));
        }
        if retention_secs == 0 {
            return Err(A2aLabError::invalid(
                "video_retention_secs",
                "must be greater than zero",
            ));
        }
        let description = match description {
            None => DEFAULT_EXTERNAL_CAMERA_DESCRIPTION.to_owned(),
            Some("") => return Err(A2aLabError::invalid("description", "must not be empty")),
            Some(text) => text.to_owned(),
        };
        Ok(Self {
            camera_index,
            description,
            fps,
            retention_secs,
            max_image_bytes,
        })
    }

    /// Native camera index when video is enabled.
    #[must_use]
    pub const fn camera_index(&self) -> Option<u32> {
        self.camera_index
    }

    /// One startup line.
    #[must_use]
    pub fn summary(&self) -> String {
        match self.camera_index {
            Some(index) => format!(
                "videos       external-camera index {index} fps {} retention {}s",
                self.fps, self.retention_secs
            ),
            None => "videos       off".to_owned(),
        }
    }
}

/// Starts the live catalog when an external camera index is configured.
pub async fn open_video_catalog(
    config: &VideoServeConfig,
    source: Option<Box<dyn LiveJpegSource>>,
) -> Result<Option<LiveVideoCatalog>, A2aLabError> {
    let Some(index) = config.camera_index else {
        return Ok(None);
    };
    Ok(Some(
        start_external_video(
            index,
            &config.description,
            config.fps,
            config.retention_secs,
            config.max_image_bytes,
            source,
        )
        .await?,
    ))
}
