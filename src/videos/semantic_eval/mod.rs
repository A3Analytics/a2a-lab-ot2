//! Bounded semantic evaluation corpus: manifests, discovery, and validation.

mod discover;
mod manifest;
mod validate;

pub use discover::{aggregate_frame_digests, discover_retained_sources, refresh_retained_sources};
pub use manifest::{
    DvrAccessRoute, EvaluationManifest, EvaluationSplits, EvidenceAvailability, FrameAnnotation,
    LabeledSample, MANIFEST_SCHEMA_VERSION, NormalizedRoi, PredicateLabel, PredicateOutcome,
    RetainedSourceRecord, RetrievalLabel, RetrievalRelevance, TimeBlock,
};
pub use validate::{ValidationReport, load_manifest, validate_manifest};
