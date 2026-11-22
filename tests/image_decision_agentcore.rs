//! `AgentCore` image decisions stay independent of semantic indexing.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use a2a_lab_dev_kit::{
    Image, ImageDecisionExecutor, ImageDecisionHttpLimits, ImageDecisionPrimitiveAnswer,
    ImageDecisionProviderError, ImageDecisionQuestion, ImageDecisionQuestions, ImageDescriptor,
    ImageId, ImageSourceId, ImageTransportConfig, JsonObject, UtcTimestamp,
};
use a2a_lab_ot2::{
    ExplicitSetting, LabConfigInputs, images::AgentCoreImageDecisionExecutor,
    images::AgentCoreInvoker, resolve_image_decision_client, resolve_lab_config,
    resolve_semantic_indexing,
};
use async_trait::async_trait;

struct ScriptedInvoker {
    payload: Arc<Mutex<Option<Vec<u8>>>>,
    result: Result<Vec<u8>, ImageDecisionProviderError>,
}

#[async_trait]
impl AgentCoreInvoker for ScriptedInvoker {
    async fn invoke(&self, payload: &[u8]) -> Result<Vec<u8>, ImageDecisionProviderError> {
        *self.payload.lock().expect("payload") = Some(payload.to_vec());
        self.result.clone()
    }
}

struct RecordingInvoker {
    invoked: Arc<AtomicBool>,
}

#[async_trait]
impl AgentCoreInvoker for RecordingInvoker {
    async fn invoke(&self, _payload: &[u8]) -> Result<Vec<u8>, ImageDecisionProviderError> {
        self.invoked.store(true, Ordering::SeqCst);
        Ok(Vec::new())
    }
}

fn questions() -> ImageDecisionQuestions {
    serde_json::from_value(serde_json::json!({
        "tip_attached": {"type": "noul", "instructions": "Is a pipette tip attached?"},
        "next": {
            "type": "choice",
            "instructions": "What should happen next?",
            "criteria": {"retry": "try again", "stop": "stop the run"}
        },
        "severity": {
            "type": "score",
            "instructions": "How severe is the fault?",
            "criteria": ["low", "high"]
        }
    }))
    .expect("questions")
}

fn image(media_type: &str) -> Image {
    let descriptor = ImageDescriptor::new(
        ImageId::new("frame").unwrap(),
        ImageSourceId::new("opentrons-camera").unwrap(),
        UtcTimestamp::parse("2026-11-20T20:30:00Z").unwrap(),
        media_type,
        8,
        8,
        None,
        JsonObject::empty(),
    )
    .unwrap();
    Image::new(descriptor, vec![0xff, 0xd8, 0xff, 0xd9]).unwrap()
}

fn limits(max_request_bytes: usize, max_response_bytes: usize) -> ImageDecisionHttpLimits {
    ImageDecisionHttpLimits::new(
        ImageTransportConfig::new(1024).unwrap(),
        max_request_bytes,
        max_response_bytes,
        Duration::from_secs(5),
    )
    .unwrap()
}

#[tokio::test]
async fn agentcore_request_uses_the_image_and_returns_validated_answers() {
    let response = serde_json::json!({
        "model": "strands-decider-2B-qwen3.5-v1-2610",
        "answers": {
            "next": {
                "type": "choice",
                "choice": "retry",
                "probabilities": {"retry": 0.8, "stop": 0.2},
                "confidence": 0.6
            },
            "severity": {
                "type": "score",
                "score": 1.0,
                "legend": {"0": "low", "1": "high"},
                "probabilities": {"0": 0.2, "1": 0.8},
                "confidence": 0.6
            },
            "tip_attached": {"type": "noul", "noul": 0.75}
        },
        "usage": {"input_tokens": 271, "output_tokens": 3},
        "latency_ms": 2837.23
    });
    let payload = Arc::new(Mutex::new(None));
    let executor = AgentCoreImageDecisionExecutor::new(
        ScriptedInvoker {
            payload: Arc::clone(&payload),
            result: Ok(serde_json::to_vec(&response).unwrap()),
        },
        limits(8192, 4096),
    );
    let result = executor
        .decide_questions(&image("image/jpeg"), &questions())
        .await
        .unwrap();
    let ImageDecisionPrimitiveAnswer::Noul { noul } = result.answers()["tip_attached"] else {
        panic!("noul answer");
    };
    assert_eq!(noul.to_bits(), 0.75_f32.to_bits());
    let ImageDecisionPrimitiveAnswer::Choice { choice, .. } = &result.answers()["next"] else {
        panic!("choice answer");
    };
    assert_eq!(choice, "retry");
    let ImageDecisionPrimitiveAnswer::Score { score, .. } = result.answers()["severity"] else {
        panic!("score answer");
    };
    assert_eq!(score.to_bits(), 1.0_f32.to_bits());
    let body: serde_json::Value =
        serde_json::from_slice(&payload.lock().expect("payload").clone().unwrap()).unwrap();
    assert!(body.get("model").is_none());
    assert_eq!(body["state"], "");
    assert_eq!(body["images"].as_array().unwrap().len(), 1);
    assert_eq!(body["questions"]["tip_attached"]["type"], "noul");
    assert_eq!(body["questions"]["next"]["type"], "choice");
    assert_eq!(body["questions"]["severity"]["type"], "score");
}

#[test]
fn agentcore_config_starts_without_semantic_indexing() {
    let dir = std::env::temp_dir().join(format!("a2a-lab-agentcore-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("a2a-lab.json");
    std::fs::write(
        &path,
        r#"{
          "schema_version": 1,
          "chat": {"provider": "bedrock", "model": "global.openai.gpt-5.6-luna"},
          "image_decision": {
            "provider": "agentcore",
            "runtime_arn": "arn:aws:bedrock-agentcore:us-west-2:419772657749:runtime/decider",
            "qualifier": "prod",
            "region": "us-west-2"
          }
        }"#,
    )
    .unwrap();
    let inputs = LabConfigInputs {
        config_path: ExplicitSetting::cli(Some(path)),
        ..LabConfigInputs::default()
    };
    let resolved = resolve_lab_config(&inputs).unwrap();
    assert!(
        resolve_semantic_indexing(&inputs, &resolved.video)
            .unwrap()
            .is_none()
    );
    let client = resolve_image_decision_client(&inputs)
        .unwrap()
        .expect("client");
    let settings = client.config().agentcore().expect("agentcore");
    assert_eq!(settings.qualifier(), "prod");
    assert_eq!(settings.region(), "us-west-2");
    assert!(client.config().model().is_none());
    let _ = std::fs::remove_dir_all(dir);
}

#[tokio::test]
async fn malformed_agentcore_answers_are_rejected() {
    let executor = AgentCoreImageDecisionExecutor::new(
        ScriptedInvoker {
            payload: Arc::new(Mutex::new(None)),
            result: Ok(br#"{"answers":{"tip_attached":{"noul":2.0}}}"#.to_vec()),
        },
        limits(8192, 4096),
    );
    let mut asked = ImageDecisionQuestions::new();
    asked.insert(
        "tip_attached".to_owned(),
        serde_json::from_value::<ImageDecisionQuestion>(serde_json::json!({
            "type": "noul",
            "instructions": "Is a pipette tip attached?"
        }))
        .unwrap(),
    );
    let error = executor
        .decide_questions(&image("image/jpeg"), &asked)
        .await
        .unwrap_err();
    assert!(matches!(
        error,
        ImageDecisionProviderError::InvalidAnswer { .. }
    ));
}

#[tokio::test]
async fn oversized_responses_timeouts_and_sdk_failures_stay_bounded() {
    let huge = AgentCoreImageDecisionExecutor::new(
        ScriptedInvoker {
            payload: Arc::new(Mutex::new(None)),
            result: Ok(vec![b' '; 32]),
        },
        limits(8192, 8),
    );
    let error = huge
        .decide_questions(&image("image/jpeg"), &questions())
        .await
        .unwrap_err();
    assert!(matches!(
        error,
        ImageDecisionProviderError::LimitExceeded { .. }
    ));

    let timed_out = AgentCoreImageDecisionExecutor::new(
        ScriptedInvoker {
            payload: Arc::new(Mutex::new(None)),
            result: Err(ImageDecisionProviderError::deadline_exceeded(
                "agentcore decision timed out",
            )
            .unwrap()),
        },
        limits(8192, 4096),
    );
    let error = timed_out
        .decide_questions(&image("image/jpeg"), &questions())
        .await
        .unwrap_err();
    assert!(matches!(
        error,
        ImageDecisionProviderError::DeadlineExceeded { .. }
    ));

    let failed = AgentCoreImageDecisionExecutor::new(
        ScriptedInvoker {
            payload: Arc::new(Mutex::new(None)),
            result: Err(ImageDecisionProviderError::unavailable("service unavailable").unwrap()),
        },
        limits(8192, 4096),
    );
    let error = failed
        .decide_questions(&image("image/jpeg"), &questions())
        .await
        .unwrap_err();
    assert!(matches!(
        error,
        ImageDecisionProviderError::Unavailable { .. }
    ));
}

#[tokio::test]
async fn unsupported_media_and_oversized_requests_do_not_invoke_the_runtime() {
    let invoked = Arc::new(AtomicBool::new(false));
    let executor = AgentCoreImageDecisionExecutor::new(
        RecordingInvoker {
            invoked: Arc::clone(&invoked),
        },
        limits(8, 4096),
    );
    let error = executor
        .decide_questions(&image("image/jpeg"), &questions())
        .await
        .unwrap_err();
    assert!(matches!(
        error,
        ImageDecisionProviderError::LimitExceeded { .. }
    ));
    assert!(!invoked.load(Ordering::SeqCst));

    let invoked = Arc::new(AtomicBool::new(false));
    let executor = AgentCoreImageDecisionExecutor::new(
        RecordingInvoker {
            invoked: Arc::clone(&invoked),
        },
        limits(8192, 4096),
    );
    let error = executor
        .decide_questions(&image("image/bmp"), &questions())
        .await
        .unwrap_err();
    assert!(matches!(
        error,
        ImageDecisionProviderError::InvalidAnswer { .. }
    ));
    assert!(!invoked.load(Ordering::SeqCst));
}
