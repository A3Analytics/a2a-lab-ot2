//! Versioned semantic evaluation manifest schema (JSON).

use serde::{Deserialize, Serialize};

pub const MANIFEST_SCHEMA_VERSION: u32 = 1;

/// Top-level evaluation manifest for Chupacabra retained DVR video.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EvaluationManifest {
    pub schema_version: u32,
    pub manifest_version: String,
    #[serde(default)]
    pub inventory_disclaimer: String,
    pub dvr_access: DvrAccessRoute,
    #[serde(default)]
    pub retained_sources: Vec<RetainedSourceRecord>,
    pub labeled_sample: LabeledSample,
    pub splits: EvaluationSplits,
}

/// Read-only route to on-host retained DVR bytes; not historical camera capability tables.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DvrAccessRoute {
    pub host: String,
    pub read_only: bool,
    pub dvr_root_env: String,
    pub dvr_root_default: String,
    #[serde(default)]
    pub service_unit: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum EvidenceAvailability {
    #[default]
    Available,
    Expired,
    Unavailable,
}

/// One retained source/track row recorded from DVR discovery or explicit pilot inventory.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RetainedSourceRecord {
    pub source_id: String,
    pub track_id: String,
    pub codec: String,
    pub capture_start_utc: String,
    pub capture_end_utc: String,
    pub frame_count: u64,
    #[serde(default)]
    pub evidence_availability: EvidenceAvailability,
    #[serde(default)]
    pub aggregate_payload_sha256: Option<String>,
}

/// Human-labeled frames for retrieval tuning and Jev evaluation coverage reporting.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LabeledSample {
    #[serde(default)]
    pub frame_annotations: Vec<FrameAnnotation>,
    #[serde(default)]
    pub untested_predicates: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FrameAnnotation {
    pub annotation_id: String,
    pub source_id: String,
    pub track_id: String,
    pub capture_utc: String,
    pub start_us: i64,
    pub payload_sha256: String,
    #[serde(default)]
    pub roi: Option<NormalizedRoi>,
    #[serde(default)]
    pub retrieval: Option<RetrievalLabel>,
    #[serde(default)]
    pub predicates: Vec<PredicateLabel>,
    #[serde(default)]
    pub occlusion: Option<String>,
}

/// Normalized ROI on the oriented original color frame: `[x_min, y_min, x_max, y_max]`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct NormalizedRoi {
    pub x_min: f32,
    pub y_min: f32,
    pub x_max: f32,
    pub y_max: f32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RetrievalLabel {
    pub query_text: String,
    pub relevance: RetrievalRelevance,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RetrievalRelevance {
    High,
    Medium,
    Low,
    Negative,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PredicateLabel {
    pub predicate: String,
    pub outcome: PredicateOutcome,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PredicateOutcome {
    Present,
    Absent,
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EvaluationSplits {
    pub tuning_time_blocks: Vec<TimeBlock>,
    pub evaluation_time_blocks: Vec<TimeBlock>,
    /// Minimum microseconds between any tuning frame and any evaluation frame on the same track.
    pub adjacent_frame_exclusion_us: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TimeBlock {
    pub source_id: String,
    pub track_id: String,
    pub start_utc: String,
    pub end_utc: String,
    #[serde(default)]
    pub start_us: i64,
    #[serde(default)]
    pub end_us: i64,
}

impl EvaluationManifest {
    pub fn parse_json(bytes: &[u8]) -> Result<Self, serde_json::Error> {
        serde_json::from_slice(bytes)
    }
}
