//! Configurable Luna frame-description adapter.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use a2a_lab_dev_kit::CredentialReference;
use a2a_lab_dev_kit::videos::{
    FrameDescriptionBackend, FrameDescriptionLimits, FrameDescriptionProvider,
    FrameDescriptionProviderConfig, FrameDescriptionRequest, KeyedFrameDescription,
    SemanticIndexingFailure, validate_description_batch,
};
use base64::Engine;
use rig::bedrock::client::Client as BedrockClient;
use rig::client::{CompletionClient, ProviderClient};
use rig::completion::{AssistantContent, CompletionModel, CompletionRequest, Message};
use rig::message::{DocumentSourceKind, Image, ImageMediaType, UserContent};
use rig::providers::openai::Client as OpenAiClient;
use tokio::time::timeout;

use super::bedrock::{load_bedrock_sdk_config, map_bedrock_invoke_failure};
use super::credentials::{credential_env_configured, redacted_credential_env};
use super::media::FrameDescriptionMedia;
use super::prompt::DESCRIPTION_PROMPT;

enum LunaCompletionModel {
    Bedrock {
        sdk: Box<aws_config::SdkConfig>,
        model: String,
    },
    OpenAi(<OpenAiClient as CompletionClient>::CompletionModel),
}

impl LunaCompletionModel {
    async fn describe_jpeg(&self, jpeg: Vec<u8>) -> Result<String, SemanticIndexingFailure> {
        match self {
            Self::Bedrock { sdk, model } => {
                let client = bedrock_client(sdk);
                let completion = client.completion_model(model);
                describe_with_model(&completion, jpeg).await
            }
            Self::OpenAi(model) => describe_with_model(model, jpeg).await,
        }
    }
}

/// Luna description provider with separate credentials from chat.
pub struct LunaFrameDescriptionProvider<M: FrameDescriptionMedia> {
    config: FrameDescriptionProviderConfig,
    limits: FrameDescriptionLimits,
    media: Arc<M>,
    model: LunaCompletionModel,
    cancelled: Arc<AtomicBool>,
}

impl<M: FrameDescriptionMedia + 'static> LunaFrameDescriptionProvider<M> {
    /// Builds a provider from configuration and a JPEG source.
    pub async fn connect(
        config: FrameDescriptionProviderConfig,
        limits: FrameDescriptionLimits,
        media: Arc<M>,
        credentials: Option<&CredentialReference>,
    ) -> Result<Self, SemanticIndexingFailure> {
        if let Some(reference) = credentials
            && !credential_env_configured(reference)
        {
            return Err(SemanticIndexingFailure::ProviderUnavailable {
                message: format!(
                    "description credentials not configured ({})",
                    redacted_credential_env(reference)
                ),
            });
        }
        let model = build_completion_model(&config).await?;
        Ok(Self {
            config,
            limits,
            media,
            model,
            cancelled: Arc::new(AtomicBool::new(false)),
        })
    }

    /// Signals in-flight [`Self::describe_frames`] calls to stop.
    pub fn cancel(&self) {
        self.cancelled.store(true, Ordering::SeqCst);
    }
}

async fn build_completion_model(
    config: &FrameDescriptionProviderConfig,
) -> Result<LunaCompletionModel, SemanticIndexingFailure> {
    match config.backend() {
        FrameDescriptionBackend::Bedrock => {
            let sdk = load_bedrock_sdk_config().await?;
            Ok(LunaCompletionModel::Bedrock {
                sdk: Box::new(sdk),
                model: config.model().to_owned(),
            })
        }
        FrameDescriptionBackend::Openai => {
            let client = OpenAiClient::from_env().map_err(|error| {
                SemanticIndexingFailure::ProviderUnavailable {
                    message: format!(
                        "openai description client unavailable (check OPENAI_API_KEY): {error}"
                    ),
                }
            })?;
            Ok(LunaCompletionModel::OpenAi(
                client.completion_model(config.model()),
            ))
        }
    }
}

fn bedrock_client(config: &aws_config::SdkConfig) -> BedrockClient {
    aws_sdk_bedrockruntime::Client::new(config).into()
}

impl<M: FrameDescriptionMedia + Send + Sync + 'static> FrameDescriptionProvider
    for LunaFrameDescriptionProvider<M>
{
    fn config(&self) -> &FrameDescriptionProviderConfig {
        &self.config
    }

    fn limits(&self) -> FrameDescriptionLimits {
        self.limits
    }

    async fn describe_frames(
        &self,
        requests: Vec<FrameDescriptionRequest>,
    ) -> Result<Vec<KeyedFrameDescription>, SemanticIndexingFailure> {
        if requests.len() > usize::try_from(self.limits.max_batch_items()).unwrap_or(usize::MAX) {
            return Err(SemanticIndexingFailure::LimitExceeded {
                limit: "max_batch_items".to_owned(),
                message: format!(
                    "batch size {} exceeds {}",
                    requests.len(),
                    self.limits.max_batch_items()
                ),
            });
        }
        let deadline = self.limits.deadline();
        timeout(deadline, self.describe_frames_inner(requests))
            .await
            .map_err(|_| SemanticIndexingFailure::DeadlineExceeded)?
    }
}

impl<M: FrameDescriptionMedia + Send + Sync + 'static> LunaFrameDescriptionProvider<M> {
    async fn describe_frames_inner(
        &self,
        requests: Vec<FrameDescriptionRequest>,
    ) -> Result<Vec<KeyedFrameDescription>, SemanticIndexingFailure> {
        if self.cancelled.load(Ordering::SeqCst) {
            return Err(SemanticIndexingFailure::Cancelled);
        }
        let mut responses = Vec::with_capacity(requests.len());
        for request in &requests {
            if self.cancelled.load(Ordering::SeqCst) {
                return Err(SemanticIndexingFailure::Cancelled);
            }
            let jpeg = self.media.jpeg_for(request).await?;
            if jpeg.len() > self.limits.max_input_bytes() {
                return Err(SemanticIndexingFailure::LimitExceeded {
                    limit: "max_input_bytes".to_owned(),
                    message: format!(
                        "jpeg {} exceeds {}",
                        jpeg.len(),
                        self.limits.max_input_bytes()
                    ),
                });
            }
            let text = self.model.describe_jpeg(jpeg).await?;
            let keyed = KeyedFrameDescription::new(
                self.limits,
                self.config(),
                request.key().clone(),
                text,
            )?;
            responses.push(keyed);
        }
        validate_description_batch(self.limits, &requests, &responses)
    }
}

async fn describe_with_model<M: CompletionModel + Clone>(
    model: &M,
    jpeg: Vec<u8>,
) -> Result<String, SemanticIndexingFailure> {
    let image = encoded_jpeg_image(&jpeg);
    let request = build_description_request(model, image);
    let response = model
        .completion(request)
        .await
        .map_err(|error| map_completion_error(&error))?;
    let mut parts = Vec::new();
    for item in response.choice {
        if let AssistantContent::Text(text) = item {
            parts.push(text.text);
        }
    }
    if parts.is_empty() {
        return Err(SemanticIndexingFailure::EmptyDescription);
    }
    Ok(parts.join(" ").trim().to_owned())
}

fn build_description_request<M: CompletionModel + Clone>(
    model: &M,
    image: Image,
) -> CompletionRequest {
    model
        .completion_request("Describe this lab video frame for search retrieval.")
        .preamble(DESCRIPTION_PROMPT.to_owned())
        .message(Message::User {
            content: vec![
                UserContent::Image(image),
                UserContent::Text("Return only the description text.".to_owned().into()),
            ],
        })
        .build()
}

fn encoded_jpeg_image(jpeg: &[u8]) -> Image {
    Image {
        data: DocumentSourceKind::Base64(base64::engine::general_purpose::STANDARD.encode(jpeg)),
        media_type: Some(ImageMediaType::JPEG),
        detail: None,
        additional_params: None,
    }
}

fn map_completion_error(error: &rig::completion::CompletionError) -> SemanticIndexingFailure {
    map_bedrock_invoke_failure(&error.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bedrock_image_uses_standard_base64_and_preserves_jpeg_media_type() {
        let image = encoded_jpeg_image(&[0xff, 0xd8, 0x00, 0x01]);
        assert_eq!(image.media_type, Some(ImageMediaType::JPEG));
        assert_eq!(
            image.data,
            DocumentSourceKind::Base64("/9gAAQ==".to_owned())
        );
    }

    #[test]
    fn description_request_omits_unsupported_temperature() {
        let model = rig::providers::openai::Client::builder()
            .api_key("test")
            .build()
            .expect("test client")
            .completion_model("test-model");
        let request = build_description_request(&model, encoded_jpeg_image(b"jpeg"));
        assert_eq!(request.temperature, None);
    }

    #[test]
    fn description_preamble_stays_the_retrieval_prompt() {
        let model = rig::providers::openai::Client::builder()
            .api_key("test")
            .build()
            .expect("test client")
            .completion_model("test-model");
        let request = build_description_request(&model, encoded_jpeg_image(b"jpeg"));
        let preambles: Vec<_> = request
            .chat_history
            .iter()
            .filter_map(|message| match message {
                rig::completion::Message::System { content } => Some(content.as_str()),
                _ => None,
            })
            .collect();
        assert_eq!(preambles, [DESCRIPTION_PROMPT]);
    }
}
