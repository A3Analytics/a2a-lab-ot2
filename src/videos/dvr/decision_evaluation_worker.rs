//! Bounded Jev-only evaluation worker (no Luna/chat path).

use std::collections::HashMap;
use std::sync::Arc;

use a2a_lab_dev_kit::{
    A2aLabError, DecisionEvaluationCoverage, DecisionEvidenceItem, FakeFrameDecisionEvaluator,
    FakeJevBehavior, FrameDecisionEvaluationFailure, FrameDecisionEvaluationRequest,
    FrameDecisionEvaluationRouter, FrameDecisionEvaluator, FrameDecisionEvaluatorLimits,
    FrameDecisionEvaluatorProviderConfig, FrameDecisionRecord, FrameRoiTransform, JevEvaluationId,
    KeyedFrameDecisionEvaluation, SemanticDecisionLimits, SemanticIndexingConfig,
};

use super::decision_evaluation_enqueue::DecisionEvaluationTarget;
use super::decision_evaluation_job_store::{
    DecisionEvaluationJobRecord, DecisionEvaluationJobState,
};
use super::decision_evaluation_pilot::pilot_operator_criteria;
use super::decision_worker_faults::DecisionEvaluationWorkerFaults;
use super::frame_evidence::{FrameEvidenceLimits, FrameEvidenceRequest};
use super::store::IndexedVideoDvr;

/// Outcome of one bounded Jev evaluation batch.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct DecisionEvaluationWorkerBatchReport {
    reclaimed_leases: u32,
    claimed: u32,
    evaluations_committed: u32,
    coverage_only: u32,
    failures: u32,
}

impl DecisionEvaluationWorkerBatchReport {
    #[must_use]
    pub const fn reclaimed_leases(&self) -> u32 {
        self.reclaimed_leases
    }

    #[must_use]
    pub const fn claimed(&self) -> u32 {
        self.claimed
    }

    #[must_use]
    pub const fn evaluations_committed(&self) -> u32 {
        self.evaluations_committed
    }

    #[must_use]
    pub const fn coverage_only(&self) -> u32 {
        self.coverage_only
    }

    #[must_use]
    pub const fn failures(&self) -> u32 {
        self.failures
    }
}

/// Deterministic Jev worker wired for tests and local pilots.
pub type DeterministicDecisionEvaluationWorker =
    DecisionEvaluationWorker<FakeFrameDecisionEvaluator>;

/// Routes decision batches through either the dev-kit fake or live `TypeSafe` evaluator.
pub enum AnyDecisionEvaluationWorker {
    /// Deterministic `FakeFrameDecisionEvaluator` (default for CI and absent live mode).
    Deterministic(tokio::sync::Mutex<DeterministicDecisionEvaluationWorker>),
    /// Live `TypeSafe` HTTP evaluator when opted in and credentialed.
    Live(tokio::sync::Mutex<crate::videos::semantic_indexing::LiveDecisionEvaluationWorker>),
}

impl AnyDecisionEvaluationWorker {
    /// Runs one bounded evaluation batch with pilot targets only (no Luna path).
    pub async fn run_batch(
        &self,
        dvr: &IndexedVideoDvr,
        semantic: &SemanticIndexingConfig,
        targets: &[DecisionEvaluationTarget],
    ) -> Result<DecisionEvaluationWorkerBatchReport, A2aLabError> {
        match self {
            Self::Deterministic(worker) => {
                worker.lock().await.run_batch(dvr, semantic, targets).await
            }
            Self::Live(worker) => worker.lock().await.run_batch(dvr, semantic, targets).await,
        }
    }
}

/// Routes saved-frame evidence through the sole registered Jev evaluator.
pub struct DecisionEvaluationWorker<E: FrameDecisionEvaluator> {
    router: FrameDecisionEvaluationRouter<E>,
    config: FrameDecisionEvaluatorProviderConfig,
    worker_id: String,
    limits: FrameDecisionEvaluatorLimits,
    decision_limits: SemanticDecisionLimits,
    faults: Arc<DecisionEvaluationWorkerFaults>,
}

impl DecisionEvaluationWorker<FakeFrameDecisionEvaluator> {
    /// Wires a deterministic dev-kit Jev fake; no `TypeSafe` HTTP adapter.
    pub fn deterministic(
        config: &FrameDecisionEvaluatorProviderConfig,
        limits: FrameDecisionEvaluatorLimits,
        decision_limits: SemanticDecisionLimits,
        behavior: FakeJevBehavior,
        worker_id: impl Into<String>,
        faults: Arc<DecisionEvaluationWorkerFaults>,
    ) -> Self {
        let evaluator = FakeFrameDecisionEvaluator::new(config.clone(), limits, behavior);
        Self::new(
            FrameDecisionEvaluationRouter::new(evaluator),
            config.clone(),
            limits,
            decision_limits,
            worker_id,
            faults,
        )
    }
}

impl<E: FrameDecisionEvaluator> DecisionEvaluationWorker<E> {
    #[must_use]
    pub fn new(
        router: FrameDecisionEvaluationRouter<E>,
        config: FrameDecisionEvaluatorProviderConfig,
        limits: FrameDecisionEvaluatorLimits,
        decision_limits: SemanticDecisionLimits,
        worker_id: impl Into<String>,
        faults: Arc<DecisionEvaluationWorkerFaults>,
    ) -> Self {
        Self {
            router,
            config,
            worker_id: worker_id.into(),
            limits,
            decision_limits,
            faults,
        }
    }

    /// Runs one bounded pass without consulting description or embedding checkpoints.
    pub async fn run_batch(
        &mut self,
        dvr: &IndexedVideoDvr,
        _semantic: &SemanticIndexingConfig,
        targets: &[DecisionEvaluationTarget],
    ) -> Result<DecisionEvaluationWorkerBatchReport, A2aLabError> {
        let now = crate::time::now()?;
        let now_us = now.as_micros();
        let deadline_us = i64::try_from(
            self.decision_limits
                .job_deadline
                .as_secs()
                .saturating_mul(1_000_000),
        )
        .unwrap_or(i64::MAX);
        let lease_deadline_us = now_us.saturating_add(deadline_us);
        let reclaimed_leases = dvr
            .reclaim_expired_decision_evaluation_leases(now_us)
            .await?;
        let claimed = dvr
            .claim_decision_evaluation_jobs(
                &self.worker_id,
                &self.decision_limits,
                lease_deadline_us,
                now_us,
            )
            .await?;
        let mut report = DecisionEvaluationWorkerBatchReport {
            reclaimed_leases,
            claimed: u32::try_from(claimed.len()).unwrap_or(u32::MAX),
            ..DecisionEvaluationWorkerBatchReport::default()
        };
        let target_map = targets_by_evaluation_id(targets);
        for job in claimed {
            if let Err(error) = self.process_one(dvr, &job, &target_map, now_us).await {
                report.failures = report.failures.saturating_add(1);
                let _ = error;
            }
        }
        Ok(report)
    }

    #[allow(clippy::too_many_lines, clippy::cognitive_complexity)]
    async fn process_one(
        &mut self,
        dvr: &IndexedVideoDvr,
        job: &DecisionEvaluationJobRecord,
        targets: &HashMap<String, DecisionEvaluationTarget>,
        now_us: i64,
    ) -> Result<(), A2aLabError> {
        let Some(target) = targets.get(job.evaluation_id()) else {
            self.finish_job(
                dvr,
                job,
                DecisionEvaluationCoverage::NotEvaluated,
                Some("target_not_registered"),
                None,
                None,
                now_us,
            )
            .await?;
            return Ok(());
        };
        let manifest = dvr.get_manifest_frame(job.frame_id()).await?;
        let saved = manifest.frame().clone();
        if self.faults.force_evidence_failure() {
            self.finish_job(
                dvr,
                job,
                DecisionEvaluationCoverage::Failed,
                Some("poison_evidence"),
                None,
                None,
                now_us,
            )
            .await?;
            return Ok(());
        }
        let evidence_request = FrameEvidenceRequest::new(&saved, None);
        let evidence_limits = FrameEvidenceLimits::standard();
        let pixels = match dvr
            .read_bounded_frame_evidence(evidence_request, evidence_limits)
            .await
        {
            Ok(pixels) => pixels,
            Err(error) => {
                self.finish_job(
                    dvr,
                    job,
                    DecisionEvaluationCoverage::Failed,
                    Some(error.code()),
                    None,
                    None,
                    now_us,
                )
                .await?;
                return Ok(());
            }
        };
        let evidence_roi = subject_roi(target.subject()).unwrap_or(pixels.model_input_transform());
        let evidence = vec![
            DecisionEvidenceItem::saved_frame_revision(saved.frame_id().clone(), evidence_roi)
                .map_err(|error| A2aLabError::invalid("evidence", error.to_string()))?,
        ];
        let evaluation_id = match JevEvaluationId::new(job.evaluation_id()) {
            Ok(value) => value,
            Err(_error) => {
                self.finish_job(
                    dvr,
                    job,
                    DecisionEvaluationCoverage::Failed,
                    Some("invalid_evaluation_id"),
                    None,
                    None,
                    now_us,
                )
                .await?;
                return Ok(());
            }
        };
        let request = match FrameDecisionEvaluationRequest::new(
            self.limits,
            evaluation_id,
            saved.clone(),
            target.predicate().clone(),
            target.subject().clone(),
            evidence,
            pilot_operator_criteria(),
            pixels.model_input_transform(),
            pixels.rgb().len(),
            pixels.width().max(pixels.height()),
        ) {
            Ok(value) => value,
            Err(failure) => {
                self.finish_job(
                    dvr,
                    job,
                    DecisionEvaluationCoverage::Failed,
                    Some(failure.as_str()),
                    None,
                    None,
                    now_us,
                )
                .await?;
                return Ok(());
            }
        };
        let keyed = match self.router.evaluate_frames(vec![request.clone()]).await {
            Ok(mut batch) => batch.pop().ok_or_else(|| {
                A2aLabError::protocol("jev evaluator returned an empty batch".to_owned())
            })?,
            Err(failure) => {
                let gap_detail = failure.to_string();
                let (coverage, _gap) = map_router_failure(&failure);
                if coverage == DecisionEvaluationCoverage::Failed
                    || coverage == DecisionEvaluationCoverage::TimedOut
                {
                    let backoff = i64::try_from(
                        self.decision_limits
                            .job_deadline
                            .as_secs()
                            .saturating_mul(1_000_000),
                    )
                    .unwrap_or(1_000_000);
                    let _ = dvr
                        .fail_decision_evaluation_job_retryable_cas(
                            job.job_id(),
                            &self.worker_id,
                            job.revision_generation(),
                            coverage,
                            &gap_detail,
                            now_us.saturating_add(backoff),
                            now_us,
                            3,
                        )
                        .await?;
                } else {
                    self.finish_job(
                        dvr,
                        job,
                        coverage,
                        Some(gap_detail.as_str()),
                        None,
                        None,
                        now_us,
                    )
                    .await?;
                }
                return Ok(());
            }
        };
        let record = match materialize_record(&keyed, &request, &self.config) {
            Ok(value) => value,
            Err(error) => {
                self.finish_job(
                    dvr,
                    job,
                    DecisionEvaluationCoverage::Failed,
                    Some("record_materialization"),
                    None,
                    None,
                    now_us,
                )
                .await?;
                let _ = error;
                return Ok(());
            }
        };
        if let Some(record) = record {
            if self.faults.crash_before_decision_commit() {
                let _ = dvr
                    .append_frame_decision(record)
                    .await
                    .map_err(map_append)?;
                let _ = dvr
                    .fail_decision_evaluation_job_retryable_cas(
                        job.job_id(),
                        &self.worker_id,
                        job.revision_generation(),
                        DecisionEvaluationCoverage::Failed,
                        "crash_before_commit",
                        now_us,
                        now_us,
                        1,
                    )
                    .await;
                return Ok(());
            }
            let append = match dvr.append_frame_decision(record.clone()).await {
                Ok(value) => value,
                Err(error) => {
                    self.finish_job(
                        dvr,
                        job,
                        DecisionEvaluationCoverage::Failed,
                        Some("append_failed"),
                        None,
                        None,
                        now_us,
                    )
                    .await?;
                    let _ = map_append(error);
                    return Ok(());
                }
            };
            let decision_id = append.record().decision_id().as_str();
            self.finish_job(
                dvr,
                job,
                keyed.coverage(),
                None,
                Some(record.idempotency_key().as_str()),
                Some(decision_id),
                now_us,
            )
            .await?;
        } else {
            let gap = coverage_gap_label(keyed.coverage());
            self.finish_job(dvr, job, keyed.coverage(), Some(gap), None, None, now_us)
                .await?;
        }
        Ok(())
    }

    async fn finish_job(
        &self,
        dvr: &IndexedVideoDvr,
        job: &DecisionEvaluationJobRecord,
        coverage: DecisionEvaluationCoverage,
        gap_reason: Option<&str>,
        idempotency_key: Option<&str>,
        decision_id: Option<&str>,
        now_us: i64,
    ) -> Result<(), A2aLabError> {
        if job.job_state() == DecisionEvaluationJobState::Leased {
            let committed = dvr
                .complete_decision_evaluation_job_cas(
                    job.job_id(),
                    &self.worker_id,
                    job.revision_generation(),
                    coverage,
                    gap_reason,
                    idempotency_key,
                    decision_id,
                    now_us,
                )
                .await?;
            if !committed {
                return Err(A2aLabError::invalid(
                    "decision_job",
                    "lease lost before commit",
                ));
            }
        }
        Ok(())
    }
}

fn targets_by_evaluation_id(
    targets: &[DecisionEvaluationTarget],
) -> HashMap<String, DecisionEvaluationTarget> {
    targets
        .iter()
        .map(|target| (target.evaluation_id().as_str().to_owned(), target.clone()))
        .collect()
}

fn subject_roi(subject: &a2a_lab_dev_kit::DecisionSubject) -> Option<FrameRoiTransform> {
    match subject {
        a2a_lab_dev_kit::DecisionSubject::PipetteChannel(entry) => entry.tip_evidence_roi(),
        a2a_lab_dev_kit::DecisionSubject::ForeignObject(entry) => Some(entry.evidence_roi()),
        a2a_lab_dev_kit::DecisionSubject::PlateSlot(entry) => entry.inspection_roi(),
    }
}

fn map_append(error: a2a_lab_dev_kit::FrameDecisionAppendError) -> A2aLabError {
    a2a_lab_dev_kit::map_append_error(error)
}

fn map_router_failure(
    failure: &FrameDecisionEvaluationFailure,
) -> (DecisionEvaluationCoverage, &'static str) {
    match failure.as_str() {
        "deadline_exceeded" => (DecisionEvaluationCoverage::TimedOut, "evaluator_timeout"),
        "provider_unavailable" => (DecisionEvaluationCoverage::Failed, "jev_outage"),
        _ => (DecisionEvaluationCoverage::Failed, "evaluator_failure"),
    }
}

fn coverage_gap_label(coverage: DecisionEvaluationCoverage) -> &'static str {
    match coverage {
        DecisionEvaluationCoverage::Unsupported => "unsupported_predicate",
        DecisionEvaluationCoverage::NotEvaluated => "not_evaluated",
        DecisionEvaluationCoverage::Failed => "evaluation_failed",
        DecisionEvaluationCoverage::TimedOut => "evaluator_timeout",
        DecisionEvaluationCoverage::Cancelled => "operator_cancelled",
        DecisionEvaluationCoverage::Evaluated => "evaluated",
    }
}

fn materialize_record(
    keyed: &KeyedFrameDecisionEvaluation,
    request: &FrameDecisionEvaluationRequest,
    config: &FrameDecisionEvaluatorProviderConfig,
) -> Result<Option<FrameDecisionRecord>, A2aLabError> {
    if !keyed.coverage().permits_decision_outcome() {
        return Ok(None);
    }
    let captured = request.target().captured_at();
    let evaluated_at = request
        .target()
        .captured_at()
        .saturating_add_micros(1_000_000);
    keyed
        .clone()
        .into_record(
            request,
            config,
            captured,
            evaluated_at,
            request.model_input_transform(),
        )
        .map(Some)
        .map_err(|error| A2aLabError::protocol(error.to_string()))
}
