//! Vendor-neutral catalog over a capture backend and SQLite history.

use std::path::Path;
use std::sync::{Arc, Mutex};

use super::error::FrameSetError;
use super::history::{FramePage, FrameSetHistory};
use super::id::{FrameChannelId, FrameSetId, FrameSetSourceId};
use super::model::{CaptureRequest, FrameChannel, FrameSet, FrameSetMetadata, FrameSetSource};
use super::store::FrameSetCapture;

/// Discovery, capture, and persisted history for frame-set sources.
pub struct FrameSetCatalog {
    sources: Mutex<Vec<FrameSetSource>>,
    capture: Arc<dyn FrameSetCapture>,
    history: FrameSetHistory,
    retention: usize,
}

impl FrameSetCatalog {
    /// Opens history and records the backend's current sources.
    ///
    /// Opening history does not capture a frame.
    pub async fn open(
        path: &Path,
        capture: Arc<dyn FrameSetCapture>,
        retention: usize,
        max_channel_bytes: u64,
    ) -> Result<Self, FrameSetError> {
        if retention == 0 {
            return Err(FrameSetError::invalid(
                "retention",
                "must keep at least one frame set",
            ));
        }
        let sources = capture.sources()?;
        let history = FrameSetHistory::open(path, max_channel_bytes).await?;
        Ok(Self {
            sources: Mutex::new(sources),
            capture,
            history,
            retention,
        })
    }

    /// Opens history without discovering devices.
    ///
    /// The first capture records the backend's sources. Listing stored sets does
    /// not open a camera.
    pub async fn open_deferred(
        path: &Path,
        capture: Arc<dyn FrameSetCapture>,
        retention: usize,
        max_channel_bytes: u64,
    ) -> Result<Self, FrameSetError> {
        if retention == 0 {
            return Err(FrameSetError::invalid(
                "retention",
                "must keep at least one frame set",
            ));
        }
        let history = FrameSetHistory::open(path, max_channel_bytes).await?;
        Ok(Self {
            sources: Mutex::new(Vec::new()),
            capture,
            history,
            retention,
        })
    }

    /// Configured sources.
    #[must_use]
    pub fn sources(&self) -> Vec<FrameSetSource> {
        self.guard().clone()
    }

    /// Capture backend. Callers can run it off the async runtime.
    #[must_use]
    pub fn capture_backend(&self) -> Arc<dyn FrameSetCapture> {
        Arc::clone(&self.capture)
    }

    /// Discovers sources and remembers them. This does not capture a frame.
    pub fn load_sources(&self) -> Result<Vec<FrameSetSource>, FrameSetError> {
        self.remember_sources()?;
        Ok(self.sources())
    }

    /// Remembers sources that a caller already discovered.
    pub fn replace_sources(&self, sources: Vec<FrameSetSource>) {
        *self.guard() = sources;
    }

    /// Captures one synchronized set and commits it atomically.
    pub async fn capture(&self, request: &CaptureRequest) -> Result<FrameSet, FrameSetError> {
        let captured = self.capture.capture(request)?;
        self.publish_captured(request, captured).await
    }

    /// Publishes a set that was already captured.
    pub async fn publish_captured(
        &self,
        request: &CaptureRequest,
        captured: super::model::CapturedFrameSet,
    ) -> Result<FrameSet, FrameSetError> {
        self.remember_sources()?;
        let source = self.source(request.source_id())?;
        let id = self.history.next_id().await?;
        let set = captured.publish(
            id,
            &source,
            request.channel_ids(),
            request.max_channel_bytes(),
        )?;
        self.history.insert(&set, self.retention).await?;
        Ok(set)
    }

    /// Removes one published set. Used when a paired color store fails.
    pub async fn remove(&self, id: &FrameSetId) -> Result<(), FrameSetError> {
        self.history.remove(id).await
    }

    /// Stored metadata for one source, oldest first.
    pub async fn list(
        &self,
        source_id: &FrameSetSourceId,
        cursor: Option<&str>,
        limit: u32,
    ) -> Result<FramePage<FrameSetMetadata>, FrameSetError> {
        self.history.list(Some(source_id), "", cursor, limit).await
    }

    /// Stored metadata matching `query`.
    pub async fn search(
        &self,
        source_id: Option<&FrameSetSourceId>,
        query: &str,
        cursor: Option<&str>,
        limit: u32,
    ) -> Result<FramePage<FrameSetMetadata>, FrameSetError> {
        self.history.list(source_id, query, cursor, limit).await
    }

    /// One stored set.
    pub async fn get(&self, id: &FrameSetId) -> Result<FrameSet, FrameSetError> {
        self.history.get(id).await
    }

    /// One stored channel.
    pub async fn channel(
        &self,
        id: &FrameSetId,
        channel_id: &FrameChannelId,
    ) -> Result<FrameChannel, FrameSetError> {
        self.history.channel(id, channel_id).await
    }

    #[cfg(test)]
    pub(crate) fn fail_next_write(&self) {
        self.history.fail_next_write();
    }

    fn remember_sources(&self) -> Result<(), FrameSetError> {
        let latest = self.capture.sources()?;
        *self.guard() = latest;
        Ok(())
    }

    fn source(&self, id: &FrameSetSourceId) -> Result<FrameSetSource, FrameSetError> {
        self.sources()
            .into_iter()
            .find(|source| source.id() == id)
            .ok_or_else(|| FrameSetError::NotFound {
                kind: "frame-set source",
                id: id.to_string(),
            })
    }

    fn guard(&self) -> std::sync::MutexGuard<'_, Vec<FrameSetSource>> {
        self.sources
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::frame_sets::{
        ChannelCalibration, ChannelProfile, ChannelRole, DepthScale, DepthUnit, FrameChannelId,
        FrameChannelParts, frame_channel,
    };
    use serde_json::json;

    struct One {
        source: FrameSetSource,
    }

    impl FrameSetCapture for One {
        fn sources(&self) -> Result<Vec<FrameSetSource>, FrameSetError> {
            Ok(vec![self.source.clone()])
        }

        fn capture(
            &self,
            request: &CaptureRequest,
        ) -> Result<crate::frame_sets::CapturedFrameSet, FrameSetError> {
            let color = frame_channel(FrameChannelParts {
                id: FrameChannelId::new("color").unwrap(),
                role: ChannelRole::color(),
                media_type: "image/jpeg".to_owned(),
                width: 1,
                height: 1,
                bit_depth: 8,
                device_timestamp_us: 1,
                frame_number: 1,
                depth: None,
                calibration: ChannelCalibration::default(),
                attributes: serde_json::Map::new(),
                data: vec![0xFF, 0xD8, 0xFF, 0xD9],
            });
            let depth = frame_channel(FrameChannelParts {
                id: FrameChannelId::new("depth").unwrap(),
                role: ChannelRole::depth(),
                media_type: "image/png".to_owned(),
                width: 1,
                height: 1,
                bit_depth: 16,
                device_timestamp_us: 1,
                frame_number: 1,
                depth: Some(DepthScale::new(1.0, DepthUnit::Millimeter).unwrap()),
                calibration: ChannelCalibration::default(),
                attributes: json!({}).as_object().cloned().unwrap(),
                data: png_header(),
            });
            crate::frame_sets::CapturedFrameSet::new(
                &self.source,
                "2026-10-30T00:00:00Z",
                vec![color, depth],
                request.channel_ids(),
                serde_json::Map::new(),
                request.max_channel_bytes(),
            )
        }
    }

    fn png_header() -> Vec<u8> {
        let mut data = b"\x89PNG\r\n\x1a\n".to_vec();
        data.extend_from_slice(&13u32.to_be_bytes());
        data.extend_from_slice(b"IHDR");
        data.extend_from_slice(&1u32.to_be_bytes());
        data.extend_from_slice(&1u32.to_be_bytes());
        data.extend_from_slice(&[16, 0, 0, 0, 0, 0, 0, 0, 0]);
        data
    }

    #[tokio::test]
    async fn failed_write_leaves_prior_history() {
        let path = std::env::temp_dir().join(format!("frame-rollback-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&path);
        std::fs::create_dir_all(&path).unwrap();
        let source = FrameSetSource::new(
            FrameSetSourceId::new("cam").unwrap(),
            "generic",
            "serial",
            "model",
            vec![
                ChannelProfile::new(
                    FrameChannelId::new("color").unwrap(),
                    ChannelRole::color(),
                    "image/jpeg",
                    1,
                    1,
                    8,
                    30.0,
                )
                .unwrap(),
                ChannelProfile::new(
                    FrameChannelId::new("depth").unwrap(),
                    ChannelRole::depth(),
                    "image/png",
                    1,
                    1,
                    16,
                    30.0,
                )
                .unwrap(),
            ],
            "group",
            10,
        )
        .unwrap();
        let catalog = FrameSetCatalog::open(
            &path.join("frames.sqlite"),
            Arc::new(One { source }),
            4,
            1024,
        )
        .await
        .unwrap();
        let request =
            CaptureRequest::new(catalog.sources()[0].id().clone(), Vec::new(), 10, 1024).unwrap();
        let first = catalog.capture(&request).await.unwrap();
        catalog.fail_next_write();
        assert!(catalog.capture(&request).await.is_err());
        assert_eq!(catalog.get(first.id()).await.unwrap().id(), first.id());
    }
}
