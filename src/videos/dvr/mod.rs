//! Live memory window plus indexed disk DVR.

mod annotation_queue;
mod annotation_worker;
pub mod chunk;
mod coverage;
mod decision_evaluation_enqueue;
mod decision_evaluation_job_store;
mod decision_evaluation_pilot;
mod decision_evaluation_queue;
mod decision_evaluation_worker;
mod decision_worker_faults;
mod dvr_frame_decision_ledger;
mod frame_decision_store;
pub mod frame_evidence;
pub mod frame_manifest;
mod indexing_job_store;
pub mod indexing_registration;
pub mod inventory;
mod provider;
mod store;
mod worker_faults;

pub use annotation_queue::{
    ANNOTATION_RECONCILE_INTERVAL, AnnotationQueueWorker, VideoDvrAnnotationQueue,
};
pub use annotation_worker::{
    AnyIndexingAnnotationWorker, DeterministicIndexingWorker, IndexingAnnotationWorker,
    DeterministicDescriptionOnlyWorker, IndexingWorkerBatchReport, LiveDescriptionOnlyWorker,
    LiveIndexingWorker,
};
pub mod semantic_search;
pub use semantic_search::DvrSemanticSearchProvider;
mod indexing_scope;
pub use decision_evaluation_enqueue::{
    DecisionEvaluationFrameSelection, DecisionEvaluationTarget, EnqueueDecisionEvaluationsReport,
    EnqueueDecisionEvaluationsRequest, decision_evaluation_job_id, subject_digest,
};
pub use decision_evaluation_job_store::{
    DecisionEvaluationCoverageReport, DecisionEvaluationJobRecord, DecisionEvaluationJobState,
};
pub use decision_evaluation_pilot::{
    pilot_evaluator_config, pilot_operator_criteria, pilot_predicate_targets,
};
pub use decision_evaluation_queue::{
    DECISION_EVALUATION_INTERVAL, DecisionEvaluationQueueWorker, VideoDvrDecisionEvaluationQueue,
};
pub use decision_evaluation_worker::{
    AnyDecisionEvaluationWorker, DecisionEvaluationWorker, DecisionEvaluationWorkerBatchReport,
    DeterministicDecisionEvaluationWorker,
};
pub use decision_worker_faults::DecisionEvaluationWorkerFaults;
pub use dvr_frame_decision_ledger::DvrFrameDecisionLedger;
pub use frame_evidence::{
    FrameEvidenceFailure, FrameEvidenceFailureKind, FrameEvidenceLimits, FrameEvidencePixels,
    FrameEvidenceRequest,
};
pub use frame_manifest::{
    DEFAULT_SEMANTIC_STORE_NAMESPACE, IndexingCheckpoint, ListManifestFramesRequest,
    ManifestPageRequest, ManifestRetainedState, MediaPayloadStatus, RegisterManifestOutcome,
    SemanticFrameManifestRecord, StoredFrameDescription, StoredFrameEmbedding,
};
pub use indexing_job_store::{IndexingJobRecord, IndexingJobState, IndexingWorkerQueueStats};
pub use indexing_registration::{
    DEFAULT_RECONCILE_BATCH, IndexingReconcileReport, SaveFaults, indexing_job_id,
};
pub use indexing_scope::IndexedSourceFilter;
pub use inventory::{DvrTrackInventory, FramePayloadEvidence};
pub use provider::{VideoDvrCatalog, VideoDvrMaintenance};
pub use store::{
    DEFAULT_DISK_VIDEO_RETENTION_SECS, DEFAULT_VIDEO_DVR_ROOT, IndexedVideoDvr,
    VIDEO_DVR_CLEANUP_INTERVAL,
};
pub use worker_faults::IndexingWorkerFaults;
