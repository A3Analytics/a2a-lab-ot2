//! Bounded enqueue requests for durable Jev evaluation jobs.

use a2a_lab_dev_kit::{
    A2aLabError, DecisionCoveragePolicy, DecisionSubject, DvrTrackId,
    FrameDecisionEvaluatorProviderConfig, JevEvaluationId, LabPredicate, TimeRange, VideoSourceId,
};

use super::decision_evaluation_pilot::pilot_predicate_targets;
use super::frame_manifest::ListManifestFramesRequest;
use super::frame_manifest::ManifestPageRequest;
use super::store::IndexedVideoDvr;

/// One predicate/subject pair to evaluate for each resolved frame.
#[derive(Debug, Clone, PartialEq)]
pub struct DecisionEvaluationTarget {
    evaluation_suffix: String,
    predicate: LabPredicate,
    subject: DecisionSubject,
}

impl DecisionEvaluationTarget {
    /// Builds one evaluation target with a unique suffix token.
    pub fn new(
        evaluation_suffix: impl Into<String>,
        predicate: LabPredicate,
        subject: DecisionSubject,
    ) -> Result<Self, A2aLabError> {
        let evaluation_suffix = evaluation_suffix.into();
        if evaluation_suffix.trim().is_empty() {
            return Err(A2aLabError::invalid(
                "evaluation_suffix",
                "must not be empty",
            ));
        }
        if !subject.matches_predicate(&predicate) {
            return Err(A2aLabError::invalid(
                "subject",
                "does not match predicate schema",
            ));
        }
        Ok(Self {
            evaluation_suffix,
            predicate,
            subject,
        })
    }

    #[must_use]
    pub fn evaluation_id(&self) -> JevEvaluationId {
        JevEvaluationId::new(format!("jev-req-{}", self.evaluation_suffix))
            .expect("evaluation suffix produces valid id")
    }

    #[must_use]
    pub const fn predicate(&self) -> &LabPredicate {
        &self.predicate
    }

    #[must_use]
    pub const fn subject(&self) -> &DecisionSubject {
        &self.subject
    }
}

/// Explicit frame ids or bounded source/time span.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DecisionEvaluationFrameSelection {
    FrameIds(Vec<String>),
    SourceTimeRange {
        source_id: VideoSourceId,
        track_id: DvrTrackId,
        window: TimeRange,
    },
}

/// Bounded enqueue boundary for on-demand and policy-scoped work.
#[derive(Debug, Clone, PartialEq)]
pub struct EnqueueDecisionEvaluationsRequest {
    policy: DecisionCoveragePolicy,
    config: FrameDecisionEvaluatorProviderConfig,
    selection: DecisionEvaluationFrameSelection,
    targets: Vec<DecisionEvaluationTarget>,
    sampled_stride: u32,
}

impl EnqueueDecisionEvaluationsRequest {
    /// Creates an enqueue request after target validation.
    pub fn new(
        policy: DecisionCoveragePolicy,
        config: FrameDecisionEvaluatorProviderConfig,
        selection: DecisionEvaluationFrameSelection,
        targets: Vec<DecisionEvaluationTarget>,
        sampled_stride: u32,
    ) -> Result<Self, A2aLabError> {
        if targets.is_empty() {
            return Err(A2aLabError::invalid(
                "targets",
                "at least one predicate target is required",
            ));
        }
        Ok(Self {
            policy,
            config,
            selection,
            targets,
            sampled_stride: sampled_stride.max(1),
        })
    }

    /// On-demand enqueue for explicit frame ids and the three pilot predicates.
    pub fn on_demand_pilot_frames(
        config: FrameDecisionEvaluatorProviderConfig,
        frame_ids: Vec<String>,
    ) -> Result<Self, A2aLabError> {
        let targets = pilot_predicate_targets()
            .into_iter()
            .enumerate()
            .map(|(index, (predicate, subject))| {
                DecisionEvaluationTarget::new(format!("pilot-{index}"), predicate, subject)
            })
            .collect::<Result<Vec<_>, _>>()?;
        Self::new(
            DecisionCoveragePolicy::OnDemand,
            config,
            DecisionEvaluationFrameSelection::FrameIds(frame_ids),
            targets,
            1,
        )
    }

    #[must_use]
    pub const fn policy(&self) -> DecisionCoveragePolicy {
        self.policy
    }

    #[must_use]
    pub const fn config(&self) -> &FrameDecisionEvaluatorProviderConfig {
        &self.config
    }

    #[must_use]
    pub fn targets(&self) -> &[DecisionEvaluationTarget] {
        &self.targets
    }
}

/// Counts from one idempotent enqueue pass.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct EnqueueDecisionEvaluationsReport {
    frames_considered: u32,
    jobs_created: u32,
    idempotent: u32,
    policy_skipped: u32,
}

impl EnqueueDecisionEvaluationsReport {
    #[must_use]
    pub const fn frames_considered(&self) -> u32 {
        self.frames_considered
    }

    #[must_use]
    pub const fn jobs_created(&self) -> u32 {
        self.jobs_created
    }

    #[must_use]
    pub const fn idempotent(&self) -> u32 {
        self.idempotent
    }

    #[must_use]
    pub const fn policy_skipped(&self) -> u32 {
        self.policy_skipped
    }
}

/// Stable job id derived from evaluator/predicate/config identity.
#[must_use]
pub fn decision_evaluation_job_id(
    evaluation_id: &str,
    predicate: &LabPredicate,
    subject_digest: &str,
    config_digest: &str,
    producer_revision: &str,
) -> String {
    format!(
        "decision-eval:{evaluation_id}:{}:v{}:{subject_digest}:{config_digest}:{producer_revision}",
        predicate.id(),
        predicate.version()
    )
}

/// Stable digest for idempotent job identity.
#[must_use]
pub fn subject_digest(subject: &DecisionSubject) -> String {
    use a2a_lab_dev_kit::Sha256Digest;
    let token = match subject {
        DecisionSubject::PipetteChannel(entry) => {
            format!("pipette:{}:{}", entry.pipetter_id(), entry.channel())
        }
        DecisionSubject::ForeignObject(entry) => format!("foreign:{}", entry.deck_region()),
        DecisionSubject::PlateSlot(entry) => {
            format!("plate:{}:{}", entry.plate_id(), entry.deck_slot())
        }
    };
    Sha256Digest::hash_bytes(token.as_bytes())
        .as_str()
        .to_owned()
}

impl IndexedVideoDvr {
    /// Resolves frame ids and enqueues idempotent decision evaluation jobs.
    pub async fn enqueue_decision_evaluation_jobs(
        &self,
        request: &EnqueueDecisionEvaluationsRequest,
    ) -> Result<EnqueueDecisionEvaluationsReport, A2aLabError> {
        let frame_ids = self.resolve_enqueue_frames(request).await?;
        let now = crate::time::now()?.as_micros();
        let config_digest = request.config().config_digest().as_str();
        let producer_revision = request.config().producer().model_revision();
        let policy = policy_label(request.policy());
        let mut report = EnqueueDecisionEvaluationsReport::default();
        for frame_id in frame_ids {
            report.frames_considered = report.frames_considered.saturating_add(1);
            if !frame_selected_for_policy(
                frame_id.as_str(),
                request.policy(),
                request.sampled_stride,
            ) {
                report.policy_skipped = report.policy_skipped.saturating_add(1);
                continue;
            }
            for target in request.targets() {
                let evaluation_id = target.evaluation_id().as_str().to_owned();
                let subject_digest = subject_digest(target.subject());
                let job_id = decision_evaluation_job_id(
                    evaluation_id.as_str(),
                    target.predicate(),
                    &subject_digest,
                    config_digest,
                    producer_revision,
                );
                let inserted = self
                    .insert_decision_evaluation_job(
                        &job_id,
                        &frame_id,
                        evaluation_id.as_str(),
                        target.predicate(),
                        &subject_digest,
                        config_digest,
                        producer_revision,
                        policy,
                        now,
                    )
                    .await?;
                if inserted {
                    report.jobs_created = report.jobs_created.saturating_add(1);
                } else {
                    report.idempotent = report.idempotent.saturating_add(1);
                }
            }
        }
        Ok(report)
    }

    async fn resolve_enqueue_frames(
        &self,
        request: &EnqueueDecisionEvaluationsRequest,
    ) -> Result<Vec<String>, A2aLabError> {
        match &request.selection {
            DecisionEvaluationFrameSelection::FrameIds(ids) => {
                if ids.is_empty() {
                    return Err(A2aLabError::invalid("frame_ids", "must not be empty"));
                }
                if ids.len() > 256 {
                    return Err(A2aLabError::invalid(
                        "frame_ids",
                        "exceeds bounded enqueue limit of 256",
                    ));
                }
                Ok(ids.clone())
            }
            DecisionEvaluationFrameSelection::SourceTimeRange {
                source_id,
                track_id,
                window,
            } => {
                let list = ListManifestFramesRequest::new(
                    source_id.clone(),
                    track_id.clone(),
                    Some(*window),
                    true,
                    ManifestPageRequest::new(None, 256)?,
                )?;
                let page = self.list_manifest_frames(&list).await?;
                Ok(page
                    .items()
                    .iter()
                    .map(|record| record.frame().frame_id().as_str().to_owned())
                    .collect())
            }
        }
    }
}

fn policy_label(policy: DecisionCoveragePolicy) -> &'static str {
    match policy {
        DecisionCoveragePolicy::AllSavedFrames => "all_saved_frames",
        DecisionCoveragePolicy::OnDemand => "on_demand",
        DecisionCoveragePolicy::Sampled => "sampled",
    }
}

fn frame_selected_for_policy(
    frame_id: &str,
    policy: DecisionCoveragePolicy,
    sampled_stride: u32,
) -> bool {
    match policy {
        DecisionCoveragePolicy::AllSavedFrames | DecisionCoveragePolicy::OnDemand => true,
        DecisionCoveragePolicy::Sampled => {
            let hash = frame_id.bytes().fold(0u32, |acc, byte| {
                acc.wrapping_mul(16_777_619).wrapping_add(u32::from(byte))
            });
            hash % sampled_stride == 0
        }
    }
}
