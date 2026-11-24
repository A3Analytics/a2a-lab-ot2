//! Opt-in Jev smoke/benchmark reporting (fixtures vs live adoption).

use std::path::Path;
use std::sync::Arc;

use a2a_lab_dev_kit::{
    FrameDecisionEvaluationRequest, FrameDecisionEvaluationRouter, FrameDecisionEvaluatorLimits,
    LoadedLabConfig, SemanticIndexingConfig,
};
use serde::Serialize;

use super::adapter::{jev_smoke_enabled, live_jev_allowed};
use super::media::EvidenceJpegDecisionMedia;
use super::{TypeSafeJevFrameDecisionEvaluator, jev_api_base};
use crate::videos::dvr::pilot_operator_criteria;
use crate::videos::dvr::{
    DEFAULT_DISK_VIDEO_RETENTION_SECS, IndexedVideoDvr, pilot_evaluator_config,
};

/// Real-image adoption gates reported separately from fixture success.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct JevSmokeAdoptionGates {
    pub live_opt_in: bool,
    pub credentials_configured: bool,
    pub api_endpoint: String,
    pub fixture_only_pass: bool,
}

/// Usage/latency snapshot for authorized smoke runs.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct JevSmokeReport {
    pub mode: &'static str,
    pub model: String,
    pub frame_id: String,
    pub evaluation_id: String,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub latency_ms: u64,
    pub idempotent_decision_key: String,
    pub adoption: JevSmokeAdoptionGates,
}

/// Runs one pinned Jev evaluation when `A2ALAB_JEV_SMOKE=1` and credentials are configured.
pub async fn run_jev_smoke(
    config_path: &Path,
    dvr_root: &Path,
    frame_id: &str,
    evaluation_id: &str,
) -> Result<JevSmokeReport, String> {
    if !jev_smoke_enabled() {
        return Err("Jev smoke disabled; set A2ALAB_JEV_SMOKE=1".into());
    }
    let loaded = LoadedLabConfig::load(config_path).map_err(|error| error.to_string())?;
    let semantic = loaded
        .config
        .semantic_indexing
        .as_ref()
        .ok_or_else(|| "configuration has no semantic_indexing section".to_owned())?;
    ensure_decisions_config(semantic)?;
    let credentials = semantic.decisions.credentials();
    let credentials_configured = credentials
        .is_some_and(crate::videos::semantic_indexing::credentials::credential_env_configured);
    let adoption = JevSmokeAdoptionGates {
        live_opt_in: live_jev_allowed(),
        credentials_configured,
        api_endpoint: jev_api_base(),
        fixture_only_pass: !live_jev_allowed() || !credentials_configured,
    };
    if adoption.fixture_only_pass {
        return Ok(fixture_report(frame_id, evaluation_id, adoption));
    }
    let dvr = Arc::new(
        IndexedVideoDvr::open(dvr_root, DEFAULT_DISK_VIDEO_RETENTION_SECS)
            .await
            .map_err(|error| error.to_string())?,
    );
    let manifest = dvr
        .get_manifest_frame(frame_id)
        .await
        .map_err(|error| error.to_string())?;
    let config = pilot_evaluator_config();
    let limits = FrameDecisionEvaluatorLimits::pilot();
    let media = Arc::new(EvidenceJpegDecisionMedia::new(dvr));
    let evaluator =
        TypeSafeJevFrameDecisionEvaluator::connect_live(config.clone(), limits, media, credentials)
            .map_err(|error| error.to_string())?
            .with_preamble(semantic.decisions.preamble().map(str::to_owned));
    let request = build_smoke_request(&manifest.frame().clone(), evaluation_id)?;
    let mut router = FrameDecisionEvaluationRouter::new(evaluator);
    let responses = router
        .evaluate_frames(vec![request.clone()])
        .await
        .map_err(|error| error.to_string())?;
    let keyed = responses
        .into_iter()
        .next()
        .ok_or_else(|| "empty jev response".to_owned())?;
    let structured = keyed
        .structured_output()
        .ok_or_else(|| "missing structured output".to_owned())?;
    let usage = structured
        .structured_value()
        .and_then(|value| value.as_map().get("usage"))
        .cloned()
        .unwrap_or_else(|| serde_json::json!({"input_tokens": 0, "output_tokens": 0}));
    let latency_ms = structured
        .structured_value()
        .and_then(|value| value.as_map().get("latency_ms"))
        .and_then(serde_json::Value::as_u64)
        .unwrap_or(0);
    Ok(JevSmokeReport {
        mode: "live",
        model: semantic.decisions.model().to_owned(),
        frame_id: frame_id.to_owned(),
        evaluation_id: evaluation_id.to_owned(),
        input_tokens: usage
            .get("input_tokens")
            .and_then(serde_json::Value::as_u64)
            .unwrap_or(0),
        output_tokens: usage
            .get("output_tokens")
            .and_then(serde_json::Value::as_u64)
            .unwrap_or(0),
        latency_ms,
        idempotent_decision_key: request.idempotency_key().as_str().to_owned(),
        adoption: JevSmokeAdoptionGates {
            fixture_only_pass: false,
            ..adoption
        },
    })
}

fn fixture_report(
    frame_id: &str,
    evaluation_id: &str,
    adoption: JevSmokeAdoptionGates,
) -> JevSmokeReport {
    JevSmokeReport {
        mode: "fixture",
        model: a2a_lab_dev_kit::DEFAULT_JEV_MODEL_REVISION.to_owned(),
        frame_id: frame_id.to_owned(),
        evaluation_id: evaluation_id.to_owned(),
        input_tokens: 0,
        output_tokens: 0,
        latency_ms: 0,
        idempotent_decision_key: format!(
            "{evaluation_id}:lab.pipette_tip_attached:pipette:left-pipetter:channel-1"
        ),
        adoption,
    }
}

fn ensure_decisions_config(semantic: &SemanticIndexingConfig) -> Result<(), String> {
    if semantic.decisions.provider() != "jev" {
        return Err("semantic_indexing.decisions.provider must be jev".into());
    }
    if semantic.decisions.model() != a2a_lab_dev_kit::DEFAULT_JEV_MODEL_REVISION {
        return Err(format!(
            "semantic_indexing.decisions.model must be {}",
            a2a_lab_dev_kit::DEFAULT_JEV_MODEL_REVISION
        ));
    }
    Ok(())
}

fn build_smoke_request(
    frame: &a2a_lab_dev_kit::SavedFrameRef,
    evaluation_id: &str,
) -> Result<FrameDecisionEvaluationRequest, String> {
    use a2a_lab_dev_kit::{
        DecisionEvidenceItem, DecisionSubject, FrameRoiTransform, JevEvaluationId, LabPredicate,
        PipetteChannelSubject,
    };
    let subject = DecisionSubject::PipetteChannel(
        PipetteChannelSubject::new("left-pipetter", "channel-1", None)
            .map_err(|e| e.to_string())?,
    );
    let evidence = vec![
        DecisionEvidenceItem::saved_frame_revision(
            frame.frame_id().clone(),
            FrameRoiTransform::new([0.0, 0.0, 1.0, 1.0]).map_err(|e| e.to_string())?,
        )
        .map_err(|e| e.to_string())?,
    ];
    FrameDecisionEvaluationRequest::new(
        FrameDecisionEvaluatorLimits::pilot(),
        JevEvaluationId::new(evaluation_id).map_err(|e| e.to_string())?,
        frame.clone(),
        LabPredicate::pipette_tip_attached_v1(),
        subject,
        evidence,
        pilot_operator_criteria(),
        FrameRoiTransform::new([0.0, 0.0, 1.0, 1.0]).map_err(|e| e.to_string())?,
        4096,
        640,
    )
    .map_err(|error| error.to_string())
}
