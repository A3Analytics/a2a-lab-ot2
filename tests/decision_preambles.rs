//! Outbound generic and semantic decision preambles stay independent.

use std::io::Cursor;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use axum::Router;
use axum::body::Bytes;
use axum::extract::State;
use axum::http::StatusCode;
use axum::http::header::CONTENT_TYPE;
use axum::response::IntoResponse;
use axum::routing::post;
use image::ExtendedColorType;
use image::codecs::jpeg::JpegEncoder;
use tokio::net::TcpListener;

use a2a_lab_dev_kit::{
    A2aLabCommand, A2aLabResult, A2aLabService, DecisionEvidenceItem, DecisionSubject,
    DvrStoreNamespace, DvrTrackId, FrameDecisionEvaluationRequest, FrameDecisionEvaluationRouter,
    FrameDecisionEvaluatorLimits, FrameRoiTransform, Image, ImageDecisionQuestions,
    ImageDescriptor, ImageId, ImageSource, ImageSourceId, JevEvaluationId, JsonObject,
    LabPredicate, MemoryImages, MemoryLogs, MemoryMetrics, MemoryTasks, OperatorDecisionCriteria,
    OperatorReferenceImage, PipetteChannelSubject, SavedFrameRef, Sha256Digest,
    SourceImageDecisionRequest, UtcTimestamp, VideoSourceId,
};
use a2a_lab_ot2::videos::dvr::pilot_evaluator_config;
use a2a_lab_ot2::videos::semantic_indexing::{
    InjectedJevDecisionMedia, TypeSafeJevFrameDecisionEvaluator,
};
use a2a_lab_ot2::{
    ExplicitSetting, LabConfigInputs, attach_configured_image_decision,
    resolve_image_decision_client, resolve_lab_config, resolve_semantic_indexing,
};

const TIP: &str = "Is the tip attached?";
const CHAT_PREAMBLE: &str = "CHAT-PREAMBLE-EXACT";
const IMAGE_PREAMBLE: &str = "  IMAGE-PREAMBLE";
const DECISION_PREAMBLE: &str = "DECISION-PREAMBLE";

enum StoredPreamble {
    Omitted,
    Exact(&'static str),
}

fn temp_dir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "a2a-lab-ot2-decision-preamble-{name}-{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn inputs(path: PathBuf) -> LabConfigInputs {
    LabConfigInputs {
        config_path: ExplicitSetting::cli(Some(path)),
        ..LabConfigInputs::default()
    }
}

fn write_generic_config(
    dir: &Path,
    endpoint: &str,
    preamble: &StoredPreamble,
    typesafe: bool,
) -> LabConfigInputs {
    let mut image_decision = serde_json::json!({
        "endpoint": endpoint,
        "model": "jev-1.14.0",
        "timeout": "5s"
    });
    if let StoredPreamble::Exact(text) = preamble {
        image_decision["preamble"] = serde_json::Value::String((*text).to_owned());
    }
    let document = serde_json::json!({
        "schema_version": 1,
        "chat": {"provider": "openai", "model": "chat-only", "preamble": CHAT_PREAMBLE},
        "image_decision": image_decision
    });
    let path = dir.join("a2a-lab.json");
    std::fs::write(&path, serde_json::to_vec_pretty(&document).unwrap()).unwrap();
    if typesafe {
        std::fs::write(
            dir.join("a2a-lab.secret.json"),
            r#"{"typesafe":{"api_key":"local-test-key"}}"#,
        )
        .unwrap();
    }
    inputs(path)
}

fn limits_value() -> serde_json::Value {
    serde_json::from_str(
        r#"{"worker":{"max_batch_items":4,"max_concurrency":2,"max_queue_backlog":64,"max_retry_attempts":3,"job_deadline":"30s","retry_backoff":"1s"},"search":{"top_k":10,"candidate_limit":1000,"max_neighbor_frames":5,"query_window":"2h"},"roi":{"min_edge_pixels":280,"max_edge_pixels":1568,"max_rois_per_frame":8},"storage":{"vector_index_path":"./state/semantic-index","max_disk_bytes":1073741824},"decisions":{"max_batch_items":8,"max_concurrency":2,"job_deadline":"45s","coverage_policy":"on_demand"}}"#,
    )
    .unwrap()
}

fn write_independent_config(dir: &Path, endpoint: &str) -> LabConfigInputs {
    let document = serde_json::json!({
        "schema_version": 2,
        "chat": {"provider": "openai", "model": "chat-only", "preamble": CHAT_PREAMBLE},
        "videos": {"sources": [{"id": "camera", "device_index": 0, "format": "mjpeg"}]},
        "image_decision": {
            "endpoint": endpoint,
            "model": "jev-1.14.0",
            "timeout": "5s",
            "preamble": IMAGE_PREAMBLE
        },
        "semantic_indexing": {
            "description": {"provider": "bedrock", "model": "custom-luna"},
            "embeddings": {
                "provider": "bedrock",
                "model": "text-embed-v1",
                "space": {
                    "dimensions": 4,
                    "normalization": "l2_unit",
                    "metric": "cosine"
                }
            },
            "limits": limits_value(),
            "decisions": {
                "provider": "jev",
                "model": "jev-1.14.0",
                "preamble": DECISION_PREAMBLE
            }
        }
    });
    let path = dir.join("a2a-lab.json");
    std::fs::write(&path, serde_json::to_vec_pretty(&document).unwrap()).unwrap();
    inputs(path)
}

fn deck_image() -> Image {
    let descriptor = ImageDescriptor::new(
        ImageId::new("frame").unwrap(),
        ImageSourceId::new("cam").unwrap(),
        UtcTimestamp::parse("2024-01-01T00:00:00Z").unwrap(),
        "image/png",
        640,
        480,
        Some("tip".to_owned()),
        JsonObject::empty(),
    )
    .unwrap();
    Image::new(descriptor, vec![9, 8, 7, 6]).unwrap()
}

async fn image_catalog() -> MemoryImages {
    let images = MemoryImages::new();
    images
        .insert_source(ImageSource {
            id: ImageSourceId::new("cam").unwrap(),
            name: "Deck".to_owned(),
            description: "deck camera".to_owned(),
            asset_id: None,
            semantic_id: None,
        })
        .await
        .unwrap();
    images.insert_image(deck_image()).await.unwrap();
    images
}

async fn spawn_mock() -> (String, Arc<Mutex<Vec<serde_json::Value>>>) {
    let bodies = Arc::new(Mutex::new(Vec::new()));
    let app = Router::new()
        .route("/decide", post(record_decision))
        .with_state(Arc::clone(&bodies));
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    (format!("http://{address}/decide"), bodies)
}

async fn record_decision(
    State(bodies): State<Arc<Mutex<Vec<serde_json::Value>>>>,
    body: Bytes,
) -> impl IntoResponse {
    let payload: serde_json::Value =
        serde_json::from_slice(&body).unwrap_or(serde_json::Value::Null);
    let mut answers = serde_json::Map::new();
    if let Some(questions) = payload.get("questions").and_then(|value| value.as_object()) {
        for name in questions.keys() {
            answers.insert(
                name.clone(),
                serde_json::json!({"type": "noul", "noul": 0.5}),
            );
        }
    }
    bodies.lock().expect("bodies").push(payload);
    (
        StatusCode::OK,
        [(CONTENT_TYPE, "application/json")],
        serde_json::json!({ "answers": answers }).to_string(),
    )
}

fn decision_request() -> SourceImageDecisionRequest {
    let questions = serde_json::from_value::<ImageDecisionQuestions>(serde_json::json!({
        "tip": {"type": "noul", "instructions": TIP}
    }))
    .unwrap();
    SourceImageDecisionRequest::new(
        ImageSourceId::new("cam").unwrap(),
        UtcTimestamp::parse("2024-01-01T00:00:00Z").unwrap(),
        questions,
    )
    .unwrap()
}

fn assert_stored_preamble(config: &LabConfigInputs, preamble: &StoredPreamble) {
    let stored = resolve_image_decision_client(config)
        .unwrap()
        .expect("client")
        .config()
        .preamble()
        .map(str::to_owned);
    match preamble {
        StoredPreamble::Omitted => assert_eq!(stored, None),
        StoredPreamble::Exact(text) => assert_eq!(stored.as_deref(), Some(*text)),
    }
}

fn assert_transport(body: &serde_json::Value, typesafe: bool) {
    if typesafe {
        assert!(body["assets"].is_object());
        assert!(body["state"]["image"].is_string());
        return;
    }
    assert_eq!(body["state"]["image"]["media_type"], "image/png");
    assert!(body.get("assets").is_none());
}

async fn generic_body(preamble: &StoredPreamble, typesafe: bool) -> serde_json::Value {
    let (endpoint, bodies) = spawn_mock().await;
    let dir = temp_dir(if typesafe { "typesafe" } else { "http" });
    let config = write_generic_config(&dir, &endpoint, preamble, typesafe);
    assert_stored_preamble(&config, preamble);
    let service = attach_configured_image_decision(
        A2aLabService::new(
            MemoryLogs::default(),
            MemoryMetrics::default(),
            MemoryTasks::default(),
        )
        .with_images(image_catalog().await),
        &config,
    )
    .await
    .unwrap();
    let outcome = service
        .execute(A2aLabCommand::DecideImage(decision_request()))
        .await
        .unwrap();
    assert!(matches!(outcome.task.result, A2aLabResult::DecideImage(_)));
    let seen = bodies.lock().expect("bodies").clone();
    assert_eq!(seen.len(), 1);
    assert_transport(&seen[0], typesafe);
    let _ = dir;
    seen[0].clone()
}

fn tip_instructions(body: &serde_json::Value) -> &str {
    body["questions"]["tip"]["instructions"].as_str().unwrap()
}

#[tokio::test]
async fn http_preamble_is_prepended_and_blank_or_omitted_text_is_unchanged() {
    let configured = generic_body(&StoredPreamble::Exact(IMAGE_PREAMBLE), false).await;
    assert_eq!(
        tip_instructions(&configured),
        format!("{IMAGE_PREAMBLE}\n{TIP}")
    );
    assert!(!configured.to_string().contains(CHAT_PREAMBLE));
    let blank = generic_body(&StoredPreamble::Exact(""), false).await;
    let whitespace = generic_body(&StoredPreamble::Exact(" \t\n"), false).await;
    let omitted = generic_body(&StoredPreamble::Omitted, false).await;
    assert_eq!(tip_instructions(&blank), TIP);
    assert_eq!(tip_instructions(&whitespace), TIP);
    assert_eq!(tip_instructions(&omitted), TIP);
}

#[tokio::test]
async fn typesafe_preamble_is_prepended_and_blank_text_is_unchanged() {
    let configured = generic_body(&StoredPreamble::Exact(IMAGE_PREAMBLE), true).await;
    assert_eq!(
        tip_instructions(&configured),
        format!("{IMAGE_PREAMBLE}\n{TIP}")
    );
    let blank = generic_body(&StoredPreamble::Exact(""), true).await;
    let omitted = generic_body(&StoredPreamble::Omitted, true).await;
    assert_eq!(tip_instructions(&blank), TIP);
    assert_eq!(tip_instructions(&omitted), TIP);
}

#[tokio::test]
async fn overlong_http_preamble_stops_before_the_decision_request() {
    let (endpoint, bodies) = spawn_mock().await;
    let dir = temp_dir("overlong");
    let image_decision = serde_json::json!({
        "endpoint": endpoint,
        "model": "jev-1.14.0",
        "timeout": "5s",
        "preamble": "P".repeat(4_096)
    });
    let path = dir.join("a2a-lab.json");
    std::fs::write(
        &path,
        serde_json::to_vec(&serde_json::json!({
            "schema_version": 1,
            "image_decision": image_decision
        }))
        .unwrap(),
    )
    .unwrap();
    let config = inputs(path);
    let service = attach_configured_image_decision(
        A2aLabService::new(
            MemoryLogs::default(),
            MemoryMetrics::default(),
            MemoryTasks::default(),
        )
        .with_images(image_catalog().await),
        &config,
    )
    .await
    .unwrap();
    let error = service
        .execute(A2aLabCommand::DecideImage(decision_request()))
        .await
        .unwrap_err();
    assert_eq!(error.code(), "limit_exceeded");
    assert!(error.to_string().contains("question"));
    assert!(bodies.lock().expect("bodies").is_empty());
    let _ = dir;
}

fn encode_jpeg() -> Vec<u8> {
    let mut pixels = Vec::new();
    for _ in 0..(320 * 280) {
        pixels.extend_from_slice(&[4, 10, 20]);
    }
    let mut cursor = Cursor::new(Vec::new());
    JpegEncoder::new_with_quality(&mut cursor, 90)
        .encode(&pixels, 320, 280, ExtendedColorType::Rgb8)
        .unwrap();
    cursor.into_inner()
}

fn jev_request(jpeg: &[u8]) -> FrameDecisionEvaluationRequest {
    let saved = SavedFrameRef::from_payload_bytes(
        DvrStoreNamespace::new("chupacabra.dvr").unwrap(),
        VideoSourceId::new("deck-left").unwrap(),
        DvrTrackId::new("mjpeg-0").unwrap(),
        UtcTimestamp::parse("2024-06-01T12:00:00Z").unwrap(),
        b"preamble-frame",
        None,
    )
    .unwrap();
    let model_input = FrameRoiTransform::full_frame();
    let evidence = vec![
        DecisionEvidenceItem::saved_frame_revision(saved.frame_id().clone(), model_input).unwrap(),
    ];
    let reference = Sha256Digest::hash_bytes(b"operator-reference");
    let criteria = OperatorDecisionCriteria::new(
        Sha256Digest::hash_bytes(b"criteria-v1"),
        Sha256Digest::hash_bytes(b"prompt-v1"),
        vec![
            OperatorReferenceImage::new(
                reference,
                "expected-tip",
                FrameRoiTransform::new([0.0, 0.0, 1.0, 1.0]).unwrap(),
            )
            .unwrap(),
        ],
    )
    .unwrap();
    FrameDecisionEvaluationRequest::new(
        FrameDecisionEvaluatorLimits::pilot(),
        JevEvaluationId::new("jev-preamble").unwrap(),
        saved,
        LabPredicate::pipette_tip_attached_v1(),
        DecisionSubject::PipetteChannel(
            PipetteChannelSubject::new("left-pipetter", "channel-1", None).unwrap(),
        ),
        evidence,
        criteria,
        model_input,
        jpeg.len(),
        320,
    )
    .unwrap()
}

#[tokio::test]
async fn configured_preambles_stay_independent_on_outbound_requests() {
    let (endpoint, bodies) = spawn_mock().await;
    let dir = temp_dir("independent");
    let config = write_independent_config(&dir, &endpoint);
    let resolved = resolve_lab_config(&config).unwrap();
    let semantic = resolve_semantic_indexing(&config, &resolved.video)
        .unwrap()
        .expect("semantic");
    let image = resolve_image_decision_client(&config)
        .unwrap()
        .expect("image");
    assert_eq!(resolved.chat.preamble, CHAT_PREAMBLE);
    assert_eq!(image.config().preamble(), Some(IMAGE_PREAMBLE));
    assert_eq!(
        semantic.config.decisions.preamble(),
        Some(DECISION_PREAMBLE)
    );
    let service = attach_configured_image_decision(
        A2aLabService::new(
            MemoryLogs::default(),
            MemoryMetrics::default(),
            MemoryTasks::default(),
        )
        .with_images(image_catalog().await),
        &config,
    )
    .await
    .unwrap();
    service
        .execute(A2aLabCommand::DecideImage(decision_request()))
        .await
        .unwrap();
    let jpeg = encode_jpeg();
    let request = jev_request(&jpeg);
    let mut media = InjectedJevDecisionMedia::new();
    media.insert_frame(request.target(), jpeg);
    let evaluator = TypeSafeJevFrameDecisionEvaluator::mock(
        pilot_evaluator_config(),
        FrameDecisionEvaluatorLimits::pilot(),
        Arc::new(media),
        endpoint,
    )
    .unwrap()
    .with_preamble(semantic.config.decisions.preamble().map(str::to_owned));
    let mut router = FrameDecisionEvaluationRouter::new(evaluator);
    router.evaluate_frames(vec![request]).await.unwrap();
    let seen = bodies.lock().expect("bodies").clone();
    assert_eq!(seen.len(), 2);
    assert_eq!(
        tip_instructions(&seen[0]),
        format!("{IMAGE_PREAMBLE}\n{TIP}")
    );
    assert_eq!(
        seen[1]["questions"]["pipette_tip_attached"]["instructions"],
        format!("{DECISION_PREAMBLE}\npipette tip attached")
    );
    let generic = seen[0].to_string();
    let semantic_body = seen[1].to_string();
    assert!(!generic.contains(CHAT_PREAMBLE));
    assert!(!generic.contains(DECISION_PREAMBLE));
    assert!(!semantic_body.contains(CHAT_PREAMBLE));
    assert!(!semantic_body.contains(IMAGE_PREAMBLE));
    let _ = dir;
}
