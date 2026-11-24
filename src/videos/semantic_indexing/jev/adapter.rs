//! Live typed evaluator that loads full-frame stills and evaluates them through
//! the devkit [`TypedLabPredicateJevAdapter`].
//!
//! The adapter is the only production HTTP path. It posts named questions through
//! [`TypeSafeSystemOneClient`]. Omitted model ROI is the full source image.
//! This module does not append a typed ledger record and does not call Luna or
//! chat.

use std::collections::HashMap;
use std::sync::Arc;

use a2a_lab_dev_kit::{
    CredentialReference, FrameDecisionEvaluationFailure, FrameDecisionEvaluationRequest,
    FrameDecisionEvaluator, FrameDecisionEvaluatorLimits, FrameDecisionEvaluatorProviderConfig,
    FrameRoiTransform, Image, ImageDescriptor, ImageId, ImageSourceId, ImageTransportConfig,
    JsonObject, KeyedFrameDecisionEvaluation, SavedFrameId, TypeSafeSystemOneClient,
    TypedLabPredicateJevAdapter,
};

use super::media::JevDecisionMedia;

const DEFAULT_JEV_DECISION_URL: &str = "https://api.typesafe.ai/v1/systemone";

/// Whether live Jev calls are permitted (`A2ALAB_JEV_LIVE=1`).
#[must_use]
pub fn live_jev_allowed() -> bool {
    std::env::var("A2ALAB_JEV_LIVE").is_ok_and(|value| matches!(value.trim(), "1" | "true" | "yes"))
}

/// Opt-in smoke/benchmark gate (`A2ALAB_JEV_SMOKE=1`).
#[must_use]
pub fn jev_smoke_enabled() -> bool {
    std::env::var("A2ALAB_JEV_SMOKE")
        .is_ok_and(|value| matches!(value.trim(), "1" | "true" | "yes"))
}

/// Overrideable API base for tests (`A2ALAB_JEV_API_BASE`).
#[must_use]
pub fn jev_api_base() -> String {
    std::env::var("A2ALAB_JEV_API_BASE").unwrap_or_else(|_| DEFAULT_JEV_DECISION_URL.to_owned())
}

/// Live typed evaluator backed by the devkit compatibility adapter.
pub struct TypeSafeJevFrameDecisionEvaluator<M: JevDecisionMedia> {
    config: FrameDecisionEvaluatorProviderConfig,
    limits: FrameDecisionEvaluatorLimits,
    client: TypeSafeSystemOneClient,
    media: Arc<M>,
}

impl<M: JevDecisionMedia + 'static> TypeSafeJevFrameDecisionEvaluator<M> {
    /// Authorized live adapter. Requires `A2ALAB_JEV_LIVE=1` and a configured credential.
    pub fn connect_live(
        config: FrameDecisionEvaluatorProviderConfig,
        limits: FrameDecisionEvaluatorLimits,
        media: Arc<M>,
        credentials: Option<&CredentialReference>,
    ) -> Result<Self, FrameDecisionEvaluationFailure> {
        if !live_jev_allowed() {
            return Err(FrameDecisionEvaluationFailure::ProviderUnavailable {
                message: "live Jev calls require A2ALAB_JEV_LIVE=1".to_owned(),
            });
        }
        let Some(api_key) = configured_api_key(credentials) else {
            return Err(FrameDecisionEvaluationFailure::ProviderUnavailable {
                message: "TypeSafe API credentials are not configured".to_owned(),
            });
        };
        let client = decision_client(jev_api_base(), &api_key, limits)?;
        Ok(Self::new(config, limits, media, client))
    }

    /// Mock endpoint without network credentials.
    pub fn mock(
        config: FrameDecisionEvaluatorProviderConfig,
        limits: FrameDecisionEvaluatorLimits,
        media: Arc<M>,
        endpoint: impl Into<String>,
    ) -> Result<Self, FrameDecisionEvaluationFailure> {
        let client = decision_client(endpoint.into(), "test-key", limits)?;
        Ok(Self::new(config, limits, media, client))
    }

    fn new(
        config: FrameDecisionEvaluatorProviderConfig,
        limits: FrameDecisionEvaluatorLimits,
        media: Arc<M>,
        client: TypeSafeSystemOneClient,
    ) -> Self {
        Self {
            config,
            limits,
            client,
            media,
        }
    }

    /// Stores the exact semantic decision preamble. Blank text is kept and does not change questions.
    #[must_use]
    pub fn with_preamble(mut self, preamble: Option<String>) -> Self {
        self.client = self.client.with_preamble(preamble);
        self
    }

    async fn bind_frames(
        &self,
        requests: &[FrameDecisionEvaluationRequest],
    ) -> Result<HashMap<SavedFrameId, Image>, FrameDecisionEvaluationFailure> {
        let mut frames = HashMap::new();
        for request in requests {
            let frame_id = request.target().frame_id().clone();
            if frames.contains_key(&frame_id) {
                continue;
            }
            let bytes = self
                .media
                .frame_still(request.target(), FrameRoiTransform::full_frame())
                .await?;
            frames.insert(frame_id, still_image(request, &bytes, self.limits)?);
        }
        Ok(frames)
    }
}

impl<M: JevDecisionMedia + Send + Sync + 'static> FrameDecisionEvaluator
    for TypeSafeJevFrameDecisionEvaluator<M>
{
    fn config(&self) -> &FrameDecisionEvaluatorProviderConfig {
        &self.config
    }

    fn limits(&self) -> FrameDecisionEvaluatorLimits {
        self.limits
    }

    async fn evaluate_frames(
        &self,
        requests: Vec<FrameDecisionEvaluationRequest>,
    ) -> Result<Vec<KeyedFrameDecisionEvaluation>, FrameDecisionEvaluationFailure> {
        if requests.is_empty() {
            return Ok(Vec::new());
        }
        let frames = self.bind_frames(&requests).await?;
        let evaluated_at = crate::time::now().map_err(|error| {
            FrameDecisionEvaluationFailure::InvalidProvenance {
                message: error.to_string(),
            }
        })?;
        let adapter = TypedLabPredicateJevAdapter::new(
            self.config.clone(),
            self.limits,
            self.client.clone(),
            evaluated_at,
        )
        .map_err(|error| invalid_evidence(&error))?
        .with_frames(frames);
        adapter.evaluate_frames(requests).await
    }
}

fn configured_api_key(credentials: Option<&CredentialReference>) -> Option<String> {
    let referenced = credentials.and_then(|reference| nonempty_env(reference.env_var()));
    referenced.or_else(|| nonempty_env("TYPESAFE_API_KEY"))
}

fn nonempty_env(name: &str) -> Option<String> {
    std::env::var(name).ok().and_then(|value| {
        let value = value.trim();
        if value.is_empty() {
            None
        } else {
            Some(value.to_owned())
        }
    })
}

fn decision_client(
    endpoint: String,
    api_key: &str,
    limits: FrameDecisionEvaluatorLimits,
) -> Result<TypeSafeSystemOneClient, FrameDecisionEvaluationFailure> {
    let http_limits = TypedLabPredicateJevAdapter::http_limits(limits).map_err(|error| {
        FrameDecisionEvaluationFailure::ProviderUnavailable {
            message: error.to_string(),
        }
    })?;
    TypeSafeSystemOneClient::new(endpoint, api_key, http_limits).map_err(|error| {
        FrameDecisionEvaluationFailure::ProviderUnavailable {
            message: error.to_string(),
        }
    })
}

fn still_image(
    request: &FrameDecisionEvaluationRequest,
    bytes: &[u8],
    limits: FrameDecisionEvaluatorLimits,
) -> Result<Image, FrameDecisionEvaluationFailure> {
    let decoded = image::load_from_memory(bytes).map_err(|error| {
        FrameDecisionEvaluationFailure::InvalidEvidence {
            message: format!("saved frame still: {error}"),
        }
    })?;
    let target = request.target();
    let descriptor = ImageDescriptor::new(
        ImageId::new(target.frame_id().as_str()).map_err(|error| invalid_evidence(&error))?,
        ImageSourceId::new(target.source_id().as_str())
            .map_err(|error| invalid_evidence(&error))?,
        target.captured_at(),
        still_media_type(bytes),
        decoded.width(),
        decoded.height(),
        None,
        JsonObject::empty(),
    )
    .map_err(|error| invalid_evidence(&error))?;
    let max_image_bytes = u64::try_from(limits.max_image_bytes()).unwrap_or(u64::MAX);
    let transport =
        ImageTransportConfig::new(max_image_bytes).map_err(|error| invalid_evidence(&error))?;
    Image::with_transport(descriptor, bytes.to_vec(), transport)
        .map_err(|error| invalid_evidence(&error))
}

fn still_media_type(bytes: &[u8]) -> &'static str {
    if bytes.starts_with(&[0x89, b'P', b'N', b'G']) {
        "image/png"
    } else {
        "image/jpeg"
    }
}

fn invalid_evidence(error: &impl ToString) -> FrameDecisionEvaluationFailure {
    FrameDecisionEvaluationFailure::InvalidEvidence {
        message: error.to_string(),
    }
}
