//! Bounded background loop for durable Jev evaluation jobs.

use std::sync::Arc;
use std::time::Duration;

use a2a_lab_dev_kit::SemanticIndexingConfig;
use tokio::sync::watch;

use super::decision_evaluation_enqueue::DecisionEvaluationTarget;
use super::decision_evaluation_pilot::pilot_predicate_targets;
use super::decision_evaluation_worker::AnyDecisionEvaluationWorker;
use super::provider::VideoDvrCatalog;

/// Default delay between decision evaluation batches while the catalog is live.
pub const DECISION_EVALUATION_INTERVAL: Duration = Duration::from_secs(2);

/// Keeps the decision evaluation loop alive until dropped.
pub struct VideoDvrDecisionEvaluationQueue {
    task: tokio::task::JoinHandle<()>,
}

impl Drop for VideoDvrDecisionEvaluationQueue {
    fn drop(&mut self) {
        self.task.abort();
    }
}

/// Worker configuration for one catalog lifetime.
#[derive(Clone)]
pub struct DecisionEvaluationQueueWorker {
    inner: Arc<AnyDecisionEvaluationWorker>,
    semantic: Arc<SemanticIndexingConfig>,
    targets: Arc<Vec<DecisionEvaluationTarget>>,
}

impl DecisionEvaluationQueueWorker {
    #[must_use]
    pub fn new(inner: AnyDecisionEvaluationWorker, semantic: SemanticIndexingConfig) -> Self {
        let targets = pilot_decision_targets();
        Self {
            inner: Arc::new(inner),
            semantic: Arc::new(semantic),
            targets: Arc::new(targets),
        }
    }
}

impl VideoDvrCatalog {
    /// Schedules bounded Jev evaluation batches until `stop` is set.
    #[must_use]
    pub fn maintain_decision_evaluation_jobs(
        &self,
        mut stop: watch::Receiver<bool>,
        worker: Option<DecisionEvaluationQueueWorker>,
    ) -> VideoDvrDecisionEvaluationQueue {
        let catalog = self.clone();
        let task = tokio::spawn(async move {
            let mut interval = tokio::time::interval(DECISION_EVALUATION_INTERVAL);
            interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
            loop {
                if *stop.borrow() {
                    break;
                }
                if let Some(worker) = worker.as_ref()
                    && let Err(error) = worker
                        .inner
                        .run_batch(
                            catalog.dvr(),
                            worker.semantic.as_ref(),
                            worker.targets.as_ref(),
                        )
                        .await
                {
                    eprintln!("video DVR decision evaluation worker: {error}");
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
        VideoDvrDecisionEvaluationQueue { task }
    }
}

fn pilot_decision_targets() -> Vec<DecisionEvaluationTarget> {
    pilot_predicate_targets()
        .into_iter()
        .enumerate()
        .map(|(index, (predicate, subject))| {
            DecisionEvaluationTarget::new(format!("pilot-{index}"), predicate, subject)
                .expect("pilot target")
        })
        .collect()
}
