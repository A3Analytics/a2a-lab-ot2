//! Capture and atomic publication.

use std::collections::BTreeMap;
use std::future::Future;
use std::sync::Mutex;

use super::error::FrameSetError;
use super::id::FrameSetId;
use super::model::{CaptureRequest, CapturedFrameSet, FrameSet, FrameSetSource};

/// Reads synchronized frames from one backend.
pub trait FrameSetCapture: Send + Sync {
    /// Sources this backend can capture.
    fn sources(&self) -> Result<Vec<FrameSetSource>, FrameSetError>;

    /// Captures one complete set or returns an error without a partial result.
    fn capture(&self, request: &CaptureRequest) -> Result<CapturedFrameSet, FrameSetError>;
}

/// Stores a validated set atomically.
pub trait FrameSetStore: Send + Sync {
    /// Publishes `set` completely or leaves prior history unchanged.
    fn publish<'a>(
        &'a self,
        set: &'a FrameSet,
    ) -> impl Future<Output = Result<(), FrameSetError>> + Send + 'a;

    /// Reads one published set.
    fn read<'a>(
        &'a self,
        id: &'a FrameSetId,
    ) -> impl Future<Output = Result<FrameSet, FrameSetError>> + Send + 'a;
}

/// Publishes a capture only after the set validates.
pub async fn publish_capture(
    capture: &impl FrameSetCapture,
    store: &impl FrameSetStore,
    source: &FrameSetSource,
    request: &CaptureRequest,
    id: FrameSetId,
) -> Result<FrameSet, FrameSetError> {
    let captured = capture.capture(request)?;
    let set = captured.publish(
        id,
        source,
        request.channel_ids(),
        request.max_channel_bytes(),
    )?;
    store.publish(&set).await?;
    Ok(set)
}

/// In-memory store that inserts a set in one map update.
#[derive(Debug, Default)]
pub struct MemoryFrameSetStore {
    sets: Mutex<BTreeMap<String, FrameSet>>,
}

impl MemoryFrameSetStore {
    /// Creates an empty store.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }
}

impl FrameSetStore for MemoryFrameSetStore {
    #[allow(clippy::unused_async_trait_impl)]
    async fn publish(&self, set: &FrameSet) -> Result<(), FrameSetError> {
        let mut sets = self.sets.lock().map_err(|_| {
            FrameSetError::Unavailable("frame-set store lock was poisoned".to_owned())
        })?;
        if sets.contains_key(set.id().as_str()) {
            return Err(FrameSetError::invalid(
                "frame_set_id",
                "is already published",
            ));
        }
        sets.insert(set.id().as_str().to_owned(), set.clone());
        Ok(())
    }

    #[allow(clippy::unused_async_trait_impl)]
    async fn read(&self, id: &FrameSetId) -> Result<FrameSet, FrameSetError> {
        let sets = self.sets.lock().map_err(|_| {
            FrameSetError::Unavailable("frame-set store lock was poisoned".to_owned())
        })?;
        sets.get(id.as_str())
            .cloned()
            .ok_or_else(|| FrameSetError::NotFound {
                kind: "frame set",
                id: id.to_string(),
            })
    }
}
