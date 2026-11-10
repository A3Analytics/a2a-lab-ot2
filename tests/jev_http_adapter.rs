//! `TypeSafe` Jev HTTP adapter fixtures (mock server; no live calls).

use std::io::Cursor;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use axum::Router;
use axum::body::Bytes;
use axum::extract::State;
use axum::http::{HeaderMap, HeaderValue, StatusCode};
use axum::response::IntoResponse;
use axum::routing::post;
use image::ExtendedColorType;
use image::codecs::jpeg::JpegEncoder;
use tokio::net::TcpListener;

use a2a_lab_dev_kit::{
    DecisionEvidenceItem, DecisionOutcome, DecisionSubject, DvrStoreNamespace, DvrTrackId,
    FrameDecisionEvaluationRequest, FrameDecisionEvaluationRouter,
    FrameDecisionEvaluatorLimits, FrameRoiTransform, JevEvaluationId, LabPredicate,
    OperatorDecisionCriteria, OperatorReferenceImage, PipetteChannelSubject, SavedFrameRef,
    Sha256Digest, UtcTimestamp, VideoSourceId,
};

use a2a_lab_ot2::videos::dvr::pilot_evaluator_config;
use a2a_lab_ot2::videos::semantic_indexing::{
    InjectedJevDecisionMedia, TypeSafeJevFrameDecisionEvaluator,
};

static LUNA_SPIES: AtomicUsize = AtomicUsize::new(0);
static CHAT_SPIES: AtomicUsize = AtomicUsize::new(0);

#[derive(Clone)]
struct MockState {
    mode: MockMode,
    calls: Arc<AtomicUsize>,
}

#[derive(Clone, Copy, PartialEq)]
enum MockMode {
    Ok,
    AbsentForeign,
    Malformed,
    Timeout,
    RateLimit,
    RateLimitExhausted,
}

fn frame(payload: &[u8]) -> SavedFrameRef {
    SavedFrameRef::from_payload_bytes(
        DvrStoreNamespace::new("chupacabra.dvr").unwrap(),
        VideoSourceId::new("deck-left").unwrap(),
        DvrTrackId::new("mjpeg-0").unwrap(),
        UtcTimestamp::parse("2024-06-01T12:00:00Z").unwrap(),
        payload,
        None,
    )
    .unwrap()
}

fn encode_jpeg(width: u32, height: u32, tag: u8) -> Vec<u8> {
    let mut pixels = Vec::new();
    for _ in 0..(width * height) {
        pixels.extend_from_slice(&[tag, 10, 20]);
    }
    let mut cursor = Cursor::new(Vec::new());
    JpegEncoder::new_with_quality(&mut cursor, 90)
        .encode(&pixels, width, height, ExtendedColorType::Rgb8)
        .unwrap();
    cursor.into_inner()
}

fn criteria() -> OperatorDecisionCriteria {
    let reference_digest = Sha256Digest::hash_bytes(b"operator-reference");
    OperatorDecisionCriteria::new(
        Sha256Digest::hash_bytes(b"criteria-v1"),
        Sha256Digest::hash_bytes(b"prompt-v1"),
        vec![
            OperatorReferenceImage::new(
                reference_digest,
                "expected-tip",
                FrameRoiTransform::new([0.0, 0.0, 1.0, 1.0]).unwrap(),
            )
            .unwrap(),
        ],
    )
    .unwrap()
}

fn evaluation_request(eval_id: &str, payload: &[u8]) -> FrameDecisionEvaluationRequest {
    let saved = frame(payload);
    let subject = DecisionSubject::PipetteChannel(
        PipetteChannelSubject::new(
            "left-pipetter",
            "channel-1",
            Some(FrameRoiTransform::new([0.2, 0.3, 0.5, 0.7]).unwrap()),
        )
        .unwrap(),
    );
    let evidence = vec![
        DecisionEvidenceItem::saved_frame_revision(
            saved.frame_id().clone(),
            FrameRoiTransform::new([0.0, 0.0, 1.0, 1.0]).unwrap(),
        )
        .unwrap(),
    ];
    FrameDecisionEvaluationRequest::new(
        FrameDecisionEvaluatorLimits::pilot(),
        JevEvaluationId::new(eval_id).unwrap(),
        saved,
        LabPredicate::pipette_tip_attached_v1(),
        subject,
        evidence,
        criteria(),
        FrameRoiTransform::new([0.0, 0.0, 1.0, 1.0]).unwrap(),
        encode_jpeg(320, 280, 7).len(),
        320,
    )
    .unwrap()
}

async fn spawn_mock(mode: MockMode) -> String {
    let calls = Arc::new(AtomicUsize::new(0));
    let state = MockState { mode, calls };
    let app = Router::new()
        .route("/v1/systemone", post(system_one))
        .with_state(state);
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    format!("http://{addr}/v1/systemone")
}

async fn system_one(
    State(state): State<MockState>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<impl IntoResponse, StatusCode> {
    state.calls.fetch_add(1, Ordering::SeqCst);
    assert!(
        headers
            .get("TypeSafe-Danger-Zone-Images")
            .is_some_and(|value| value == "true")
    );
    assert!(headers.get("Authorization").is_none_or(|_| true));
    let payload: serde_json::Value =
        serde_json::from_slice(&body).map_err(|_| StatusCode::BAD_REQUEST)?;
    assert_eq!(payload["model"], "jev-1.14.0");
    match state.mode {
        MockMode::Malformed => {
            return Ok((StatusCode::BAD_REQUEST, "{\"error\":\"bad\"}".to_owned()).into_response());
        }
        MockMode::Timeout => return Err(StatusCode::GATEWAY_TIMEOUT),
        MockMode::RateLimit => {
            if state.calls.load(Ordering::SeqCst) == 1 {
                let mut response = StatusCode::TOO_MANY_REQUESTS.into_response();
                response.headers_mut().insert(
                    axum::http::header::RETRY_AFTER,
                    HeaderValue::from_static("1"),
                );
                return Ok(response);
            }
        }
        MockMode::RateLimitExhausted => {
            let mut response = StatusCode::TOO_MANY_REQUESTS.into_response();
            response.headers_mut().insert(
                axum::http::header::RETRY_AFTER,
                HeaderValue::from_static("1"),
            );
            return Ok(response);
        }
        MockMode::Ok | MockMode::AbsentForeign => {}
    }
    let questions = payload["questions"]
        .as_object()
        .cloned()
        .unwrap_or_default();
    let mut answers = serde_json::Map::new();
    for key in questions.keys() {
        if key.ends_with(":noul") {
            let noul = if state.mode == MockMode::AbsentForeign {
                0.04
            } else {
                0.94
            };
            answers.insert(key.clone(), serde_json::json!({"type":"noul","noul":noul}));
        } else if key.ends_with(":box") {
            answers.insert(
                key.clone(),
                serde_json::json!({
                    "type":"box",
                    "confidence":0.42,
                    "box":{"top":0.2,"bottom":0.5,"left":0.3,"right":0.6}
                }),
            );
        }
    }
    Ok((
        StatusCode::OK,
        serde_json::json!({
            "model":"jev-1.14.0",
            "answers": answers,
            "usage":{"input_tokens":512,"output_tokens":64}
        })
        .to_string(),
    )
        .into_response())
}

fn evaluator(
    endpoint: &str,
    media: Arc<InjectedJevDecisionMedia>,
) -> TypeSafeJevFrameDecisionEvaluator<InjectedJevDecisionMedia> {
    TypeSafeJevFrameDecisionEvaluator::mock(
        pilot_evaluator_config(),
        FrameDecisionEvaluatorLimits::pilot(),
        media,
        endpoint,
    )
}

#[tokio::test]
async fn request_pins_model_auth_header_and_shares_questions() {
    LUNA_SPIES.store(0, Ordering::SeqCst);
    CHAT_SPIES.store(0, Ordering::SeqCst);
    let endpoint = spawn_mock(MockMode::Ok).await;
    let jpeg = encode_jpeg(320, 280, 1);
    let mut media = InjectedJevDecisionMedia::new();
    let req_a = evaluation_request("jev-req-a", b"a");
    let req_b = evaluation_request("jev-req-b", b"a");
    media.insert_frame(req_a.target(), jpeg.clone());
    let reference = criteria().references()[0].digest().clone();
    media.insert_reference(&reference, encode_jpeg(320, 280, 2));
    let evaluator = evaluator(&endpoint, Arc::new(media));
    let mut router = FrameDecisionEvaluationRouter::new(evaluator);
    let batch = router
        .evaluate_frames(vec![req_a.clone(), req_b.clone()])
        .await
        .expect("batch");
    assert_eq!(batch.len(), 2);
    assert!(
        batch[0]
            .structured_output()
            .unwrap()
            .noul_probability()
            .unwrap()
            > 0.9
    );
    assert_eq!(LUNA_SPIES.load(Ordering::SeqCst), 0);
    assert_eq!(CHAT_SPIES.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn box_and_noul_map_without_proving_presence_from_box_alone() {
    let endpoint = spawn_mock(MockMode::Ok).await;
    let jpeg = encode_jpeg(320, 280, 3);
    let mut media = InjectedJevDecisionMedia::new();
    let request = evaluation_request("jev-box", b"b");
    media.insert_frame(request.target(), jpeg);
    media.insert_reference(
        criteria().references()[0].digest(),
        encode_jpeg(320, 280, 4),
    );
    let evaluator = evaluator(&endpoint, Arc::new(media));
    let mut router = FrameDecisionEvaluationRouter::new(evaluator);
    let batch = router.evaluate_frames(vec![request]).await.unwrap();
    let keyed = &batch[0];
    let structured = keyed.structured_output().unwrap();
    assert!(structured.evidence_box().is_some());
    assert_eq!(keyed.outcome(), Some(DecisionOutcome::Present),);
}

#[tokio::test]
async fn malformed_timeout_and_429_are_explicit_failures() {
    let jpeg = encode_jpeg(320, 280, 5);
    let request = evaluation_request("jev-fail", b"c");
    let mut media = InjectedJevDecisionMedia::new();
    media.insert_frame(request.target(), jpeg.clone());
    media.insert_reference(
        criteria().references()[0].digest(),
        encode_jpeg(320, 280, 6),
    );
    for (mode, expected) in [
        (MockMode::Malformed, "invalid_evidence"),
        (MockMode::Timeout, "deadline_exceeded"),
        (MockMode::RateLimitExhausted, "provider_unavailable"),
    ] {
        let endpoint = spawn_mock(mode).await;
        let evaluator = evaluator(&endpoint, Arc::new(media.clone()));
        let mut router = FrameDecisionEvaluationRouter::new(evaluator);
        let err = router
            .evaluate_frames(vec![request.clone()])
            .await
            .expect_err(mode_name(mode));
        assert!(err.to_string().contains(expected), "{err}");
    }
}

fn mode_name(mode: MockMode) -> &'static str {
    match mode {
        MockMode::Ok => "ok",
        MockMode::AbsentForeign => "absent",
        MockMode::Malformed => "malformed",
        MockMode::Timeout => "timeout",
        MockMode::RateLimit => "429",
        MockMode::RateLimitExhausted => "429-exhausted",
    }
}

fn foreign_object_request(eval_id: &str, payload: &[u8]) -> FrameDecisionEvaluationRequest {
    use a2a_lab_dev_kit::ForeignObjectSubject;
    let saved = frame(payload);
    let subject = DecisionSubject::ForeignObject(
        ForeignObjectSubject::new(
            "deck-slot-3",
            FrameRoiTransform::new([0.1, 0.1, 0.9, 0.9]).unwrap(),
            None,
            None,
        )
        .unwrap(),
    );
    let evidence = vec![
        DecisionEvidenceItem::saved_frame_revision(
            saved.frame_id().clone(),
            FrameRoiTransform::new([0.0, 0.0, 1.0, 1.0]).unwrap(),
        )
        .unwrap(),
    ];
    FrameDecisionEvaluationRequest::new(
        FrameDecisionEvaluatorLimits::pilot(),
        JevEvaluationId::new(eval_id).unwrap(),
        saved,
        LabPredicate::foreign_object_present_v1(),
        subject,
        evidence,
        criteria(),
        FrameRoiTransform::new([0.0, 0.0, 1.0, 1.0]).unwrap(),
        encode_jpeg(320, 280, 11).len(),
        320,
    )
    .unwrap()
}

#[tokio::test]
async fn absent_foreign_returns_box_without_present_outcome() {
    let endpoint = spawn_mock(MockMode::AbsentForeign).await;
    let jpeg = encode_jpeg(320, 280, 11);
    let mut media = InjectedJevDecisionMedia::new();
    let request = foreign_object_request("jev-foreign-absent", b"f");
    media.insert_frame(request.target(), jpeg);
    media.insert_reference(
        criteria().references()[0].digest(),
        encode_jpeg(320, 280, 12),
    );
    let evaluator = evaluator(&endpoint, Arc::new(media));
    let mut router = FrameDecisionEvaluationRouter::new(evaluator);
    let batch = router.evaluate_frames(vec![request]).await.unwrap();
    let keyed = &batch[0];
    let structured = keyed.structured_output().unwrap();
    assert!(structured.evidence_box().is_some());
    assert_eq!(keyed.outcome(), Some(DecisionOutcome::Absent));
}

#[tokio::test]
async fn box_coordinates_map_through_model_input_crop() {
    let endpoint = spawn_mock(MockMode::Ok).await;
    let jpeg = encode_jpeg(320, 280, 13);
    let mut media = InjectedJevDecisionMedia::new();
    let crop = FrameRoiTransform::new([0.2, 0.2, 0.8, 0.8]).unwrap();
    let saved = frame(b"g");
    let subject = DecisionSubject::PipetteChannel(
        PipetteChannelSubject::new("left-pipetter", "channel-1", Some(crop)).unwrap(),
    );
    let evidence =
        vec![DecisionEvidenceItem::saved_frame_revision(saved.frame_id().clone(), crop).unwrap()];
    let request = FrameDecisionEvaluationRequest::new(
        FrameDecisionEvaluatorLimits::pilot(),
        JevEvaluationId::new("jev-roi-map").unwrap(),
        saved.clone(),
        LabPredicate::pipette_tip_attached_v1(),
        subject,
        evidence,
        criteria(),
        crop,
        jpeg.len(),
        320,
    )
    .unwrap();
    media.insert_frame(request.target(), jpeg);
    media.insert_reference(
        criteria().references()[0].digest(),
        encode_jpeg(320, 280, 14),
    );
    let evaluator = evaluator(&endpoint, Arc::new(media));
    let mut router = FrameDecisionEvaluationRouter::new(evaluator);
    let batch = router.evaluate_frames(vec![request]).await.unwrap();
    let structured = batch[0].structured_output().unwrap();
    let bounds = structured.evidence_box().unwrap().bounds();
    assert!((bounds[0] - 0.38).abs() < 0.02);
    assert!((bounds[1] - 0.32).abs() < 0.02);
    assert!((bounds[2] - 0.56).abs() < 0.02);
    assert!((bounds[3] - 0.5).abs() < 0.02);
}

#[tokio::test]
async fn retry_after_429_can_succeed_on_second_attempt() {
    let endpoint = spawn_mock(MockMode::RateLimit).await;
    let jpeg = encode_jpeg(320, 280, 8);
    let mut media = InjectedJevDecisionMedia::new();
    let request = evaluation_request("jev-429-ok", b"d");
    media.insert_frame(request.target(), jpeg);
    media.insert_reference(
        criteria().references()[0].digest(),
        encode_jpeg(320, 280, 9),
    );
    let evaluator = evaluator(&endpoint, Arc::new(media));
    let mut router = FrameDecisionEvaluationRouter::new(evaluator);
    let batch = router.evaluate_frames(vec![request]).await.unwrap();
    assert_eq!(batch.len(), 1);
}

#[test]
fn smoke_requires_opt_in_env() {
    assert!(!a2a_lab_ot2::videos::semantic_indexing::jev_smoke_enabled());
}
