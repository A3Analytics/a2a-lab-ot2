//! Validate evaluation manifests against read-only DVR evidence.

use std::path::Path;

use a2a_lab_dev_kit::{A2aLabError, DvrTrackId, VideoSourceId};

use super::manifest::{
    EvaluationManifest, EvaluationSplits, EvidenceAvailability, FrameAnnotation,
    MANIFEST_SCHEMA_VERSION,
};
use crate::videos::dvr::IndexedVideoDvr;

/// Outcome of a manifest validation pass (stdout-friendly).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ValidationReport {
    pub manifest_version: String,
    pub sources_checked: usize,
    pub sources_available: usize,
    pub sources_skipped: usize,
    pub total_retained_frames: u64,
    pub labeled_frames_verified: usize,
    pub untested_predicates: Vec<String>,
    pub warnings: Vec<String>,
}

impl ValidationReport {
    #[must_use]
    pub fn is_success(&self) -> bool {
        self.warnings.iter().all(|line| !line.starts_with("error:"))
    }
}

/// Validates `manifest` against the DVR at `dvr_root` without creating or mutating recordings.
pub async fn validate_manifest(
    manifest: &EvaluationManifest,
    dvr_root: &Path,
) -> Result<ValidationReport, A2aLabError> {
    if manifest.schema_version != MANIFEST_SCHEMA_VERSION {
        return Err(A2aLabError::invalid(
            "schema_version",
            format!(
                "expected {}, got {}",
                MANIFEST_SCHEMA_VERSION, manifest.schema_version
            ),
        ));
    }
    if !manifest.dvr_access.read_only {
        return Err(A2aLabError::invalid(
            "dvr_access.read_only",
            "evaluation discovery must be read-only",
        ));
    }
    validate_splits(&manifest.splits)?;

    let store = IndexedVideoDvr::open(dvr_root, 86_400).await?;
    let mut warnings = Vec::new();
    let mut sources_checked = 0_usize;
    let mut sources_available = 0_usize;
    let mut sources_skipped = 0_usize;
    let mut total_frames = 0_u64;

    for record in &manifest.retained_sources {
        sources_checked += 1;
        match record.evidence_availability {
            EvidenceAvailability::Expired | EvidenceAvailability::Unavailable => {
                sources_skipped += 1;
                warnings.push(format!(
                    "skipped {}:{} ({:?})",
                    record.source_id, record.track_id, record.evidence_availability
                ));
                continue;
            }
            EvidenceAvailability::Available => {}
        }
        let source_id = VideoSourceId::new(&record.source_id)?;
        let inventories = store.track_inventories(&source_id).await;
        let Ok(inventories) = inventories else {
            sources_skipped += 1;
            warnings.push(format!(
                "error: retained source missing from DVR index: {}",
                record.source_id
            ));
            continue;
        };
        let Some(inventory) = inventories
            .iter()
            .find(|row| row.track_id.as_str() == record.track_id)
        else {
            sources_skipped += 1;
            warnings.push(format!(
                "error: track {}:{} not present in DVR",
                record.source_id, record.track_id
            ));
            continue;
        };
        sources_available += 1;
        total_frames = total_frames.saturating_add(inventory.frame_count);
        if inventory.frame_count != record.frame_count {
            warnings.push(format!(
                "warning: frame_count mismatch for {}:{} manifest={} dvr={}",
                record.source_id, record.track_id, record.frame_count, inventory.frame_count
            ));
        }
    }

    let mut labeled_frames_verified = 0_usize;
    for annotation in &manifest.labeled_sample.frame_annotations {
        match verify_annotation(&store, annotation).await {
            Ok(()) => labeled_frames_verified += 1,
            Err(error) => warnings.push(format!(
                "error: annotation {}: {error}",
                annotation.annotation_id
            )),
        }
    }

    Ok(ValidationReport {
        manifest_version: manifest.manifest_version.clone(),
        sources_checked,
        sources_available,
        sources_skipped,
        total_retained_frames: total_frames,
        labeled_frames_verified,
        untested_predicates: manifest.labeled_sample.untested_predicates.clone(),
        warnings,
    })
}

async fn verify_annotation(
    store: &IndexedVideoDvr,
    annotation: &FrameAnnotation,
) -> Result<(), A2aLabError> {
    let source_id = VideoSourceId::new(&annotation.source_id)?;
    let track_id = DvrTrackId::new(&annotation.track_id)?;
    let evidence = store
        .frame_payload_evidence(&source_id, &track_id, annotation.start_us)
        .await?;
    if evidence.payload_sha256 != annotation.payload_sha256 {
        return Err(A2aLabError::invalid(
            "payload_sha256",
            "manifest digest does not match retained payload",
        ));
    }
    Ok(())
}

fn validate_splits(splits: &EvaluationSplits) -> Result<(), A2aLabError> {
    if splits.adjacent_frame_exclusion_us <= 0 {
        return Err(A2aLabError::invalid(
            "splits.adjacent_frame_exclusion_us",
            "must be positive",
        ));
    }
    for tuning in &splits.tuning_time_blocks {
        for evaluation in &splits.evaluation_time_blocks {
            if tuning.source_id == evaluation.source_id
                && tuning.track_id == evaluation.track_id
                && blocks_overlap(
                    tuning.start_us,
                    tuning.end_us,
                    evaluation.start_us,
                    evaluation.end_us,
                )
            {
                return Err(A2aLabError::invalid(
                    "splits",
                    "tuning and evaluation time blocks overlap on the same track",
                ));
            }
            if tuning.source_id == evaluation.source_id
                && tuning.track_id == evaluation.track_id
                && adjacent_leakage(
                    tuning.start_us,
                    tuning.end_us,
                    evaluation.start_us,
                    evaluation.end_us,
                    splits.adjacent_frame_exclusion_us,
                )
            {
                return Err(A2aLabError::invalid(
                    "splits",
                    "tuning/evaluation blocks violate adjacent_frame_exclusion_us",
                ));
            }
        }
    }
    Ok(())
}

fn blocks_overlap(start_a: i64, end_a: i64, start_b: i64, end_b: i64) -> bool {
    start_a < end_b && start_b < end_a
}

fn adjacent_leakage(
    tune_start: i64,
    tune_end: i64,
    eval_start: i64,
    eval_end: i64,
    gap: i64,
) -> bool {
    let tune_before_eval = tune_end <= eval_start;
    let eval_before_tune = eval_end <= tune_start;
    if tune_before_eval {
        return eval_start - tune_end < gap;
    }
    if eval_before_tune {
        return tune_start - eval_end < gap;
    }
    true
}

pub fn load_manifest(path: &Path) -> Result<EvaluationManifest, A2aLabError> {
    let bytes = std::fs::read(path)
        .map_err(|error| A2aLabError::unavailable(format!("read {}: {error}", path.display())))?;
    EvaluationManifest::parse_json(&bytes)
        .map_err(|error| A2aLabError::invalid("manifest", error.to_string()))
}
