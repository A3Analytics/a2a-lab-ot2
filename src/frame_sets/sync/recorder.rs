//! Optional continuous recorder of complete synchronized sets.

use std::sync::atomic::{AtomicU64, Ordering};

use super::store::{SyncChannel, SyncSet, SyncStore};
use crate::frame_sets::{CapturedFrameSet, FrameSetError, FrameSetSource};

/// Writes complete captured sets into synchronized history.
pub struct SyncRecorder {
    store: SyncStore,
    next_id: AtomicU64,
}

impl SyncRecorder {
    /// Records into `store`.
    #[must_use]
    pub fn new(store: SyncStore) -> Self {
        Self {
            store,
            next_id: AtomicU64::new(1),
        }
    }

    /// Shared store used by color video reads.
    #[must_use]
    pub fn store(&self) -> &SyncStore {
        &self.store
    }

    /// Inserts one complete frame. Color-only frames are stored during a stereo gap.
    pub async fn record(
        &self,
        source: &FrameSetSource,
        set: &CapturedFrameSet,
    ) -> Result<String, FrameSetError> {
        let id = format!("sync-{}", self.next_id.fetch_add(1, Ordering::Relaxed));
        let stored = SyncSet {
            id: id.clone(),
            source_id: source.id().as_str().to_owned(),
            serial: source.serial().to_owned(),
            captured_at: set.captured_at().to_owned(),
            timestamp_us: set.timestamp_us(),
            stereo: set.has_stereo(),
            channels: set
                .channels()
                .iter()
                .map(|channel| SyncChannel {
                    channel_id: channel.id().as_str().to_owned(),
                    media_type: channel.media_type().to_owned(),
                    payload: channel.data().to_vec(),
                })
                .collect(),
        };
        if set.color_jpeg().is_none() {
            return Err(FrameSetError::invalid(
                "channels",
                "a recorded set needs a color channel",
            ));
        }
        self.store.insert(&stored).await?;
        Ok(id)
    }
}
