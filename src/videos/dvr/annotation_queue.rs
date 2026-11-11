//! Bounded background reconcile plus dedicated live and backfill semantic workers.

use std::sync::Arc;
use std::time::Duration;

use a2a_lab_dev_kit::{A2aLabError, DvrStoreNamespace, SemanticIndexingConfig};
use tokio::sync::watch;

use super::annotation_worker::{AnyIndexingAnnotationWorker, IndexingWorkerBatchReport};
use super::frame_manifest::DEFAULT_SEMANTIC_STORE_NAMESPACE;
use super::indexing_registration::DEFAULT_RECONCILE_BATCH;
use super::provider::VideoDvrCatalog;

/// Default delay between annotation reconcile batches while the catalog is live.
pub const ANNOTATION_RECONCILE_INTERVAL: Duration = Duration::from_secs(1);

/// Time reserved for reconcile and scheduling; workers may drain for the remainder.
const ANNOTATION_WORKER_SCHEDULE_SLOP: Duration = Duration::from_millis(25);

/// Keeps one reconcile/worker task alive until dropped.
pub struct VideoDvrAnnotationQueue {
    task: tokio::task::JoinHandle<()>,
}

impl Drop for VideoDvrAnnotationQueue {
    fn drop(&mut self) {
        self.task.abort();
    }
}

/// One dedicated semantic indexing worker lane (live or backfill).
#[derive(Clone)]
pub struct AnnotationQueueWorker {
    pub(crate) inner: Arc<AnyIndexingAnnotationWorker>,
    pub(crate) semantic: Arc<SemanticIndexingConfig>,
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

/// Live capture and historical backfill workers running on the same reconcile loop.
#[derive(Clone)]
pub struct AnnotationQueueWorkerBundle {
    pub live: AnnotationQueueWorker,
    pub backfill: AnnotationQueueWorker,
}

impl AnnotationQueueWorkerBundle {
    #[must_use]
    pub fn new(live: AnnotationQueueWorker, backfill: AnnotationQueueWorker) -> Self {
        Self { live, backfill }
    }
}

async fn run_worker_batch(
    worker: &AnnotationQueueWorker,
    catalog: &VideoDvrCatalog,
) -> Result<IndexingWorkerBatchReport, A2aLabError> {
    worker
        .inner
        .run_batch(catalog.dvr(), worker.semantic.as_ref())
        .await
}

fn log_worker_error(lane: &str, error: &A2aLabError) {
    eprintln!("video DVR annotation {lane} worker: {error}");
}

impl VideoDvrCatalog {
    /// Schedules bounded reconcile batches and dedicated lane workers until `stop` is set.
    #[must_use]
    pub fn maintain_annotation_jobs(
        &self,
        mut stop: watch::Receiver<bool>,
        workers: Option<AnnotationQueueWorkerBundle>,
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
                if let Err(error) = catalog
                    .dvr()
                    .reconcile_indexing_jobs(&store_ns, DEFAULT_RECONCILE_BATCH)
                    .await
                {
                    eprintln!("video DVR annotation reconcile: {error}");
                }
                if let Some(workers) = workers.as_ref() {
                    let drain_budget = ANNOTATION_RECONCILE_INTERVAL
                        .saturating_sub(ANNOTATION_WORKER_SCHEDULE_SLOP);
                    let drain_deadline = tokio::time::Instant::now() + drain_budget;
                    loop {
                        if tokio::time::Instant::now() >= drain_deadline {
                            break;
                        }
                        let (live_result, backfill_result) = tokio::join!(
                            run_worker_batch(&workers.live, &catalog),
                            run_worker_batch(&workers.backfill, &catalog),
                        );
                        let live_claimed = match &live_result {
                            Ok(report) => report.claimed(),
                            Err(error) => {
                                log_worker_error("live", error);
                                0
                            }
                        };
                        let backfill_claimed = match &backfill_result {
                            Ok(report) => report.claimed(),
                            Err(error) => {
                                log_worker_error("backfill", error);
                                0
                            }
                        };
                        if live_claimed == 0 && backfill_claimed == 0 {
                            break;
                        }
                    }
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
