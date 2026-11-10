//! Builds dev-kit provider configs from v2 semantic indexing settings.

use a2a_lab_dev_kit::{
    A2aLabError, EmbeddingMetric, FrameDescriptionBackend, FrameDescriptionProviderConfig,
    SemanticDescriptionConfig, SemanticEmbeddingsConfig, SemanticIndexingConfig,
    TextEmbeddingProviderConfig, VectorNormalization,
};

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
        a2a_lab_dev_kit::ProvenanceGeneration::new(space.generation)?,
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

/// Convenience: minimal embedding config for contract tests.
pub fn default_open_luna_embedding_provider(
    semantic: &SemanticIndexingConfig,
) -> Result<TextEmbeddingProviderConfig, A2aLabError> {
    let embeddings = semantic
        .embeddings
        .as_ref()
        .ok_or_else(|| A2aLabError::invalid("semantic_indexing.mode", "embeddings are disabled"))?;
    embedding_provider_config(embeddings)
}
