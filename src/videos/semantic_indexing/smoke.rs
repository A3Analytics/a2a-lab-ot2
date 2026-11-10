//! Opt-in smoke against existing retained Chupacabra frames (no new capture).

use std::path::Path;
use std::sync::Arc;

use a2a_lab_dev_kit::{
    FrameDescriptionProvider, FrameDescriptionRequest, LoadedLabConfig, SemanticIndexingConfig,
    TextEmbeddingKey, TextEmbeddingProvider, TextEmbeddingRequest,
    validate_description_batch, validate_embedding_batch,
};
use serde::Serialize;

use super::factory::build_live_providers;
use super::limits::{frame_description_limits, text_embedding_limits};
use super::media::EvidenceJpegMedia;
use crate::videos::dvr::{DEFAULT_DISK_VIDEO_RETENTION_SECS, IndexedVideoDvr};

/// Usage snapshot returned to stdout for authorized smoke runs.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct IndexingSmokeReport {
    mode: &'static str,
    description_provider: String,
    description_model: String,
    embedding_provider: String,
    embedding_model: String,
    frame_id: String,
    description_bytes: usize,
    embedding_dimensions: u32,
}

/// Runs one description + embedding cycle when `A2ALAB_SEMANTIC_INDEXING_SMOKE=1`.
pub async fn run_indexing_smoke(
    config_path: &Path,
    dvr_root: &Path,
    frame_id: &str,
) -> Result<IndexingSmokeReport, String> {
    if !smoke_enabled() {
        return Err(
            "semantic indexing smoke is disabled; set A2ALAB_SEMANTIC_INDEXING_SMOKE=1".into(),
        );
    }
    let loaded = LoadedLabConfig::load(config_path).map_err(|error| error.to_string())?;
    let semantic = loaded
        .config
        .semantic_indexing
        .as_ref()
        .ok_or_else(|| "configuration has no semantic_indexing section".to_owned())?;
    if semantic.embeddings.is_none() {
        return Err("semantic indexing smoke requires full embedding mode".to_owned());
    }
    ensure_live_credentials(semantic)?;

    let dvr = Arc::new(
        IndexedVideoDvr::open(dvr_root, DEFAULT_DISK_VIDEO_RETENTION_SECS)
            .await
            .map_err(|error| error.to_string())?,
    );
    let record = dvr
        .get_manifest_frame(frame_id)
        .await
        .map_err(|error| error.to_string())?;

    let frame = record.frame().clone();
    let description_limits = frame_description_limits(&semantic.limits);
    let embedding_limits = text_embedding_limits(&semantic.limits);
    let generation = record
        .description()
        .map_or(1u64, |value| value.provenance().generation().get());
    let description_key = a2a_lab_dev_kit::FrameDescriptionKey::new(
        frame.clone(),
        None,
        a2a_lab_dev_kit::ProvenanceGeneration::new(generation)
            .map_err(|error| error.to_string())?,
    );
    let description_request = FrameDescriptionRequest::new(
        description_limits,
        description_key,
        1024,
        description_limits.max_pixels(),
    )
    .map_err(|error| error.to_string())?;

    let media = Arc::new(EvidenceJpegMedia::new(Arc::clone(&dvr)));
    let providers = build_live_providers(semantic, media)
        .await
        .map_err(|error| error.to_string())?;

    let descriptions = providers
        .description
        .describe_frames(vec![description_request.clone()])
        .await
        .map_err(|error| error.to_string())?;
    let validated = validate_description_batch(
        description_limits,
        std::slice::from_ref(&description_request),
        &descriptions,
    )
    .map_err(|error| error.to_string())?;
    let text = validated[0].text().to_owned();

    let embed_key = TextEmbeddingKey::PersistedDescription(description_request.key().clone());
    let token_count = u32::try_from(text.len()).map_err(|error| error.to_string())?;
    let embed_request = TextEmbeddingRequest::new(embedding_limits, embed_key, &text, token_count)
        .map_err(|error| error.to_string())?;
    let embeddings = providers
        .embedding
        .embed_texts(vec![embed_request.clone()])
        .await
        .map_err(|error| error.to_string())?;
    validate_embedding_batch(
        providers.embedding.config(),
        embedding_limits,
        std::slice::from_ref(&embed_request),
        &embeddings,
    )
    .map_err(|error| error.to_string())?;

    Ok(IndexingSmokeReport {
        mode: "live",
        description_provider: providers.description.config().provider().to_owned(),
        description_model: providers.description.config().model().to_owned(),
        embedding_provider: providers.embedding.config().provider().to_owned(),
        embedding_model: providers.embedding.config().model().to_owned(),
        frame_id: frame_id.to_owned(),
        description_bytes: text.len(),
        embedding_dimensions: providers.embedding.config().model_space().dimensions(),
    })
}

fn smoke_enabled() -> bool {
    std::env::var("A2ALAB_SEMANTIC_INDEXING_SMOKE")
        .is_ok_and(|value| matches!(value.trim(), "1" | "true" | "yes"))
}

fn ensure_live_credentials(semantic: &SemanticIndexingConfig) -> Result<(), String> {
    if semantic.description.credentials().is_some()
        || semantic
            .embeddings
            .as_ref()
            .is_some_and(|embeddings| embeddings.credentials().is_some())
    {
        return Ok(());
    }
    if std::env::var("AWS_PROFILE").is_ok() || std::env::var("OPENAI_API_KEY").is_ok() {
        return Ok(());
    }
    Err("live smoke requires semantic indexing credentials or AWS_PROFILE/OPENAI_API_KEY".into())
}
