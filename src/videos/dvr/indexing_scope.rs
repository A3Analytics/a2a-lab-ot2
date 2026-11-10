//! Optional source allow-list for semantic indexing jobs.

use std::collections::HashSet;
use std::sync::Arc;

/// When set, only these DVR source ids receive durable indexing jobs.
#[derive(Debug, Clone, Default)]
pub struct IndexedSourceFilter {
    inner: Arc<Option<HashSet<String>>>,
}

impl IndexedSourceFilter {
    /// Indexes every saved source (legacy reconcile behavior).
    #[must_use]
    pub fn all_sources() -> Self {
        Self {
            inner: Arc::new(None),
        }
    }

    /// Indexes only configured live video sources.
    #[must_use]
    pub fn allow_only(source_ids: HashSet<String>) -> Self {
        Self {
            inner: Arc::new(Some(source_ids)),
        }
    }

    #[must_use]
    pub fn permits(&self, source_id: &str) -> bool {
        match self.inner.as_ref() {
            None => true,
            Some(allowed) => allowed.contains(source_id),
        }
    }
}
