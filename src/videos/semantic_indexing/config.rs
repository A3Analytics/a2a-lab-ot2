//! Builds dev-kit provider configs from v2 semantic indexing settings.

use a2a_lab_dev_kit::{
    A2aLabError, EmbeddingMetric, FrameDescriptionBackend, FrameDescriptionProviderConfig,
    SemanticDescriptionConfig, SemanticEmbeddingsConfig, SemanticIndexingConfig,
    TextEmbeddingProviderConfig, VectorNormalization,
};

use super::cohere::DEFAULT_COHERE_EMBED_DIMENSIONS;
use super::prompt::{description_preprocessing_digest, description_prompt_digest};

/// Default local BGE model and its immutable retrieval-space contract.
pub const DEFAULT_LOCAL_BGE_MODEL: &str = "BAAI/bge-small-en-v1.5";

/// Resolved Luna description config independent from chat.
pub fn description_provider_config(
    description: &SemanticDescriptionConfig,
) -> Result<FrameDescriptionProviderConfig, A2aLabError> {
    description_provider_config_for_model(description, None)
}

/// Builds a description provider config with an optional source override.
pub fn description_provider_config_for_model(
    description: &SemanticDescriptionConfig,
    model: Option<&str>,
) -> Result<FrameDescriptionProviderConfig, A2aLabError> {
    FrameDescriptionProviderConfig::new(
        description.backend(),
        Some(model.unwrap_or_else(|| description.model())),
        description_prompt_digest(),
        description_preprocessing_digest(),
    )
}

/// Resolved text-embedding config independent from chat and Jev.
pub fn embedding_provider_config(
    embeddings: &SemanticEmbeddingsConfig,
) -> Result<TextEmbeddingProviderConfig, A2aLabError> {
    embedding_provider_config_for_model(embeddings, None)
}

/// Builds an embedding config with a source model override in the same
/// validated vector space.
pub fn embedding_provider_config_for_model(
    embeddings: &SemanticEmbeddingsConfig,
    model: Option<&str>,
) -> Result<TextEmbeddingProviderConfig, A2aLabError> {
    let model = model.unwrap_or_else(|| embeddings.model());
    let space = embeddings.space();
    if embeddings.provider().eq_ignore_ascii_case("bedrock")
        && model.starts_with("cohere.")
        && space.dimensions != DEFAULT_COHERE_EMBED_DIMENSIONS
    {
        return Err(A2aLabError::invalid(
            "semantic_indexing.embeddings.space.dimensions",
            "Cohere Bedrock embeddings require 1024 dimensions",
        ));
    }
    if embeddings.provider().eq_ignore_ascii_case("local_bge")
        && (model != DEFAULT_LOCAL_BGE_MODEL
            || space.dimensions != 384
            || space.normalization != VectorNormalization::L2Unit
            || space.metric != EmbeddingMetric::Cosine)
    {
        return Err(A2aLabError::invalid(
            "semantic_indexing.embeddings.space",
            "local_bge requires BAAI/bge-small-en-v1.5, 384 dimensions, l2_unit normalization, and cosine metric",
        ));
    }
    let identity = a2a_lab_dev_kit::ModelSpaceIdentity::new(
        embeddings.provider(),
        model,
        space.dimensions,
        space.normalization,
        space.metric,
        space.generation,
    )?;
    TextEmbeddingProviderConfig::new(embeddings.provider(), model, identity)
}

/// Convenience: description Luna default for Bedrock when model omitted in file.
#[must_use]
pub fn default_open_luna_description_provider() -> FrameDescriptionProviderConfig {
    FrameDescriptionProviderConfig::new(
        FrameDescriptionBackend::Bedrock,
        None::<&str>,
        description_prompt_digest(),
        description_preprocessing_digest(),
    )
    .expect("bedrock luna defaults")
}

pub fn default_open_luna_embedding_provider(
    semantic: &SemanticIndexingConfig,
) -> Result<TextEmbeddingProviderConfig, A2aLabError> {
    let embeddings = semantic
        .embeddings
        .as_ref()
        .ok_or_else(|| A2aLabError::invalid("semantic_indexing.mode", "embeddings are disabled"))?;
    embedding_provider_config(embeddings)
}

/// Convenience: minimal embedding config for contract tests.
#[cfg(test)]
mod tests {
    use super::*;
    use a2a_lab_dev_kit::A2aLabConfig;

    #[test]
    fn cohere_space_requires_1024_dimensions() {
        let document = r#"{"schema_version":2,"semantic_indexing":{
            "embeddings":{"provider":"bedrock","model":"cohere.embed-english-v3","space":{"dimensions":384,"normalization":"l2_unit","metric":"cosine","generation":1}},
            "limits":{
                "worker":{"max_batch_items":1,"max_concurrency":1,"max_queue_backlog":1,"max_retry_attempts":1,"job_deadline":"30s","retry_backoff":"5s"},
                "search":{"top_k":1,"candidate_limit":1,"max_neighbor_frames":1,"query_window":"1h"},
                "roi":{"min_edge_pixels":280,"max_edge_pixels":1568,"max_rois_per_frame":1},
                "storage":{"vector_index_path":"./semantic","max_disk_bytes":1024},
                "decisions":{"max_batch_items":1,"max_concurrency":1,"job_deadline":"45s","coverage_policy":"on_demand"}
            }
        }}"#;
        let config = A2aLabConfig::parse(document).unwrap();
        let embeddings = config
            .semantic_indexing
            .as_ref()
            .unwrap()
            .embeddings
            .as_ref()
            .unwrap();
        let error = embedding_provider_config(embeddings).unwrap_err();
        assert!(error.to_string().contains("1024"));
    }
}
