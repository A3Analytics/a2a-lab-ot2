//! Text-embedding adapter for persisted descriptions and search queries.

use std::sync::atomic::{AtomicBool, Ordering};

use a2a_lab_dev_kit::videos::{
    KeyedTextEmbedding, SemanticIndexingFailure, TextEmbeddingLimits, TextEmbeddingProvider,
    TextEmbeddingProviderConfig, TextEmbeddingRequest, validate_embedding_batch,
};
use a2a_lab_dev_kit::{CredentialReference, VectorNormalization};
use rig::bedrock::client::Client as BedrockClient;
use rig::bedrock::embedding::EmbeddingModel as BedrockEmbeddingModel;
use rig::client::{EmbeddingsClient, ProviderClient};
use rig::embeddings::EmbeddingModel;
use rig::providers::openai::Client as OpenAiClient;
use rig::providers::openai::embedding::EmbeddingModel as OpenAiEmbeddingModel;
use tokio::time::timeout;

use super::credentials::{credential_env_configured, redacted_credential_env};
use crate::agent::{bedrock_profile, bedrock_region};

enum TextEmbeddingBackend {
    Bedrock(BedrockEmbeddingModel),
    OpenAi(OpenAiEmbeddingModel),
}

impl TextEmbeddingBackend {
    async fn embed_one(&self, text: &str) -> Result<Vec<f32>, SemanticIndexingFailure> {
        match self {
            Self::Bedrock(model) => embed_rig_texts(model, text).await,
            Self::OpenAi(model) => embed_rig_texts(model, text).await,
        }
    }
}

/// Maps configured embedding provider/model into one validated retrieval space.
pub struct ConfiguredTextEmbeddingProvider {
    config: TextEmbeddingProviderConfig,
    limits: TextEmbeddingLimits,
    backend: TextEmbeddingBackend,
    cancelled: AtomicBool,
}

impl ConfiguredTextEmbeddingProvider {
    /// Connects using approved provider clients and optional credential env gates.
    pub async fn connect(
        config: TextEmbeddingProviderConfig,
        limits: TextEmbeddingLimits,
        credentials: Option<&CredentialReference>,
    ) -> Result<Self, SemanticIndexingFailure> {
        if let Some(reference) = credentials
            && !credential_env_configured(reference)
        {
            return Err(SemanticIndexingFailure::ProviderUnavailable {
                message: format!(
                    "embedding credentials not configured ({})",
                    redacted_credential_env(reference)
                ),
            });
        }
        let backend = build_embedding_backend(&config).await?;
        Ok(Self {
            config,
            limits,
            backend,
            cancelled: AtomicBool::new(false),
        })
    }

    /// Signals in-flight [`Self::embed_texts`] calls to stop.
    pub fn cancel(&self) {
        self.cancelled.store(true, Ordering::SeqCst);
    }
}

async fn build_embedding_backend(
    config: &TextEmbeddingProviderConfig,
) -> Result<TextEmbeddingBackend, SemanticIndexingFailure> {
    let provider = config.provider();
    if provider.eq_ignore_ascii_case("bedrock") {
        let profile = bedrock_profile();
        let client = bedrock_client(&profile).await;
        let dims = usize::try_from(config.model_space().dimensions()).unwrap_or(0);
        return Ok(TextEmbeddingBackend::Bedrock(
            client.embedding_model_with_ndims(config.model(), dims),
        ));
    }
    if provider.eq_ignore_ascii_case("openai") {
        let client = OpenAiClient::from_env().map_err(|error| {
            SemanticIndexingFailure::ProviderUnavailable {
                message: format!("openai embedding client unavailable: {error}"),
            }
        })?;
        let dims = usize::try_from(config.model_space().dimensions()).unwrap_or(0);
        return Ok(TextEmbeddingBackend::OpenAi(
            client.embedding_model_with_ndims(config.model(), dims),
        ));
    }
    Err(SemanticIndexingFailure::ProviderUnavailable {
        message: format!("unsupported text embedding provider {provider}"),
    })
}

async fn bedrock_client(profile: &str) -> BedrockClient {
    let mut loader =
        aws_config::defaults(aws_config::BehaviorVersion::latest()).profile_name(profile);
    if let Some(region) = bedrock_region() {
        loader = loader.region(aws_config::Region::new(region));
    }
    aws_sdk_bedrockruntime::Client::new(&loader.load().await).into()
}

impl TextEmbeddingProvider for ConfiguredTextEmbeddingProvider {
    fn config(&self) -> &TextEmbeddingProviderConfig {
        &self.config
    }

    fn limits(&self) -> TextEmbeddingLimits {
        self.limits
    }

    async fn embed_texts(
        &self,
        requests: Vec<TextEmbeddingRequest>,
    ) -> Result<Vec<KeyedTextEmbedding>, SemanticIndexingFailure> {
        let deadline = self.limits.deadline();
        timeout(deadline, self.embed_texts_inner(requests))
            .await
            .map_err(|_| SemanticIndexingFailure::DeadlineExceeded)?
    }
}

impl ConfiguredTextEmbeddingProvider {
    async fn embed_texts_inner(
        &self,
        requests: Vec<TextEmbeddingRequest>,
    ) -> Result<Vec<KeyedTextEmbedding>, SemanticIndexingFailure> {
        if self.cancelled.load(Ordering::SeqCst) {
            return Err(SemanticIndexingFailure::Cancelled);
        }
        let mut responses = Vec::with_capacity(requests.len());
        for request in &requests {
            if self.cancelled.load(Ordering::SeqCst) {
                return Err(SemanticIndexingFailure::Cancelled);
            }
            let values = self.backend.embed_one(request.text()).await?;
            let adjusted = adjust_vector(values, self.config.model_space().normalization())?;
            let keyed = KeyedTextEmbedding::new(
                self.config(),
                request.key().clone(),
                request.text(),
                adjusted,
            )?;
            responses.push(keyed);
        }
        validate_embedding_batch(self.config(), self.limits, &requests, &responses)
    }
}

async fn embed_rig_texts<M: EmbeddingModel>(
    model: &M,
    text: &str,
) -> Result<Vec<f32>, SemanticIndexingFailure> {
    let embeddings = model
        .embed_texts([text.to_owned()])
        .await
        .map_err(|error| SemanticIndexingFailure::ProviderUnavailable {
            message: error.to_string(),
        })?;
    let Some(first) = embeddings.first() else {
        return Err(SemanticIndexingFailure::PartialResponse {
            message: "embedding provider returned no vectors".to_owned(),
        });
    };
    let mut values = Vec::with_capacity(first.vec.len());
    for value in &first.vec {
        if !value.is_finite() {
            return Err(SemanticIndexingFailure::InvalidVector {
                message: "non-finite embedding component".to_owned(),
            });
        }
        #[allow(clippy::cast_possible_truncation)]
        values.push(*value as f32);
    }
    Ok(values)
}

fn adjust_vector(
    mut values: Vec<f32>,
    normalization: VectorNormalization,
) -> Result<Vec<f32>, SemanticIndexingFailure> {
    if values.iter().copied().any(|value| !value.is_finite()) {
        return Err(SemanticIndexingFailure::InvalidVector {
            message: "non-finite embedding component".to_owned(),
        });
    }
    if normalization == VectorNormalization::L2Unit {
        let norm_sq: f32 = values.iter().map(|value| value * value).sum();
        let norm = norm_sq.sqrt();
        if norm == 0.0 {
            return Err(SemanticIndexingFailure::InvalidVector {
                message: "zero embedding vector".to_owned(),
            });
        }
        for value in &mut values {
            *value /= norm;
        }
    }
    Ok(values)
}
