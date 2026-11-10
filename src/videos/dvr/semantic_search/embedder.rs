//! Query text embedding for DVR semantic search.

use std::sync::Arc;

use a2a_lab_dev_kit::{
    A2aLabError, FakeTextEmbeddingProvider, FrameEmbeddingVector, ModelSpaceIdentity,
    SemanticIndexingFailure, Sha256Digest, TextEmbeddingKey, TextEmbeddingLimits,
    TextEmbeddingProvider, TextEmbeddingProviderConfig, TextEmbeddingRequest,
};

use crate::videos::semantic_indexing::ConfiguredTextEmbeddingProvider;

/// Embedding client used only for semantic search queries.
#[derive(Clone)]
pub enum DvrSearchEmbedder {
    /// Deterministic fake for CI and unit tests.
    Deterministic(Arc<FakeTextEmbeddingProvider>),
    /// Live configured text-embedding adapter.
    Live(Arc<ConfiguredTextEmbeddingProvider>),
}

impl DvrSearchEmbedder {
    /// Active provider configuration and model space.
    #[must_use]
    pub fn config(&self) -> &TextEmbeddingProviderConfig {
        match self {
            Self::Deterministic(provider) => provider.config(),
            Self::Live(provider) => provider.config(),
        }
    }

    fn limits(&self) -> TextEmbeddingLimits {
        match self {
            Self::Deterministic(provider) => provider.limits(),
            Self::Live(provider) => provider.limits(),
        }
    }

    /// Embeds search query text in the configured model space.
    pub async fn embed_query_text(
        &self,
        text: &str,
    ) -> Result<(ModelSpaceIdentity, FrameEmbeddingVector), A2aLabError> {
        let key = TextEmbeddingKey::SearchQuery(Sha256Digest::hash_bytes(text.as_bytes()));
        let request =
            TextEmbeddingRequest::new(self.limits(), key, text, 1).map_err(map_indexing_failure)?;
        let responses = match self {
            Self::Deterministic(provider) => provider.embed_texts(vec![request]).await,
            Self::Live(provider) => provider.embed_texts(vec![request]).await,
        }
        .map_err(map_indexing_failure)?;
        let embedded = responses.into_iter().next().ok_or_else(|| {
            A2aLabError::unavailable("text embedding provider returned no query vector")
        })?;
        if embedded.model_space() != self.config().model_space() {
            return Err(A2aLabError::invalid(
                "query_embedding",
                "provider returned a different model space than configured",
            ));
        }
        Ok((embedded.model_space().clone(), embedded.vector().clone()))
    }
}

fn map_indexing_failure(failure: SemanticIndexingFailure) -> A2aLabError {
    failure.into_lab_error()
}
