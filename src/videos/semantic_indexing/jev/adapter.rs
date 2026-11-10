//! `TypeSafe` Jev evaluator implementing the dev-kit contract.

use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;
use std::time::Duration;

use a2a_lab_dev_kit::{
    CredentialReference, DecisionEvaluationCoverage, DecisionThresholdPolicy,
    FrameDecisionEvaluationFailure, FrameDecisionEvaluationRequest, FrameDecisionEvaluator,
    FrameDecisionEvaluatorLimits, FrameDecisionEvaluatorProviderConfig, FrameRoiTransform,
    JevEvaluationProvenance, JevStructuredOutput, JsonObject, KeyedFrameDecisionEvaluation,
    LabPredicate, LabProbabilityVector, OperatorReferenceImage, Sha256Digest, UtcTimestamp,
    interpret_structured_outcome, unsupported_predicate_result, validate_evaluation_batch,
};

use super::http::{DEFAULT_TYPESAFE_URL, JevHttpClient, attach_image_assets};
use super::limits::{ValidatedJevImage, validate_jev_image};
use super::media::JevDecisionMedia;
use super::questions::{self, QBOX, QCHOICE, QNOUL, QSCORE};
use super::wire::{PINNED_JEV_MODEL, SystemOneRequest, WireAnswer, WireQuestion};

/// Whether live `TypeSafe` calls are permitted (`A2ALAB_JEV_LIVE=1`).
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
    std::env::var("A2ALAB_JEV_API_BASE").unwrap_or_else(|_| DEFAULT_TYPESAFE_URL.to_owned())
}

/// Live or mock `TypeSafe` Jev HTTP evaluator.
pub struct TypeSafeJevFrameDecisionEvaluator<M: JevDecisionMedia> {
    config: FrameDecisionEvaluatorProviderConfig,
    limits: FrameDecisionEvaluatorLimits,
    http: JevHttpClient,
    media: Arc<M>,
    threshold: DecisionThresholdPolicy,
}

impl<M: JevDecisionMedia + 'static> TypeSafeJevFrameDecisionEvaluator<M> {
    /// Authorized live adapter; requires credentials and `A2ALAB_JEV_LIVE=1`.
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
        let http = JevHttpClient::connect_live(jev_api_base(), credentials)?;
        Ok(Self::new(config, limits, media, http))
    }

    /// Mock endpoint without network credentials (tests and fixtures).
    #[must_use]
    pub fn mock(
        config: FrameDecisionEvaluatorProviderConfig,
        limits: FrameDecisionEvaluatorLimits,
        media: Arc<M>,
        endpoint: impl Into<String>,
    ) -> Self {
        Self::new(config, limits, media, JevHttpClient::mock(endpoint))
    }

    fn new(
        config: FrameDecisionEvaluatorProviderConfig,
        limits: FrameDecisionEvaluatorLimits,
        media: Arc<M>,
        http: JevHttpClient,
    ) -> Self {
        Self {
            config,
            limits,
            media,
            http,
            threshold: DecisionThresholdPolicy::pilot(),
        }
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
        if requests.len() > usize::try_from(self.limits.max_batch_items()).unwrap_or(usize::MAX) {
            return Err(FrameDecisionEvaluationFailure::LimitExceeded {
                limit: "max_batch_items".to_owned(),
                message: format!(
                    "batch size {} exceeds {}",
                    requests.len(),
                    self.limits.max_batch_items()
                ),
            });
        }
        let mut by_frame: HashMap<String, Vec<usize>> = HashMap::new();
        for (index, request) in requests.iter().enumerate() {
            by_frame
                .entry(request.target().frame_id().as_str().to_owned())
                .or_default()
                .push(index);
        }
        let mut slots = vec![None; requests.len()];
        for indices in by_frame.values() {
            let batch: Vec<FrameDecisionEvaluationRequest> = indices
                .iter()
                .map(|index| requests[*index].clone())
                .collect();
            let outcomes = self.evaluate_frame_batch(&batch).await?;
            for (offset, index) in indices.iter().enumerate() {
                slots[*index] = Some(outcomes[offset].clone());
            }
        }
        let responses: Vec<KeyedFrameDecisionEvaluation> = slots
            .into_iter()
            .map(|slot| {
                slot.ok_or_else(|| FrameDecisionEvaluationFailure::PartialResponse {
                    message: "missing keyed response slot".to_owned(),
                })
            })
            .collect::<Result<_, _>>()?;
        validate_evaluation_batch(&requests, &responses)
    }
}

impl<M: JevDecisionMedia + Send + Sync + 'static> TypeSafeJevFrameDecisionEvaluator<M> {
    async fn evaluate_frame_batch(
        &self,
        requests: &[FrameDecisionEvaluationRequest],
    ) -> Result<Vec<KeyedFrameDecisionEvaluation>, FrameDecisionEvaluationFailure> {
        let mut supported = Vec::new();
        for request in requests {
            if self.config.supports_predicate(request.predicate()) {
                supported.push(request.clone());
            }
        }
        let mut results = Vec::new();
        if supported.is_empty() {
            for request in requests {
                results.push(unsupported_predicate_result(request, request.predicate()));
            }
            return Ok(results);
        }
        let first = supported.first().expect("non-empty supported batch");
        let frame_bytes = self
            .media
            .frame_still(first.target(), first.model_input_transform())
            .await?;
        let frame_image = validate_jev_image(&frame_bytes)?;
        let mut unique_images: HashMap<String, ValidatedJevImage> = HashMap::new();
        unique_images.insert(frame_image.asset_ref().to_owned(), frame_image);
        for request in &supported {
            for reference in request.criteria().references() {
                load_reference(self.media.as_ref(), reference, &mut unique_images).await?;
            }
        }
        let image_list: Vec<ValidatedJevImage> = unique_images.into_values().collect();
        let (state, assets_map) = attach_image_assets(&image_list)?;
        let mut questions: BTreeMap<String, WireQuestion> = BTreeMap::new();
        for request in &supported {
            for (id, question) in questions::predicate_questions(request, "frame")? {
                questions.insert(id, question);
            }
        }
        let wire = SystemOneRequest {
            model: PINNED_JEV_MODEL,
            state,
            assets: assets_map,
            questions,
        };
        let (response, digest, latency) = self
            .http
            .post_system_one(&wire, self.limits.deadline())
            .await?;
        let evaluated_at = crate::time::now().map_err(|error| {
            FrameDecisionEvaluationFailure::InvalidProvenance {
                message: error.to_string(),
            }
        })?;
        for request in requests {
            if !self.config.supports_predicate(request.predicate()) {
                results.push(unsupported_predicate_result(request, request.predicate()));
                continue;
            }
            results.push(map_keyed_result(
                request,
                &response,
                &digest,
                latency,
                evaluated_at,
                &self.config,
                self.threshold,
            )?);
        }
        Ok(results)
    }
}

async fn load_reference<M: JevDecisionMedia>(
    media: &M,
    reference: &OperatorReferenceImage,
    unique: &mut HashMap<String, ValidatedJevImage>,
) -> Result<(), FrameDecisionEvaluationFailure> {
    let digest_key = reference
        .digest()
        .as_str()
        .chars()
        .take(16)
        .collect::<String>();
    if unique.contains_key(&digest_key) {
        return Ok(());
    }
    let bytes = media.reference_still(reference.digest()).await?;
    let image = validate_jev_image(&bytes)?;
    unique.insert(image.asset_ref().to_owned(), image);
    Ok(())
}

fn map_box_to_saved_frame(
    box_bounds: [f32; 4],
    model_input: FrameRoiTransform,
) -> Result<FrameRoiTransform, FrameDecisionEvaluationFailure> {
    let [cx0, cy0, cx1, cy1] = model_input.bounds();
    let [bx0, by0, bx1, by1] = box_bounds;
    let width = cx1 - cx0;
    let height = cy1 - cy0;
    FrameRoiTransform::new([
        cx0 + bx0 * width,
        cy0 + by0 * height,
        cx0 + bx1 * width,
        cy0 + by1 * height,
    ])
    .map_err(|error| FrameDecisionEvaluationFailure::InvalidProvenance {
        message: error.to_string(),
    })
}

fn map_keyed_result(
    request: &FrameDecisionEvaluationRequest,
    response: &super::wire::SystemOneResponse,
    digest: &Sha256Digest,
    latency: Duration,
    evaluated_at: UtcTimestamp,
    config: &FrameDecisionEvaluatorProviderConfig,
    threshold: DecisionThresholdPolicy,
) -> Result<KeyedFrameDecisionEvaluation, FrameDecisionEvaluationFailure> {
    let eval = request.evaluation_id().as_str();
    let structured = map_structured_output(
        request.predicate(),
        eval,
        response,
        latency,
        request.model_input_transform(),
    )?;
    let outcome = interpret_structured_outcome(&structured, threshold)?;
    let provenance = JevEvaluationProvenance::new(
        request.target().captured_at(),
        evaluated_at,
        config.producer().clone(),
        request.evaluation_id().clone(),
        config.config_digest().clone(),
        request.criteria().prompt_digest().clone(),
        request.criteria().criteria_digest().clone(),
        config.threshold_policy_digest().clone(),
        request.model_input_transform(),
        digest.clone(),
        DecisionEvaluationCoverage::Evaluated,
        Option::<&str>::None,
    )
    .map_err(|error| FrameDecisionEvaluationFailure::InvalidProvenance {
        message: error.to_string(),
    })?;
    KeyedFrameDecisionEvaluation::new(
        request,
        DecisionEvaluationCoverage::Evaluated,
        Some(outcome),
        Some(structured),
        Some(provenance),
        Some(digest.clone()),
    )
}

fn map_structured_output(
    predicate: &LabPredicate,
    evaluation_id: &str,
    response: &super::wire::SystemOneResponse,
    latency: Duration,
    model_input: FrameRoiTransform,
) -> Result<JevStructuredOutput, FrameDecisionEvaluationFailure> {
    let noul = answer_noul(response, &questions::question_id(evaluation_id, QNOUL));
    let box_roi = answer_box(
        response,
        &questions::question_id(evaluation_id, QBOX),
        model_input,
    )?;
    let (option_distribution, vendor_confidence) =
        answer_choice(response, &questions::question_id(evaluation_id, QCHOICE));
    let (level_distribution, score, score_confidence) =
        answer_score(response, &questions::question_id(evaluation_id, QSCORE));
    let vendor_confidence = vendor_confidence.or(score_confidence);
    let mut structured_value = serde_json::Map::new();
    structured_value.insert(
        "resolved_model".to_owned(),
        serde_json::Value::String(response.model.clone()),
    );
    if let Some(usage) = &response.usage {
        structured_value.insert(
            "usage".to_owned(),
            serde_json::json!({
                "input_tokens": usage.input_tokens,
                "output_tokens": usage.output_tokens,
            }),
        );
    }
    if let Some(quotas) = &response.quotas {
        structured_value.insert("quotas".to_owned(), serde_json::to_value(quotas).unwrap());
    }
    structured_value.insert(
        "latency_ms".to_owned(),
        serde_json::json!(latency.as_millis()),
    );
    structured_value.insert(
        "predicate".to_owned(),
        serde_json::json!({
            "id": predicate.id(),
            "version": predicate.version(),
        }),
    );
    let structured_value = JsonObject::try_from_value(serde_json::Value::Object(structured_value))
        .map_err(|error| FrameDecisionEvaluationFailure::InvalidProvenance {
            message: error.to_string(),
        })?;
    JevStructuredOutput::new(
        noul?,
        option_distribution,
        level_distribution,
        score,
        vendor_confidence,
        box_roi,
        Some(structured_value),
    )
    .map_err(|error| FrameDecisionEvaluationFailure::InvalidProvenance {
        message: error.to_string(),
    })
}

fn answer_noul(
    response: &super::wire::SystemOneResponse,
    id: &str,
) -> Result<Option<f32>, FrameDecisionEvaluationFailure> {
    match response.answers.get(id) {
        Some(WireAnswer::Noul { noul }) => Ok(Some(*noul)),
        None => Ok(None),
        _ => Err(FrameDecisionEvaluationFailure::InvalidProvenance {
            message: format!("expected noul answer for {id}"),
        }),
    }
}

fn answer_choice(
    response: &super::wire::SystemOneResponse,
    id: &str,
) -> (Option<LabProbabilityVector>, Option<f32>) {
    let Some(WireAnswer::Choice {
        confidence,
        probabilities,
        ..
    }) = response.answers.get(id)
    else {
        return (None, None);
    };
    let labels: Vec<String> = probabilities.keys().cloned().collect();
    let values: Vec<f32> = labels
        .iter()
        .filter_map(|label| probabilities.get(label).copied())
        .collect();
    let distribution = LabProbabilityVector::new(labels, values).ok();
    (distribution, Some(*confidence))
}

fn answer_score(
    response: &super::wire::SystemOneResponse,
    id: &str,
) -> (Option<LabProbabilityVector>, Option<f32>, Option<f32>) {
    let Some(WireAnswer::Score {
        score,
        confidence,
        probabilities,
        legend,
    }) = response.answers.get(id)
    else {
        return (None, None, None);
    };
    let mut labels: Vec<String> = legend.values().cloned().collect();
    if labels.is_empty() {
        labels = probabilities.keys().cloned().collect();
    }
    let values: Vec<f32> = labels
        .iter()
        .enumerate()
        .map(|(index, label)| {
            probabilities
                .get(label)
                .or_else(|| probabilities.get(&index.to_string()))
                .copied()
                .unwrap_or(0.0)
        })
        .collect();
    let distribution = LabProbabilityVector::new(labels, values).ok();
    (distribution, Some(*score), Some(*confidence))
}

fn answer_box(
    response: &super::wire::SystemOneResponse,
    id: &str,
    model_input: FrameRoiTransform,
) -> Result<Option<FrameRoiTransform>, FrameDecisionEvaluationFailure> {
    let Some(WireAnswer::Box {
        bounds,
        confidence: _,
    }) = response.answers.get(id)
    else {
        return Ok(None);
    };
    let roi = bounds.to_roi();
    map_box_to_saved_frame(roi, model_input).map(Some)
}
