//! Injectable faults for durable indexing worker tests.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

/// Test-only faults on the annotation worker path.
#[derive(Debug, Default)]
pub struct IndexingWorkerFaults {
    crash_before_description_commit: AtomicBool,
    crash_after_description_commit: AtomicBool,
    crash_before_embedding_commit: AtomicBool,
    force_embedding_failure: AtomicBool,
}

impl IndexingWorkerFaults {
    #[must_use]
    pub fn new() -> Arc<Self> {
        Arc::new(Self::default())
    }

    pub fn set_crash_before_description_commit(&self, value: bool) {
        self.crash_before_description_commit
            .store(value, Ordering::Relaxed);
    }

    #[must_use]
    pub fn crash_before_description_commit(&self) -> bool {
        self.crash_before_description_commit.load(Ordering::Relaxed)
    }

    pub fn set_crash_after_description_commit(&self, value: bool) {
        self.crash_after_description_commit
            .store(value, Ordering::Relaxed);
    }

    #[must_use]
    pub fn crash_after_description_commit(&self) -> bool {
        self.crash_after_description_commit.load(Ordering::Relaxed)
    }

    pub fn set_crash_before_embedding_commit(&self, value: bool) {
        self.crash_before_embedding_commit
            .store(value, Ordering::Relaxed);
    }

    #[must_use]
    pub fn crash_before_embedding_commit(&self) -> bool {
        self.crash_before_embedding_commit.load(Ordering::Relaxed)
    }

    pub fn set_force_embedding_failure(&self, value: bool) {
        self.force_embedding_failure.store(value, Ordering::Relaxed);
    }

    #[must_use]
    pub fn force_embedding_failure(&self) -> bool {
        self.force_embedding_failure.load(Ordering::Relaxed)
    }
}
