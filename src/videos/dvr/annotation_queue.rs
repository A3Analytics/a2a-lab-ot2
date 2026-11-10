//! Bounded background reconcile plus one durable description/embedding worker.

use std::sync::Arc;
use std::time::Duration;

use a2a_lab_dev_kit::{A2aLabError, DvrStoreNamespace, SemanticIndexingConfig};
use tokio::sync::watch;

use super::annotation_worker::AnyIndexingAnnotationWorker;
use super::frame_manifest::DEFAULT_SEMANTIC_STORE_NAMESPACE;
use super::indexing_registration::DEFAULT_RECONCILE_BATCH;
use super::provider::VideoDvrCatalog;

/// Default delay between annotation reconcile batches while the catalog is live.
pub const ANNOTATION_RECONCILE_INTERVAL: Duration = Duration::from_secs(1);

/// Keeps one reconcile/worker task alive until dropped.
pub struct VideoDvrAnnotationQueue {
    task: tokio::task::JoinHandle<()>,
}

impl Drop for VideoDvrAnnotationQueue {
    fn drop(&mut self) {
        self.task.abort();
    }
}

/// Optional deterministic worker wired into the same queue as discovery reconcile.
#[derive(Clone)]
pub struct AnnotationQueueWorker {
    inner: Arc<AnyIndexingAnnotationWorker>,
    semantic: Arc<SemanticIndexingConfig>,
}

impl AnnotationQueueWorker {
    /// Shared worker configuration for one catalog lifetime.
    #[must_use]
    pub fn new(inner: AnyIndexingAnnotationWorker, semantic: SemanticIndexingConfig) -> Self {
        Self {
            inner: Arc::new(inner),
            semantic: Arc::new(semantic),
        }
    }
}

impl VideoDvrCatalog {
    /// Schedules bounded reconcile batches and optional model stages until `stop` is set.
    #[must_use]
    pub fn maintain_annotation_jobs(
        &self,
        mut stop: watch::Receiver<bool>,
        worker: Option<AnnotationQueueWorker>,
    ) -> VideoDvrAnnotationQueue {
        let catalog = self.clone();
        let store_ns = DvrStoreNamespace::new(DEFAULT_SEMANTIC_STORE_NAMESPACE).unwrap_or_else(
            |error: A2aLabError| panic!("default semantic store namespace: {error}"),
        );
        let task = tokio::spawn(async move {
            let mut interval = tokio::time::interval(ANNOTATION_RECONCILE_INTERVAL);
            interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
            loop {
                if *stop.borrow() {
                    break;
                }
                let paused = catalog.dvr().worker_meta_materialization_paused().await;
                if paused {
                    eprintln!("video DVR annotation reconcile paused: queue backlog at limit");
                } else if let Err(error) = catalog
                    .dvr()
                    .reconcile_indexing_jobs(&store_ns, DEFAULT_RECONCILE_BATCH)
                    .await
                {
                    eprintln!("video DVR annotation reconcile: {error}");
                }
                if let Some(worker) = worker.as_ref()
                    && let Err(error) = worker
                        .inner
                        .run_batch(catalog.dvr(), worker.semantic.as_ref())
                        .await
                {
                    eprintln!("video DVR annotation worker: {error}");
                }
                tokio::select! {
                    biased;
                    changed = stop.changed() => {
                        if changed.is_ok() && *stop.borrow() {
                            break;
                        }
                    }
                    _ = interval.tick() => {}
                }
            }
        });
        VideoDvrAnnotationQueue { task }
    }
}

impl super::store::IndexedVideoDvr {
    pub(crate) async fn worker_meta_materialization_paused(&self) -> bool {
        let value: Option<String> = sqlx::query_scalar(
            "SELECT meta_value FROM semantic_indexing_worker_meta WHERE meta_key = 'materialization_paused'",
        )
        .fetch_optional(self.sqlite_pool())
        .await
        .ok()
        .flatten();
        matches!(value.as_deref(), Some("true"))
    }
}
