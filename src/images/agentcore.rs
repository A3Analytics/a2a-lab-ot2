//! `AgentCore` Runtime executor for source-backed image decisions.
//!
//! The lab file names the runtime. The model stays pinned in that deployment.

use std::time::Duration;

use a2a_lab_dev_kit::{
    A2aLabError, AgentCoreImageDecisionSettings, Image, ImageDecisionExecutor,
    ImageDecisionHttpLimits, ImageDecisionPrimitiveAnswer, ImageDecisionProviderError,
    ImageDecisionQuestions, ImageDecisionRequest, ImageDecisionResult, ImageTransportConfig,
    SourceImageDecisionResult,
};
use async_trait::async_trait;
use aws_config::BehaviorVersion;
use aws_sdk_bedrockagentcore::primitives::Blob;
use base64::Engine;
use serde::Deserialize;
use serde::Serialize;

const FIELD_MESSAGE: &str = "agentcore image decision failed";

/// Sends one JSON payload to an `AgentCore` runtime.
#[async_trait]
pub trait AgentCoreInvoker: Send + Sync {
    /// Returns the aggregated runtime response body.
    async fn invoke(&self, payload: &[u8]) -> Result<Vec<u8>, ImageDecisionProviderError>;
}

/// `SigV4` client for `InvokeAgentRuntime`.
pub struct SdkAgentCoreInvoker {
    client: aws_sdk_bedrockagentcore::Client,
    runtime_arn: String,
    qualifier: String,
    session_id: String,
    timeout: Duration,
}

impl SdkAgentCoreInvoker {
    /// Loads the default AWS credential chain in `region`.
    pub async fn connect(
        settings: &AgentCoreImageDecisionSettings,
        timeout: Duration,
    ) -> Result<Self, A2aLabError> {
        let mut loader = aws_config::defaults(BehaviorVersion::latest())
            .region(aws_config::Region::new(settings.region().to_owned()));
        if let Some(profile) = std::env::var("AWS_PROFILE")
            .ok()
            .map(|value| value.trim().to_owned())
            .filter(|value| !value.is_empty())
        {
            loader = loader.profile_name(profile);
        }
        let config = loader.load().await;
        Ok(Self {
            client: aws_sdk_bedrockagentcore::Client::new(&config),
            runtime_arn: settings.runtime_arn().to_owned(),
            qualifier: settings.qualifier().to_owned(),
            session_id: session_id(),
            timeout,
        })
    }
}

#[async_trait]
impl AgentCoreInvoker for SdkAgentCoreInvoker {
    async fn invoke(&self, payload: &[u8]) -> Result<Vec<u8>, ImageDecisionProviderError> {
        let invocation = self
            .client
            .invoke_agent_runtime()
            .agent_runtime_arn(self.runtime_arn.clone())
            .qualifier(self.qualifier.clone())
            .runtime_session_id(self.session_id.clone())
            .content_type("application/json")
            .accept("application/json")
            .payload(Blob::new(payload.to_vec()));
        let output = tokio::time::timeout(self.timeout, invocation.send())
            .await
            .map_err(|_| deadline("agentcore decision timed out"))?
            .map_err(|error| unavailable(&error.to_string()))?;
        let content_type = output.content_type();
        if content_type != "application/json" && !content_type.starts_with("application/json;") {
            return Err(invalid_answer("agentcore response was not json"));
        }
        let bytes = output
            .response
            .collect()
            .await
            .map_err(|error| unavailable(&error.to_string()))?
            .into_bytes();
        Ok(bytes.to_vec())
    }
}

/// Source-question executor backed by one `AgentCore` runtime.
pub struct AgentCoreImageDecisionExecutor<I> {
    invoker: I,
    limits: ImageDecisionHttpLimits,
}

impl<I> AgentCoreImageDecisionExecutor<I> {
    /// Builds an executor around an already connected invoker.
    pub fn new(invoker: I, limits: ImageDecisionHttpLimits) -> Self {
        Self { invoker, limits }
    }
}

impl AgentCoreImageDecisionExecutor<SdkAgentCoreInvoker> {
    /// Connects the AWS SDK invoker using lab-file runtime settings and bounds.
    pub async fn connect(
        settings: &AgentCoreImageDecisionSettings,
        timeout: Duration,
        max_image_bytes: u64,
        max_request_bytes: u64,
        max_response_bytes: u64,
    ) -> Result<Self, A2aLabError> {
        let limits = ImageDecisionHttpLimits::new(
            ImageTransportConfig::new(max_image_bytes)?,
            usize::try_from(max_request_bytes).unwrap_or(usize::MAX),
            usize::try_from(max_response_bytes).unwrap_or(usize::MAX),
            timeout,
        )?;
        let invoker = SdkAgentCoreInvoker::connect(settings, timeout).await?;
        Ok(Self::new(invoker, limits))
    }
}

#[async_trait]
impl<I> ImageDecisionExecutor for AgentCoreImageDecisionExecutor<I>
where
    I: AgentCoreInvoker,
{
    async fn decide(
        &self,
        _request: &ImageDecisionRequest,
    ) -> Result<ImageDecisionResult, ImageDecisionProviderError> {
        Err(unavailable(
            "agentcore image decisions accept source questions",
        ))
    }

    async fn decide_questions(
        &self,
        image: &Image,
        questions: &ImageDecisionQuestions,
    ) -> Result<SourceImageDecisionResult, ImageDecisionProviderError> {
        self.limits
            .image_transport()
            .check_payload(image.data())
            .map_err(|_| limit_exceeded("image", "image exceeds the configured maximum"))?;
        if !supported_media_type(image.descriptor().media_type()) {
            return Err(invalid_answer("image media type is not supported"));
        }
        let encoded = base64::engine::general_purpose::STANDARD.encode(image.data());
        let body = RuntimeRequest {
            state: "",
            images: vec![encoded],
            questions,
        };
        let payload = serde_json::to_vec(&body)
            .map_err(|_| invalid_answer("agentcore request json is invalid"))?;
        if payload.len() > self.limits.max_request_bytes() {
            return Err(limit_exceeded(
                "request",
                "request exceeds the configured maximum",
            ));
        }
        let response = self.invoker.invoke(&payload).await?;
        if response.len() > self.limits.max_response_bytes() {
            return Err(limit_exceeded(
                "response",
                "response exceeds the configured maximum",
            ));
        }
        let parsed: RuntimeResponse = serde_json::from_slice(&response)
            .map_err(|_| invalid_answer("agentcore response json is invalid"))?;
        SourceImageDecisionResult::from_questions(questions, parsed.into_answers()?)
    }
}

#[derive(Serialize)]
struct RuntimeRequest<'a> {
    state: &'a str,
    images: Vec<String>,
    questions: &'a ImageDecisionQuestions,
}

#[derive(Deserialize)]
struct RuntimeResponse {
    answers: std::collections::BTreeMap<String, RuntimeAnswer>,
}

#[derive(Deserialize)]
#[serde(untagged)]
enum RuntimeAnswer {
    Primitive(ImageDecisionPrimitiveAnswer),
    ScoreIndexed(RuntimeIndexedScoreAnswer),
}

#[derive(Deserialize)]
struct RuntimeIndexedScoreAnswer {
    score: f32,
    legend: std::collections::BTreeMap<String, String>,
    probabilities: std::collections::BTreeMap<String, f32>,
    confidence: f32,
}

impl RuntimeResponse {
    fn into_answers(
        self,
    ) -> Result<
        std::collections::BTreeMap<String, ImageDecisionPrimitiveAnswer>,
        ImageDecisionProviderError,
    > {
        self.answers
            .into_iter()
            .map(|(id, answer)| Ok((id, answer.into_primitive()?)))
            .collect()
    }
}

impl RuntimeAnswer {
    fn into_primitive(self) -> Result<ImageDecisionPrimitiveAnswer, ImageDecisionProviderError> {
        match self {
            Self::Primitive(answer) => Ok(answer),
            Self::ScoreIndexed(indexed) => {
                if indexed.legend.len() != indexed.probabilities.len() {
                    return Err(invalid_answer(
                        "agentcore score answer indices do not match",
                    ));
                }
                let mut legend = Vec::with_capacity(indexed.legend.len());
                let mut probabilities = Vec::with_capacity(indexed.probabilities.len());
                for index in 0..indexed.legend.len() {
                    let key = index.to_string();
                    let Some(level) = indexed.legend.get(&key) else {
                        return Err(invalid_answer("agentcore score legend indices are invalid"));
                    };
                    let Some(probability) = indexed.probabilities.get(&key) else {
                        return Err(invalid_answer(
                            "agentcore score probability indices are invalid",
                        ));
                    };
                    legend.push(level.clone());
                    probabilities.push(*probability);
                }
                Ok(ImageDecisionPrimitiveAnswer::Score {
                    score: indexed.score,
                    legend,
                    probabilities,
                    confidence: indexed.confidence,
                })
            }
        }
    }
}

fn supported_media_type(media_type: &str) -> bool {
    matches!(
        media_type,
        "image/jpeg" | "image/png" | "image/webp" | "image/gif"
    )
}

fn session_id() -> String {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |duration| duration.as_nanos());
    format!("a2alab{nanos:032x}{:08x}", std::process::id())
}

fn unavailable(message: &str) -> ImageDecisionProviderError {
    ImageDecisionProviderError::unavailable(message).unwrap_or(
        ImageDecisionProviderError::Unavailable {
            message: FIELD_MESSAGE.to_owned(),
        },
    )
}

fn invalid_answer(message: &str) -> ImageDecisionProviderError {
    ImageDecisionProviderError::invalid_answer(message).unwrap_or(
        ImageDecisionProviderError::InvalidAnswer {
            message: FIELD_MESSAGE.to_owned(),
        },
    )
}

fn limit_exceeded(limit: &str, message: &str) -> ImageDecisionProviderError {
    ImageDecisionProviderError::limit_exceeded(limit, message).unwrap_or(
        ImageDecisionProviderError::LimitExceeded {
            limit: limit.to_owned(),
            message: FIELD_MESSAGE.to_owned(),
        },
    )
}

fn deadline(message: &str) -> ImageDecisionProviderError {
    ImageDecisionProviderError::deadline_exceeded(message).unwrap_or(
        ImageDecisionProviderError::DeadlineExceeded {
            message: FIELD_MESSAGE.to_owned(),
        },
    )
}

#[cfg(test)]
mod tests {
    use super::session_id;

    #[test]
    fn session_id_is_long_enough_for_agentcore() {
        let id = session_id();
        assert!((33..=256).contains(&id.len()));
        assert!(
            id.chars()
                .all(|character| character.is_ascii_alphanumeric())
        );
    }
}
