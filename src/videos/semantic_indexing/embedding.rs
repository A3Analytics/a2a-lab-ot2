//! Text-embedding adapter for persisted descriptions and search queries.

use std::path::{Path, PathBuf};
use std::sync::{
    Mutex,
    atomic::{AtomicBool, Ordering},
};

use a2a_lab_dev_kit::videos::{
    KeyedTextEmbedding, SemanticIndexingFailure, TextEmbeddingLimits, TextEmbeddingProvider,
    TextEmbeddingProviderConfig, TextEmbeddingRequest, validate_embedding_batch,
};
use a2a_lab_dev_kit::{CredentialReference, EmbeddingMetric, VectorNormalization};
use fastembed::{
    InitOptionsUserDefined, Pooling, TextEmbedding, TokenizerFiles, UserDefinedEmbeddingModel,
};
use rig::bedrock::client::Client as BedrockClient;
use rig::client::{EmbeddingsClient, ProviderClient};
use rig::embeddings::EmbeddingModel;
use rig::providers::openai::Client as OpenAiClient;
use rig::providers::openai::embedding::EmbeddingModel as OpenAiEmbeddingModel;
use tokio::time::timeout;

use super::bedrock::{load_bedrock_sdk_config, map_bedrock_invoke_failure};
use super::credentials::{credential_env_configured, redacted_credential_env};

enum TextEmbeddingBackend {
    Bedrock {
        sdk: Box<aws_config::SdkConfig>,
        model: String,
        dims: usize,
    },
    OpenAi(OpenAiEmbeddingModel),
    LocalBge(Box<Mutex<TextEmbedding>>),
}

impl TextEmbeddingBackend {
    async fn embed_one(&self, text: &str) -> Result<Vec<f32>, SemanticIndexingFailure> {
        match self {
            Self::Bedrock { sdk, model, dims } => {
                let client = bedrock_client(sdk);
                let embedding = client.embedding_model_with_ndims(model, *dims);
                embed_rig_texts(&embedding, text).await
            }
            Self::OpenAi(model) => embed_rig_texts(model, text).await,
            Self::LocalBge(model) => {
                let mut model =
                    model
                        .lock()
                        .map_err(|_| SemanticIndexingFailure::ProviderUnavailable {
                            message: "local BGE model lock poisoned".to_owned(),
                        })?;
                model
                    .embed([text], Some(1))
                    .map_err(|error| SemanticIndexingFailure::ProviderUnavailable {
                        message: error.to_string(),
                    })?
                    .into_iter()
                    .next()
                    .ok_or_else(|| SemanticIndexingFailure::PartialResponse {
                        message: "local BGE returned no vectors".to_owned(),
                    })
            }
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
        let sdk = load_bedrock_sdk_config().await?;
        let dims = usize::try_from(config.model_space().dimensions()).unwrap_or(0);
        return Ok(TextEmbeddingBackend::Bedrock {
            sdk: Box::new(sdk),
            model: config.model().to_owned(),
            dims,
        });
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
    if provider.eq_ignore_ascii_case("local_bge") {
        validate_local_bge_space(config)?;
        let model_path = std::env::var_os("A2ALAB_LOCAL_BGE_MODEL_PATH").ok_or_else(|| {
            SemanticIndexingFailure::ProviderUnavailable {
                message: "local BGE model unavailable: set A2ALAB_LOCAL_BGE_MODEL_PATH to a populated BGE model directory".to_owned(),
            }
        })?;
        let model = load_local_bge(Path::new(&model_path))?;
        return Ok(TextEmbeddingBackend::LocalBge(Box::new(Mutex::new(model))));
    }
    Err(SemanticIndexingFailure::ProviderUnavailable {
        message: format!("unsupported text embedding provider {provider}"),
    })
}

fn validate_local_bge_space(
    config: &TextEmbeddingProviderConfig,
) -> Result<(), SemanticIndexingFailure> {
    let space = config.model_space();
    if config.model() != "BAAI/bge-small-en-v1.5"
        || space.dimensions() != 384
        || space.normalization() != VectorNormalization::L2Unit
        || space.metric() != EmbeddingMetric::Cosine
    {
        return Err(SemanticIndexingFailure::ProviderUnavailable {
            message: "local_bge requires model BAAI/bge-small-en-v1.5, 384 dimensions, l2_unit normalization, and cosine metric".to_owned(),
        });
    }
    Ok(())
}

fn load_local_bge(root: &Path) -> Result<TextEmbedding, SemanticIndexingFailure> {
    let onnx = first_existing(root, ["model.onnx", "onnx/model.onnx"])?;
    let read = |name: &str| {
        std::fs::read(root.join(name)).map_err(|error| {
            SemanticIndexingFailure::ProviderUnavailable {
                message: format!("local BGE model unavailable: cannot read {name}: {error}"),
            }
        })
    };
    let model = UserDefinedEmbeddingModel::new(
        std::fs::read(&onnx).map_err(|error| SemanticIndexingFailure::ProviderUnavailable {
            message: format!(
                "local BGE model unavailable: cannot read {}: {error}",
                onnx.display()
            ),
        })?,
        TokenizerFiles {
            tokenizer_file: read("tokenizer.json")?,
            config_file: read("config.json")?,
            special_tokens_map_file: read("special_tokens_map.json")?,
            tokenizer_config_file: read("tokenizer_config.json")?,
        },
    )
    .with_pooling(Pooling::Cls);
    TextEmbedding::try_new_from_user_defined(model, InitOptionsUserDefined::new()).map_err(
        |error| SemanticIndexingFailure::ProviderUnavailable {
            message: format!("local BGE model unavailable: {error}"),
        },
    )
}

fn first_existing<const N: usize>(
    root: &Path,
    candidates: [&str; N],
) -> Result<PathBuf, SemanticIndexingFailure> {
    candidates
        .into_iter()
        .map(|candidate| root.join(candidate))
        .find(|path| path.is_file())
        .ok_or_else(|| SemanticIndexingFailure::ProviderUnavailable {
            message: format!(
                "local BGE model unavailable: no ONNX model found under {}",
                root.display()
            ),
        })
}

fn bedrock_client(config: &aws_config::SdkConfig) -> BedrockClient {
    aws_sdk_bedrockruntime::Client::new(config).into()
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
        .map_err(|error| map_bedrock_invoke_failure(&error.to_string()))?;
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

#[cfg(test)]
mod tests {
    use super::*;
    use a2a_lab_dev_kit::{EmbeddingMetric, ModelSpaceIdentity, ProvenanceGeneration};

    fn config() -> TextEmbeddingProviderConfig {
        TextEmbeddingProviderConfig::new(
            "local_bge",
            "BAAI/bge-small-en-v1.5",
            ModelSpaceIdentity::new(
                "local_bge",
                "BAAI/bge-small-en-v1.5",
                384,
                VectorNormalization::L2Unit,
                EmbeddingMetric::Cosine,
                ProvenanceGeneration::new(1).expect("generation"),
            )
            .expect("space"),
        )
        .expect("config")
    }

    #[tokio::test]
    async fn local_bge_without_explicit_path_is_typed_unavailable() {
        // Do not inspect the default Hugging Face cache: local_bge is explicitly offline.
        if std::env::var_os("A2ALAB_LOCAL_BGE_MODEL_PATH").is_some() {
            return;
        }
        let Err(error) = build_embedding_backend(&config()).await else {
            panic!("model path unexpectedly configured");
        };
        assert!(matches!(
            error,
            SemanticIndexingFailure::ProviderUnavailable { message }
                if message.contains("A2ALAB_LOCAL_BGE_MODEL_PATH")
        ));
    }

    #[test]
    fn local_bge_rejects_noncanonical_space() {
        let invalid = TextEmbeddingProviderConfig::new(
            "local_bge",
            "BAAI/bge-small-en-v1.5",
            ModelSpaceIdentity::new(
                "local_bge",
                "BAAI/bge-small-en-v1.5",
                384,
                VectorNormalization::None,
                EmbeddingMetric::Dot,
                ProvenanceGeneration::new(1).expect("generation"),
            )
            .expect("space"),
        )
        .expect("config");
        let error = validate_local_bge_space(&invalid).expect_err("invalid space");
        assert!(matches!(
            error,
            SemanticIndexingFailure::ProviderUnavailable { message }
                if message.contains("l2_unit") && message.contains("cosine")
        ));
    }
}
