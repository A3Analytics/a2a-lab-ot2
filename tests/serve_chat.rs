//! Serve chat resolution and injected agent factory recording.

use std::fs;

use a2a_lab_ot2::{
    ConversationStore, ExplicitSetting, LabConfigInputs, ModelProvider, RecordingServeAgentFactory,
    ServeChatLegacyInputs, build_serve_lab_agent, serve_chat_settings,
};

async fn record_once(
    inputs: &LabConfigInputs,
    legacy: &ServeChatLegacyInputs,
) -> (
    RecordingServeAgentFactory,
    Result<(), a2a_lab_dev_kit::A2aLabError>,
) {
    let recorder = RecordingServeAgentFactory::new();
    let hook = recorder.clone();
    let store = ConversationStore::open(
        &std::env::temp_dir().join(format!("serve-chat-it-{}", std::process::id())),
        1,
    )
    .await
    .expect("store");
    let result = build_serve_lab_agent(inputs, legacy, "http://127.0.0.1:9/mcp", store, {
        let hook = hook.clone();
        move |provider, model, _url, _store| {
            let hook = hook.clone();
            async move {
                hook.record(provider, &model)?;
                Err::<a2a_lab_ot2::LabAgent, _>(a2a_lab_dev_kit::A2aLabError::unavailable(
                    "stop before paid provider",
                ))
            }
        }
    })
    .await
    .map(|_pair: (a2a_lab_ot2::LabAgent, _)| ());
    (recorder, result)
}

#[tokio::test]
async fn factory_records_legacy_no_file_chat_once() {
    let legacy = ServeChatLegacyInputs {
        model_provider: ModelProvider::Anthropic,
        model: None,
        bedrock_model: None,
    };
    let expected = serve_chat_settings(&LabConfigInputs::default(), &legacy).unwrap();
    let (recorder, result) = record_once(&LabConfigInputs::default(), &legacy).await;
    assert!(result.is_err());
    assert_eq!(
        recorder.take_recorded().unwrap(),
        Some((expected.provider, expected.model))
    );
}

#[tokio::test]
async fn factory_records_file_only_chat_once() {
    let dir = std::env::temp_dir().join(format!("serve-chat-file-{}", std::process::id()));
    fs::create_dir_all(&dir).unwrap();
    let path = dir.join("a2a-lab.json");
    fs::write(
        &path,
        r#"{"schema_version":1,"chat":{"provider":"anthropic"}}"#,
    )
    .unwrap();
    let inputs = LabConfigInputs {
        config_path: ExplicitSetting::cli(Some(path)),
        ..LabConfigInputs::default()
    };
    let legacy = ServeChatLegacyInputs {
        model_provider: ModelProvider::Bedrock,
        model: None,
        bedrock_model: None,
    };
    let expected = serve_chat_settings(&inputs, &legacy).unwrap();
    let (recorder, result) = record_once(&inputs, &legacy).await;
    assert!(result.is_err());
    assert_eq!(
        recorder.take_recorded().unwrap(),
        Some((expected.provider, expected.model))
    );
}

#[tokio::test]
async fn factory_records_explicit_override_once() {
    let inputs = LabConfigInputs {
        model: ExplicitSetting::cli(Some("override-model".to_owned())),
        ..LabConfigInputs::default()
    };
    let legacy = ServeChatLegacyInputs {
        model_provider: ModelProvider::Bedrock,
        model: Some("override-model".to_owned()),
        bedrock_model: None,
    };
    let expected = serve_chat_settings(&inputs, &legacy).unwrap();
    assert_eq!(expected.model, "override-model");
    let (recorder, result) = record_once(&inputs, &legacy).await;
    assert!(result.is_err());
    assert_eq!(
        recorder.take_recorded().unwrap(),
        Some((expected.provider, expected.model))
    );
}

#[test]
fn config_validate_output_never_includes_api_key_markers() {
    let dir = std::env::temp_dir().join(format!("serve-chat-keys-{}", std::process::id()));
    fs::create_dir_all(&dir).unwrap();
    let path = dir.join("a2a-lab.json");
    fs::write(&path, r#"{"schema_version":1}"#).unwrap();
    let output = std::process::Command::new(env!("CARGO_BIN_EXE_a2a-lab-ot2"))
        .env("OPENAI_API_KEY", "sk-test-secret-value")
        .env("ANTHROPIC_API_KEY", "sk-ant-test-secret")
        .args(["config", "validate", "--config"])
        .arg(&path)
        .output()
        .unwrap();
    let combined = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(!combined.contains("sk-test-secret"));
    assert!(!combined.contains("sk-ant-test"));
}
