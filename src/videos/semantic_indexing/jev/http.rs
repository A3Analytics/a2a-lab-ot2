//! Reqwest client for `TypeSafe` System One with bounded retry-after.

use std::time::{Duration, Instant};

use a2a_lab_dev_kit::{
    CredentialReference, DEFAULT_JEV_MODEL_REVISION, FrameDecisionEvaluationFailure, Sha256Digest,
};
use base64::Engine;
use reqwest::StatusCode;
use reqwest::header::{HeaderMap, HeaderValue, RETRY_AFTER};
use serde_json::to_vec;

use super::limits::{ValidatedJevImage, validate_request_bytes};
use super::wire::{PINNED_JEV_MODEL, SystemOneRequest, SystemOneResponse};

pub const DEFAULT_TYPESAFE_URL: &str = "https://api.typesafe.ai/v1/systemone";
pub const DANGER_ZONE_HEADER: &str = "TypeSafe-Danger-Zone-Images";

/// HTTP transport for pinned Jev System One calls.
#[derive(Clone)]
pub struct JevHttpClient {
    client: reqwest::Client,
    endpoint: String,
    api_key: Option<String>,
}

impl JevHttpClient {
    /// Builds a client; live calls require `connect_live`.
    pub fn new(endpoint: impl Into<String>) -> Self {
        Self {
            client: reqwest::Client::new(),
            endpoint: endpoint.into(),
            api_key: None,
        }
    }

    /// Wires Bearer auth from a configured secret reference (never logs the value).
    pub fn connect_live(
        endpoint: impl Into<String>,
        credentials: Option<&CredentialReference>,
    ) -> Result<Self, FrameDecisionEvaluationFailure> {
        let api_key = credentials.and_then(|reference| {
            std::env::var(reference.env_var()).ok().and_then(|value| {
                let trimmed = value.trim();
                if trimmed.is_empty() {
                    None
                } else {
                    Some(trimmed.to_owned())
                }
            })
        });
        if api_key.is_none() {
            return Err(FrameDecisionEvaluationFailure::ProviderUnavailable {
                message: "TypeSafe API credentials are not configured".to_owned(),
            });
        }
        Ok(Self {
            client: reqwest::Client::builder()
                .timeout(Duration::from_secs(120))
                .build()
                .map_err(
                    |error| FrameDecisionEvaluationFailure::ProviderUnavailable {
                        message: format!("reqwest client: {error}"),
                    },
                )?,
            endpoint: endpoint.into(),
            api_key,
        })
    }

    /// Deterministic mock transport without credentials.
    #[must_use]
    pub fn mock(endpoint: impl Into<String>) -> Self {
        Self::new(endpoint)
    }

    #[must_use]
    pub fn endpoint(&self) -> &str {
        &self.endpoint
    }

    /// POST one validated System One request; retries 429 using Retry-After once.
    pub async fn post_system_one(
        &self,
        request: &SystemOneRequest,
        deadline: Duration,
    ) -> Result<(SystemOneResponse, Sha256Digest, Duration), FrameDecisionEvaluationFailure> {
        let started = Instant::now();
        let body = encode_request(request)?;
        validate_request_bytes(body.len())?;
        let mut headers = auth_headers(self.api_key.as_deref())?;
        headers.insert(DANGER_ZONE_HEADER, HeaderValue::from_static("true"));
        let response = self
            .execute_with_retry(&headers, &body, deadline, started)
            .await?;
        let latency = started.elapsed();
        let digest = Sha256Digest::hash_bytes(&response.raw);
        let parsed: SystemOneResponse = serde_json::from_slice(&response.raw).map_err(|error| {
            FrameDecisionEvaluationFailure::InvalidProvenance {
                message: format!("jev response json: {error}"),
            }
        })?;
        if parsed.model != DEFAULT_JEV_MODEL_REVISION && parsed.model != PINNED_JEV_MODEL {
            return Err(FrameDecisionEvaluationFailure::InvalidProvenance {
                message: format!(
                    "resolved model {} does not match pinned {}",
                    parsed.model, PINNED_JEV_MODEL
                ),
            });
        }
        Ok((parsed, digest, latency))
    }

    async fn execute_with_retry(
        &self,
        headers: &HeaderMap,
        body: &[u8],
        deadline: Duration,
        started: Instant,
    ) -> Result<RawHttpBody, FrameDecisionEvaluationFailure> {
        let first = self.post_once(headers, body).await?;
        if first.status != StatusCode::TOO_MANY_REQUESTS {
            return map_http_status(first);
        }
        let retry_after = first
            .retry_after
            .unwrap_or(Duration::from_secs(1))
            .min(Duration::from_secs(30));
        if started.elapsed().saturating_add(retry_after) >= deadline {
            return Err(FrameDecisionEvaluationFailure::ProviderUnavailable {
                message: "429 retry-after exceeds evaluator deadline".to_owned(),
            });
        }
        tokio::time::sleep(retry_after).await;
        let second = self.post_once(headers, body).await?;
        map_http_status(second)
    }

    async fn post_once(
        &self,
        headers: &HeaderMap,
        body: &[u8],
    ) -> Result<HttpAttempt, FrameDecisionEvaluationFailure> {
        let response = self
            .client
            .post(&self.endpoint)
            .headers(headers.clone())
            .header(reqwest::header::CONTENT_TYPE, "application/json")
            .body(body.to_vec())
            .send()
            .await
            .map_err(|error| map_transport_error(&error))?;
        let status = response.status();
        let retry_after = response
            .headers()
            .get(RETRY_AFTER)
            .and_then(|value| value.to_str().ok())
            .and_then(parse_retry_after);
        let raw = response
            .bytes()
            .await
            .map_err(|error| map_transport_error(&error))?;
        Ok(HttpAttempt {
            status,
            raw: raw.to_vec(),
            retry_after,
        })
    }
}

struct HttpAttempt {
    status: StatusCode,
    raw: Vec<u8>,
    retry_after: Option<Duration>,
}

struct RawHttpBody {
    raw: Vec<u8>,
}

fn map_http_status(attempt: HttpAttempt) -> Result<RawHttpBody, FrameDecisionEvaluationFailure> {
    match attempt.status {
        StatusCode::OK => Ok(RawHttpBody { raw: attempt.raw }),
        StatusCode::REQUEST_TIMEOUT | StatusCode::GATEWAY_TIMEOUT => {
            Err(FrameDecisionEvaluationFailure::DeadlineExceeded)
        }
        StatusCode::TOO_MANY_REQUESTS => Err(FrameDecisionEvaluationFailure::ProviderUnavailable {
            message: "rate limited after retry".to_owned(),
        }),
        status if status.is_client_error() => {
            Err(FrameDecisionEvaluationFailure::InvalidEvidence {
                message: format!(
                    "jev rejected request: {status} {}",
                    String::from_utf8_lossy(&attempt.raw)
                ),
            })
        }
        _ => Err(FrameDecisionEvaluationFailure::ProviderUnavailable {
            message: format!("jev http {}", attempt.status),
        }),
    }
}

fn map_transport_error(error: &reqwest::Error) -> FrameDecisionEvaluationFailure {
    if error.is_timeout() {
        FrameDecisionEvaluationFailure::DeadlineExceeded
    } else {
        FrameDecisionEvaluationFailure::ProviderUnavailable {
            message: error.to_string(),
        }
    }
}

fn auth_headers(api_key: Option<&str>) -> Result<HeaderMap, FrameDecisionEvaluationFailure> {
    let mut headers = HeaderMap::new();
    let Some(key) = api_key else {
        return Ok(headers);
    };
    headers.insert(
        reqwest::header::AUTHORIZATION,
        HeaderValue::from_str(&format!("Bearer {key}")).map_err(|error| {
            FrameDecisionEvaluationFailure::ProviderUnavailable {
                message: format!("authorization header: {error}"),
            }
        })?,
    );
    Ok(headers)
}

fn encode_request(request: &SystemOneRequest) -> Result<Vec<u8>, FrameDecisionEvaluationFailure> {
    to_vec(request).map_err(|error| FrameDecisionEvaluationFailure::InvalidEvidence {
        message: format!("encode systemone request: {error}"),
    })
}

type AssetBundle = (
    serde_json::Map<String, serde_json::Value>,
    std::collections::BTreeMap<String, super::wire::WireAsset>,
);

/// Builds JSON `assets` from unique validated images (deduped by digest).
pub fn attach_image_assets(
    images: &[ValidatedJevImage],
) -> Result<AssetBundle, FrameDecisionEvaluationFailure> {
    use super::wire::WireAsset;
    use serde_json::json;
    let engine = base64::engine::general_purpose::STANDARD;
    let mut assets = std::collections::BTreeMap::new();
    for image in images {
        if assets.contains_key(image.asset_ref()) {
            continue;
        }
        assets.insert(
            image.asset_ref().to_owned(),
            WireAsset {
                kind: "image",
                format: image.format().to_string(),
                data_base64: engine.encode(image.bytes()),
            },
        );
    }
    let declared = assets
        .values()
        .map(|asset| asset.data_base64.len())
        .sum::<usize>();
    validate_request_bytes(declared)?;
    let mut state = serde_json::Map::new();
    state.insert(
        "frame".to_owned(),
        json!(
            images
                .first()
                .map(|image| image.asset_ref().to_owned())
                .unwrap_or_default()
        ),
    );
    Ok((state, assets))
}

fn parse_retry_after(value: &str) -> Option<Duration> {
    if let Ok(seconds) = value.trim().parse::<u64>() {
        return Some(Duration::from_secs(seconds));
    }
    None
}
