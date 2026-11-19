//! Devkit compatibility-adapter fixtures (mock generic decision server; no live calls).

use std::io::Cursor;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use axum::Router;
use axum::body::Bytes;
use axum::extract::State;
use axum::http::StatusCode;
use axum::http::header::CONTENT_TYPE;
use axum::response::IntoResponse;
use axum::routing::post;
use base64::Engine;
use image::ExtendedColorType;
use image::codecs::jpeg::JpegEncoder;
use tokio::net::TcpListener;
use tokio::sync::Mutex;

use a2a_lab_dev_kit::{
    DecisionEvidenceItem, DecisionOutcome, DecisionSubject, DvrStoreNamespace, DvrTrackId,
    ForeignObjectSubject, FrameDecisionEvaluationRequest, FrameDecisionEvaluationRouter,
    FrameDecisionEvaluatorLimits, FrameRoiTransform, JevEvaluationId, LabPredicate,
    OperatorDecisionCriteria, OperatorReferenceImage, PipetteChannelSubject,
    PlateAlignmentReference, PlateSlotSubject, SavedFrameRef, Sha256Digest, UtcTimestamp,
    VideoSourceId,
};

use a2a_lab_ot2::videos::dvr::pilot_evaluator_config;
use a2a_lab_ot2::videos::semantic_indexing::{
    InjectedJevDecisionMedia, TypeSafeJevFrameDecisionEvaluator,
};

#[derive(Clone)]
struct MockState {
    mode: MockMode,
    calls: Arc<AtomicUsize>,
    bodies: Arc<Mutex<Vec<serde_json::Value>>>,
}

#[derive(Clone, Copy, PartialEq)]
enum MockMode {
    Ok,
    Absent,
    Malformed,
    Timeout,
    Unavailable,
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

fn evaluation_request(
    eval_id: &str,
    payload: &[u8],
    predicate: LabPredicate,
    subject: DecisionSubject,
    model_input: FrameRoiTransform,
) -> FrameDecisionEvaluationRequest {
    let saved = frame(payload);
    let evidence = vec![
        DecisionEvidenceItem::saved_frame_revision(saved.frame_id().clone(), model_input).unwrap(),
    ];
    FrameDecisionEvaluationRequest::new(
        FrameDecisionEvaluatorLimits::pilot(),
        JevEvaluationId::new(eval_id).unwrap(),
        saved,
        predicate,
        subject,
        evidence,
        criteria(),
        model_input,
        encode_jpeg(320, 280, 7).len(),
        320,
    )
    .unwrap()
}

fn pipette_request(eval_id: &str, payload: &[u8]) -> FrameDecisionEvaluationRequest {
    let subject = DecisionSubject::PipetteChannel(
        PipetteChannelSubject::new("left-pipetter", "channel-1", None).unwrap(),
    );
    evaluation_request(
        eval_id,
        payload,
        LabPredicate::pipette_tip_attached_v1(),
        subject,
        FrameRoiTransform::full_frame(),
    )
}

fn foreign_request(eval_id: &str, payload: &[u8]) -> FrameDecisionEvaluationRequest {
    let subject = DecisionSubject::ForeignObject(
        ForeignObjectSubject::new(
            "deck-slot-3",
            FrameRoiTransform::new([0.1, 0.1, 0.9, 0.9]).unwrap(),
            None,
            None,
        )
        .unwrap(),
    );
    evaluation_request(
        eval_id,
        payload,
        LabPredicate::foreign_object_present_v1(),
        subject,
        FrameRoiTransform::full_frame(),
    )
}

fn plate_request(eval_id: &str, payload: &[u8]) -> FrameDecisionEvaluationRequest {
    let subject = DecisionSubject::PlateSlot(
        PlateSlotSubject::new(
            "plate-1",
            "slot-2",
            PlateAlignmentReference::ExpectedPoseDigest(Sha256Digest::hash_bytes(b"pose")),
            None,
        )
        .unwrap(),
    );
    evaluation_request(
        eval_id,
        payload,
        LabPredicate::plate_misaligned_v1(),
        subject,
        FrameRoiTransform::full_frame(),
    )
}

async fn spawn_mock(mode: MockMode) -> (String, Arc<Mutex<Vec<serde_json::Value>>>) {
    let calls = Arc::new(AtomicUsize::new(0));
    let bodies = Arc::new(Mutex::new(Vec::new()));
    let state = MockState {
        mode,
        calls,
        bodies: Arc::clone(&bodies),
    };
    let app = Router::new()
        .route("/decide", post(decide))
        .with_state(state);
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    (format!("http://{addr}/decide"), bodies)
}

async fn decide(State(state): State<MockState>, body: Bytes) -> impl IntoResponse {
    state.calls.fetch_add(1, Ordering::SeqCst);
    let payload: serde_json::Value =
        serde_json::from_slice(&body).unwrap_or(serde_json::Value::Null);
    state.bodies.lock().await.push(payload);
    match state.mode {
        MockMode::Malformed => {
            return (
                StatusCode::OK,
                [(CONTENT_TYPE, "application/json")],
                "{".to_owned(),
            )
                .into_response();
        }
        MockMode::Timeout => return StatusCode::GATEWAY_TIMEOUT.into_response(),
        MockMode::Unavailable => return StatusCode::TOO_MANY_REQUESTS.into_response(),
        MockMode::Ok | MockMode::Absent => {}
    }
    let probability = if state.mode == MockMode::Absent {
        0.04
    } else {
        0.94
    };
    (
        StatusCode::OK,
        [(CONTENT_TYPE, "application/json")],
        serde_json::json!({
            "answers": [{
                "probability": probability,
                "model_box": {"bounds": [0.3, 0.2, 0.6, 0.5]}
            }]
        })
        .to_string(),
    )
        .into_response()
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
    .unwrap()
}

fn assert_full_image(payload: &serde_json::Value, jpeg: &[u8]) {
    assert_eq!(
        payload["model_input"]["model"],
        a2a_lab_dev_kit::DEFAULT_JEV_MODEL_REVISION
    );
    assert!(payload["model_input"].get("roi").is_none());
    let encoded = payload["image"]["data"].as_str().unwrap();
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(encoded)
        .unwrap();
    assert_eq!(bytes, jpeg);
    assert_eq!(payload["image"]["descriptor"]["width"], 320);
    assert_eq!(payload["image"]["descriptor"]["height"], 280);
}

#[tokio::test]
async fn full_image_requests_pin_the_model_and_omit_roi() {
    let (endpoint, bodies) = spawn_mock(MockMode::Ok).await;
    let jpeg = encode_jpeg(320, 280, 1);
    let mut media = InjectedJevDecisionMedia::new();
    let req_a = pipette_request("jev-req-a", b"a");
    let req_b = pipette_request("jev-req-b", b"a");
    media.insert_frame(req_a.target(), jpeg.clone());
    let evaluator = evaluator(&endpoint, Arc::new(media));
    let mut router = FrameDecisionEvaluationRouter::new(evaluator);
    let batch = router
        .evaluate_frames(vec![req_a, req_b])
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
    let seen = bodies.lock().await;
    assert_eq!(seen.len(), 2);
    for payload in seen.iter() {
        assert_full_image(payload, &jpeg);
    }
}

#[tokio::test]
async fn three_pilot_predicates_evaluate_without_a_local_client() {
    let (endpoint, bodies) = spawn_mock(MockMode::Ok).await;
    let jpeg = encode_jpeg(320, 280, 2);
    let mut media = InjectedJevDecisionMedia::new();
    let requests = vec![
        pipette_request("jev-tip", b"predicates"),
        foreign_request("jev-foreign", b"predicates"),
        plate_request("jev-plate", b"predicates"),
    ];
    media.insert_frame(requests[0].target(), jpeg.clone());
    let evaluator = evaluator(&endpoint, Arc::new(media));
    let mut router = FrameDecisionEvaluationRouter::new(evaluator);
    let batch = router.evaluate_frames(requests).await.unwrap();
    assert_eq!(batch.len(), 3);
    assert!(
        batch
            .iter()
            .all(|item| item.outcome() == Some(DecisionOutcome::Present))
    );
    let seen = bodies.lock().await;
    let questions: Vec<_> = seen
        .iter()
        .map(|payload| payload["question"].as_str().unwrap_or_default())
        .collect();
    assert!(
        questions
            .iter()
            .any(|question| question.contains("lab.pipette_tip_attached"))
    );
    assert!(
        questions
            .iter()
            .any(|question| question.contains("lab.foreign_object_present"))
    );
    assert!(
        questions
            .iter()
            .any(|question| question.contains("lab.plate_misaligned"))
    );
}

#[tokio::test]
async fn box_and_noul_map_without_proving_presence_from_box_alone() {
    let (endpoint, _bodies) = spawn_mock(MockMode::Ok).await;
    let jpeg = encode_jpeg(320, 280, 3);
    let mut media = InjectedJevDecisionMedia::new();
    let request = pipette_request("jev-box", b"b");
    media.insert_frame(request.target(), jpeg);
    let evaluator = evaluator(&endpoint, Arc::new(media));
    let mut router = FrameDecisionEvaluationRouter::new(evaluator);
    let batch = router.evaluate_frames(vec![request]).await.unwrap();
    let keyed = &batch[0];
    let structured = keyed.structured_output().unwrap();
    let bounds = structured.evidence_box().unwrap().bounds();
    assert!((bounds[0] - 0.3).abs() < 0.02);
    assert!((bounds[1] - 0.2).abs() < 0.02);
    assert!((bounds[2] - 0.6).abs() < 0.02);
    assert!((bounds[3] - 0.5).abs() < 0.02);
    assert_eq!(keyed.outcome(), Some(DecisionOutcome::Present));
}

#[tokio::test]
async fn malformed_timeout_and_unavailable_are_explicit_failures() {
    let jpeg = encode_jpeg(320, 280, 5);
    let request = pipette_request("jev-fail", b"c");
    let mut media = InjectedJevDecisionMedia::new();
    media.insert_frame(request.target(), jpeg);
    for (mode, expected) in [
        (MockMode::Malformed, "invalid_provenance"),
        (MockMode::Timeout, "deadline_exceeded"),
        (MockMode::Unavailable, "provider_unavailable"),
    ] {
        let (endpoint, _) = spawn_mock(mode).await;
        let evaluator = evaluator(&endpoint, Arc::new(media.clone()));
        let mut router = FrameDecisionEvaluationRouter::new(evaluator);
        let err = router
            .evaluate_frames(vec![request.clone()])
            .await
            .expect_err("explicit failure");
        assert!(err.to_string().contains(expected), "{err}");
    }
}

#[tokio::test]
async fn absent_foreign_returns_box_without_present_outcome() {
    let (endpoint, _) = spawn_mock(MockMode::Absent).await;
    let jpeg = encode_jpeg(320, 280, 11);
    let mut media = InjectedJevDecisionMedia::new();
    let request = foreign_request("jev-foreign-absent", b"f");
    media.insert_frame(request.target(), jpeg);
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
    let (endpoint, bodies) = spawn_mock(MockMode::Ok).await;
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
        saved,
        LabPredicate::pipette_tip_attached_v1(),
        subject,
        evidence,
        criteria(),
        crop,
        jpeg.len(),
        320,
    )
    .unwrap();
    media.insert_frame(request.target(), jpeg.clone());
    let evaluator = evaluator(&endpoint, Arc::new(media));
    let mut router = FrameDecisionEvaluationRouter::new(evaluator);
    let batch = router.evaluate_frames(vec![request]).await.unwrap();
    let seen = bodies.lock().await;
    let payload = &seen[0];
    let encoded = payload["image"]["data"].as_str().unwrap();
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(encoded)
        .unwrap();
    assert_eq!(bytes, jpeg);
    assert_eq!(
        payload["model_input"]["roi"]["bounds"],
        serde_json::json!([0.2, 0.2, 0.8, 0.8])
    );
    let bounds = batch[0]
        .structured_output()
        .unwrap()
        .evidence_box()
        .unwrap()
        .bounds();
    assert!((bounds[0] - 0.38).abs() < 0.02);
    assert!((bounds[1] - 0.32).abs() < 0.02);
    assert!((bounds[2] - 0.56).abs() < 0.02);
    assert!((bounds[3] - 0.5).abs() < 0.02);
}

#[test]
fn smoke_requires_opt_in_env() {
    assert!(!a2a_lab_ot2::videos::semantic_indexing::jev_smoke_enabled());
}
