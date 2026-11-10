//! Configurable Luna frame-description adapter.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use a2a_lab_dev_kit::CredentialReference;
use a2a_lab_dev_kit::videos::{
    FrameDescriptionBackend, FrameDescriptionLimits, FrameDescriptionProvider,
    FrameDescriptionProviderConfig, FrameDescriptionRequest, KeyedFrameDescription,
    SemanticIndexingFailure, validate_description_batch,
};
use rig::bedrock::client::Client as BedrockClient;
use rig::bedrock::completion::CompletionModel as BedrockCompletionModel;
use rig::client::{CompletionClient, ProviderClient};
use rig::completion::{AssistantContent, CompletionModel, Message};
use rig::message::{DocumentSourceKind, Image, ImageMediaType, UserContent};
use rig::providers::openai::Client as OpenAiClient;
use tokio::time::timeout;

use super::credentials::{credential_env_configured, redacted_credential_env};
use super::media::FrameDescriptionMedia;
use super::prompt::DESCRIPTION_PROMPT;
use crate::agent::{bedrock_profile, bedrock_region};

enum LunaCompletionModel {
    Bedrock(BedrockCompletionModel),
    OpenAi(<OpenAiClient as CompletionClient>::CompletionModel),
}

impl LunaCompletionModel {
    async fn describe_jpeg(&self, jpeg: Vec<u8>) -> Result<String, SemanticIndexingFailure> {
        match self {
            Self::Bedrock(model) => describe_with_model(model, jpeg).await,
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
            let profile = bedrock_profile();
            let client = bedrock_client(&profile).await;
            Ok(LunaCompletionModel::Bedrock(
                client.completion_model(config.model()),
            ))
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

async fn bedrock_client(profile: &str) -> BedrockClient {
    let mut loader =
        aws_config::defaults(aws_config::BehaviorVersion::latest()).profile_name(profile);
    if let Some(region) = bedrock_region() {
        loader = loader.region(aws_config::Region::new(region));
    }
    aws_sdk_bedrockruntime::Client::new(&loader.load().await).into()
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
    let image = Image {
        data: DocumentSourceKind::Raw(jpeg),
        media_type: Some(ImageMediaType::JPEG),
        detail: None,
        additional_params: None,
    };
    let request = model
        .completion_request("Describe this lab video frame for search retrieval.")
        .preamble(DESCRIPTION_PROMPT.to_owned())
        .message(Message::User {
            content: vec![
                UserContent::Image(image),
                UserContent::Text("Return only the description text.".to_owned().into()),
            ],
        })
        .temperature(0.0)
        .build();
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

fn map_completion_error(error: &rig::completion::CompletionError) -> SemanticIndexingFailure {
    SemanticIndexingFailure::ProviderUnavailable {
        message: error.to_string(),
    }
}
