//! Builds deterministic or live semantic indexing provider pairs.

use std::sync::Arc;

use a2a_lab_dev_kit::{
    FakeDescriptionBehavior, FakeEmbeddingBehavior, FakeFrameDescriptionProvider,
    FakeTextEmbeddingProvider, SemanticIndexingConfig, SemanticStreamSchedule,
};

use super::bedrock::{ensure_live_bedrock_description, ensure_live_bedrock_ready};
use super::bundle::SemanticIndexingProviders;
use super::config::{
    description_provider_config, description_provider_config_for_model, embedding_provider_config,
    embedding_provider_config_for_model,
};
use super::description::LunaFrameDescriptionProvider;
use super::embedding::ConfiguredTextEmbeddingProvider;
use super::limits::{frame_description_limits, text_embedding_limits};
use super::media::FrameDescriptionMedia;
use crate::videos::dvr::DvrSearchEmbedder;

/// Whether adapters call external inference APIs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SemanticIndexingAdapterMode {
    /// Dev-kit fakes (default for CI and unit tests).
    Deterministic,
    /// Luna and configured text-embedding clients when credentials are present.
    Live,
}

/// Wires description and embedding boundaries without decision-store access.
pub async fn build_semantic_indexing_providers<M: FrameDescriptionMedia + 'static>(
    semantic: &SemanticIndexingConfig,
    media: Arc<M>,
    mode: SemanticIndexingAdapterMode,
) -> Result<
    SemanticIndexingProviders<FakeFrameDescriptionProvider, FakeTextEmbeddingProvider>,
    a2a_lab_dev_kit::A2aLabError,
> {
    let _ = media;
    match mode {
        SemanticIndexingAdapterMode::Deterministic => build_deterministic_providers(semantic).await,
        SemanticIndexingAdapterMode::Live => Err(a2a_lab_dev_kit::A2aLabError::invalid(
            "semantic_indexing",
            "live adapters require build_live_providers",
        )),
    }
}

/// Deterministic query embedder for DVR semantic search.
pub async fn build_deterministic_search_embedder(
    semantic: &SemanticIndexingConfig,
) -> Result<DvrSearchEmbedder, a2a_lab_dev_kit::A2aLabError> {
    Ok(DvrSearchEmbedder::Deterministic(Arc::new(
        build_deterministic_providers(semantic).await?.embedding,
    )))
}

/// Deterministic provider pair for tests and CI.
pub async fn build_deterministic_providers(
    semantic: &SemanticIndexingConfig,
) -> Result<
    SemanticIndexingProviders<FakeFrameDescriptionProvider, FakeTextEmbeddingProvider>,
    a2a_lab_dev_kit::A2aLabError,
> {
    let description_config = description_provider_config(&semantic.description)?;
    let embeddings = semantic.embeddings.as_ref().ok_or_else(|| {
        a2a_lab_dev_kit::A2aLabError::invalid(
            "semantic_indexing.embeddings",
            "embeddings are disabled",
        )
    })?;
    let embedding_config = embedding_provider_config(embeddings)?;
    let description_limits = frame_description_limits(&semantic.limits);
    let embedding_limits = text_embedding_limits(&semantic.limits);
    Ok(SemanticIndexingProviders::new(
        FakeFrameDescriptionProvider::new(
            description_config,
            description_limits,
            FakeDescriptionBehavior::Ok,
        ),
        FakeTextEmbeddingProvider::new(
            embedding_config,
            embedding_limits,
            FakeEmbeddingBehavior::Ok,
        ),
    ))
}

/// Deterministic providers resolved for one source's model overrides.
pub async fn build_deterministic_providers_for_source(
    semantic: &SemanticIndexingConfig,
    schedule: &SemanticStreamSchedule,
) -> Result<
    SemanticIndexingProviders<FakeFrameDescriptionProvider, FakeTextEmbeddingProvider>,
    a2a_lab_dev_kit::A2aLabError,
> {
    let description_config =
        description_provider_config_for_model(&semantic.description, schedule.description_model())?;
    let embeddings = semantic.embeddings.as_ref().ok_or_else(|| {
        a2a_lab_dev_kit::A2aLabError::invalid(
            "semantic_indexing.embeddings",
            "embeddings are disabled",
        )
    })?;
    let embedding_config =
        embedding_provider_config_for_model(embeddings, schedule.embedding_model())?;
    Ok(SemanticIndexingProviders::new(
        FakeFrameDescriptionProvider::new(
            description_config,
            frame_description_limits(&semantic.limits),
            FakeDescriptionBehavior::Ok,
        ),
        FakeTextEmbeddingProvider::new(
            embedding_config,
            text_embedding_limits(&semantic.limits),
            FakeEmbeddingBehavior::Ok,
        ),
    ))
}

/// Builds the deterministic Luna-only provider for description-only mode.
pub async fn build_deterministic_description_provider(
    semantic: &SemanticIndexingConfig,
    model: Option<&str>,
) -> Result<FakeFrameDescriptionProvider, a2a_lab_dev_kit::A2aLabError> {
    Ok(FakeFrameDescriptionProvider::new(
        description_provider_config_for_model(&semantic.description, model)?,
        frame_description_limits(&semantic.limits),
        FakeDescriptionBehavior::Ok,
    ))
}

/// Live Luna description and configured embedding clients (opt-in credentials).
pub async fn build_live_providers<M: FrameDescriptionMedia + Send + Sync + 'static>(
    semantic: &SemanticIndexingConfig,
    media: Arc<M>,
) -> Result<
    SemanticIndexingProviders<LunaFrameDescriptionProvider<M>, ConfiguredTextEmbeddingProvider>,
    a2a_lab_dev_kit::A2aLabError,
> {
    let description_config = description_provider_config(&semantic.description)?;
    let embeddings = semantic.embeddings.as_ref().ok_or_else(|| {
        a2a_lab_dev_kit::A2aLabError::invalid(
            "semantic_indexing.embeddings",
            "embeddings are disabled",
        )
    })?;
    let embedding_config = embedding_provider_config(embeddings)?;
    let description_limits = frame_description_limits(&semantic.limits);
    let embedding_limits = text_embedding_limits(&semantic.limits);
    ensure_live_bedrock_ready(&description_config, &embedding_config)
        .await
        .map_err(map_indexing_failure)?;
    let description = LunaFrameDescriptionProvider::connect(
        description_config,
        description_limits,
        media,
        semantic.description.credentials(),
    )
    .await
    .map_err(map_indexing_failure)?;
    let embedding = ConfiguredTextEmbeddingProvider::connect(
        embedding_config,
        embedding_limits,
        embeddings.credentials(),
    )
    .await
    .map_err(map_indexing_failure)?;
    Ok(SemanticIndexingProviders::new(description, embedding))
}

/// Live providers resolved for one source's model overrides.
pub async fn build_live_providers_for_source<M: FrameDescriptionMedia + Send + Sync + 'static>(
    semantic: &SemanticIndexingConfig,
    media: Arc<M>,
    schedule: &SemanticStreamSchedule,
) -> Result<
    SemanticIndexingProviders<LunaFrameDescriptionProvider<M>, ConfiguredTextEmbeddingProvider>,
    a2a_lab_dev_kit::A2aLabError,
> {
    let description_config =
        description_provider_config_for_model(&semantic.description, schedule.description_model())?;
    let embeddings = semantic.embeddings.as_ref().ok_or_else(|| {
        a2a_lab_dev_kit::A2aLabError::invalid(
            "semantic_indexing.embeddings",
            "embeddings are disabled",
        )
    })?;
    let embedding_config =
        embedding_provider_config_for_model(embeddings, schedule.embedding_model())?;
    ensure_live_bedrock_ready(&description_config, &embedding_config)
        .await
        .map_err(map_indexing_failure)?;
    let description = LunaFrameDescriptionProvider::connect(
        description_config,
        frame_description_limits(&semantic.limits),
        media,
        semantic.description.credentials(),
    )
    .await
    .map_err(map_indexing_failure)?;
    let embedding = ConfiguredTextEmbeddingProvider::connect(
        embedding_config,
        text_embedding_limits(&semantic.limits),
        embeddings.credentials(),
    )
    .await
    .map_err(map_indexing_failure)?;
    Ok(SemanticIndexingProviders::new(description, embedding))
}

/// Builds the live Luna-only provider for description-only mode.
pub async fn build_live_description_provider<M: FrameDescriptionMedia + Send + Sync + 'static>(
    semantic: &SemanticIndexingConfig,
    media: Arc<M>,
    model: Option<&str>,
) -> Result<LunaFrameDescriptionProvider<M>, a2a_lab_dev_kit::A2aLabError> {
    let description_config = description_provider_config_for_model(&semantic.description, model)?;
    ensure_live_bedrock_description(&description_config)
        .await
        .map_err(map_indexing_failure)?;
    LunaFrameDescriptionProvider::connect(
        description_config,
        frame_description_limits(&semantic.limits),
        media,
        semantic.description.credentials(),
    )
    .await
    .map_err(map_indexing_failure)
}

fn map_indexing_failure(
    failure: a2a_lab_dev_kit::SemanticIndexingFailure,
) -> a2a_lab_dev_kit::A2aLabError {
    failure.into_lab_error()
}
