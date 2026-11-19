//! Builds deterministic or live Jev decision workers for serve.

use std::sync::Arc;

use a2a_lab_dev_kit::{
    A2aLabError, DEFAULT_JEV_MODEL_REVISION, FakeJevBehavior, FrameDecisionEvaluationFailure,
    FrameDecisionEvaluationRouter, FrameDecisionEvaluatorLimits, SemanticIndexingConfig,
};

use crate::videos::dvr::{
    DecisionEvaluationWorker, DecisionEvaluationWorkerFaults,
    DeterministicDecisionEvaluationWorker, IndexedVideoDvr, pilot_evaluator_config,
};
use crate::videos::semantic_indexing::credentials::credential_env_configured;

use super::adapter::{TypeSafeJevFrameDecisionEvaluator, live_jev_allowed};
use super::media::EvidenceJpegDecisionMedia;

/// Live worker type wired through the devkit compatibility adapter.
pub type LiveDecisionEvaluationWorker =
    DecisionEvaluationWorker<TypeSafeJevFrameDecisionEvaluator<EvidenceJpegDecisionMedia>>;

/// Whether v2 config and env gates permit a live Jev worker.
#[must_use]
pub fn jev_live_worker_enabled(semantic: &SemanticIndexingConfig) -> bool {
    live_jev_allowed()
        && jev_decisions_config_valid(semantic)
        && semantic
            .decisions
            .credentials()
            .is_some_and(credential_env_configured)
}

fn jev_decisions_config_valid(semantic: &SemanticIndexingConfig) -> bool {
    semantic.decisions.provider() == "jev"
        && semantic.decisions.model() == DEFAULT_JEV_MODEL_REVISION
}

/// Deterministic fake used when live mode is off or credentials are absent.
#[must_use]
pub fn build_deterministic_decision_worker(
    semantic: &SemanticIndexingConfig,
) -> DeterministicDecisionEvaluationWorker {
    let config = pilot_evaluator_config();
    let limits = FrameDecisionEvaluatorLimits::pilot();
    DecisionEvaluationWorker::deterministic(
        &config,
        limits,
        semantic.limits.decisions.clone(),
        FakeJevBehavior::OkPresent,
        format!("ot2-decision-{}", std::process::id()),
        DecisionEvaluationWorkerFaults::new(),
    )
}

/// Live worker; requires `A2ALAB_JEV_LIVE=1` and configured credentials.
pub fn build_live_decision_worker(
    semantic: &SemanticIndexingConfig,
    dvr: Arc<IndexedVideoDvr>,
) -> Result<LiveDecisionEvaluationWorker, FrameDecisionEvaluationFailure> {
    if !jev_live_worker_enabled(semantic) {
        return Err(FrameDecisionEvaluationFailure::ProviderUnavailable {
            message: "live Jev worker requires A2ALAB_JEV_LIVE=1 and decisions.credentials"
                .to_owned(),
        });
    }
    let config = pilot_evaluator_config();
    let limits = FrameDecisionEvaluatorLimits::pilot();
    let media = Arc::new(EvidenceJpegDecisionMedia::new(dvr));
    let evaluator = TypeSafeJevFrameDecisionEvaluator::connect_live(
        config.clone(),
        limits,
        media,
        semantic.decisions.credentials(),
    )?;
    Ok(DecisionEvaluationWorker::new(
        FrameDecisionEvaluationRouter::new(evaluator),
        config,
        limits,
        semantic.limits.decisions.clone(),
        format!("ot2-decision-live-{}", std::process::id()),
        DecisionEvaluationWorkerFaults::new(),
    ))
}

/// Maps evaluator setup failures into catalog startup errors.
#[must_use]
pub fn map_decision_worker_failure(failure: FrameDecisionEvaluationFailure) -> A2aLabError {
    failure.into_lab_error()
}
