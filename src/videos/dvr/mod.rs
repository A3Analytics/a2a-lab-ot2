//! Live memory window plus indexed disk DVR.

mod annotation_queue;
pub mod chunk;
mod decision_evaluation_enqueue;
mod decision_evaluation_job_store;
mod decision_evaluation_pilot;
mod decision_evaluation_queue;
mod decision_evaluation_worker;
mod decision_worker_faults;
mod dvr_frame_decision_ledger;
mod epoch_playback;
mod frame_decision_store;
pub mod frame_evidence;
pub mod frame_manifest;
mod h264_epoch;
mod indexing_scope;
pub mod inventory;
mod provider;
mod store;

pub use a2a_lab_dev_kit::videos::{
    DEFAULT_RECONCILE_BATCH, INDEXING_PRIORITY_BACKFILL, INDEXING_PRIORITY_LIVE,
    IndexingCheckpoint, IndexingJobRecord, IndexingJobState, IndexingReconcileReport,
    IndexingWorkerBatchReport, IndexingWorkerFaults, IndexingWorkerLane, IndexingWorkerQueueStats,
    ListManifestFramesRequest, ManifestPageRequest, ManifestRetainedState,
    ManifestSearchCandidateFilter, ManifestSearchCandidateScan, MediaPayloadStatus,
    RegisterManifestOutcome, SaveFaults, SemanticFrameManifestRecord, StoredFrameDescription,
    StoredFrameEmbedding, indexing_job_id,
};
pub use annotation_queue::{
    ANNOTATION_RECONCILE_INTERVAL, AnnotationQueueWorker, AnnotationQueueWorkerBundle,
    SemanticAnnotationWorkers, VideoDvrAnnotationQueue,
};
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
pub use frame_manifest::DEFAULT_SEMANTIC_STORE_NAMESPACE;
pub use indexing_scope::IndexedSourceFilter;
pub use inventory::{DvrTrackInventory, FramePayloadEvidence};
pub use provider::{VideoDvrCatalog, VideoDvrMaintenance};
pub use store::{
    DEFAULT_DISK_VIDEO_RETENTION_SECS, DEFAULT_VIDEO_DVR_ROOT, IndexedVideoDvr,
    VIDEO_DVR_CLEANUP_INTERVAL,
};
