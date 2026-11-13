//! OT-2 semantic manifest and indexing on [`DvrPersistHook`].

use std::sync::{Arc, OnceLock};

use a2a_lab_dev_kit::{A2aLabError, DvrCommittedFrame, DvrPersistHook};
use async_trait::async_trait;

use super::store::IndexedVideoDvr;

/// Post-commit semantic sidecar wired at DVR open.
pub(crate) struct SemanticPersistHook {
    store: Arc<OnceLock<IndexedVideoDvr>>,
}

impl SemanticPersistHook {
    #[must_use]
    pub fn new(store: Arc<OnceLock<IndexedVideoDvr>>) -> Self {
        Self { store }
    }
}

#[async_trait]
impl DvrPersistHook for SemanticPersistHook {
    async fn after_frame_committed(&self, frame: &DvrCommittedFrame) -> Result<(), A2aLabError> {
        let Some(store) = self.store.get() else {
            return Err(A2aLabError::unavailable(
                "indexed DVR hook before store init",
            ));
        };
        match store.on_frame_committed(frame).await {
            Ok(()) => Ok(()),
            Err(error) => {
                store.record_hook_failure(error.clone());
                Err(error)
            }
        }
    }
}
