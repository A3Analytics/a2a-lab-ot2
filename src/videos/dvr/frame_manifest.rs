//! Re-exports generic semantic manifest types from the dev-kit.

pub use a2a_lab_dev_kit::videos::{
    IndexingCheckpoint, ListManifestFramesRequest, ManifestPageRequest, ManifestRetainedState,
    ManifestSearchCandidateFilter, ManifestSearchCandidateScan, MediaPayloadStatus,
    RegisterManifestOutcome, SemanticFrameManifestRecord, StoredFrameDescription,
    StoredFrameEmbedding,
};

pub use super::store::DEFAULT_SEMANTIC_STORE_NAMESPACE;
