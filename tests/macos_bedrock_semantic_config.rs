//! Credential-free macOS Bedrock semantic configuration example and resolution checks.

use std::collections::HashSet;
use std::fs;
use std::path::PathBuf;
use std::process::Command;

use a2a_lab_dev_kit::{
    A2aLabConfig, ChatProvider, DEFAULT_BEDROCK_DESCRIPTION_MODEL, EmbeddingMetric,
    FrameDescriptionBackend, ImageProvider, ListImageSourcesRequest, LoadedLabConfig, PageRequest,
    VectorNormalization,
};
use a2a_lab_ot2::videos::LiveCameraInputFormat;
use a2a_lab_ot2::videos::semantic_indexing::{
    description_provider_config, embedding_provider_config,
};
use a2a_lab_ot2::{
    DEFAULT_DISK_VIDEO_RETENTION_SECS, ExplicitSetting, ImageServeConfig, LabConfigInputs,
    image_catalog, resolve_lab_config, resolve_semantic_indexing,
};

const OPENTRONS_CAMERA_SOURCE_ID: &str = "opentrons-camera";

fn example_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(".mise/examples/local.json")
}

fn example_text() -> String {
    fs::read_to_string(example_path()).unwrap()
}

fn inputs_for_example() -> LabConfigInputs {
    LabConfigInputs {
        config_path: ExplicitSetting::cli(Some(example_path())),
        ..LabConfigInputs::default()
    }
}

#[test]
fn example_passes_config_validate_without_credentials() {
    let output = Command::new(env!("CARGO_BIN_EXE_a2a-lab-ot2"))
        .env_remove("OPENAI_API_KEY")
        .env_remove("ANTHROPIC_API_KEY")
        .env_remove("AWS_ACCESS_KEY_ID")
        .env_remove("AWS_SECRET_ACCESS_KEY")
        .env_remove("AWS_PROFILE")
        .env("A2ALAB_OPENTRONS_URL", "http://127.0.0.1:9")
        .args(["config", "validate", "--config"])
        .arg(example_path())
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(String::from_utf8_lossy(&output.stdout).contains("configuration valid"));
}

#[test]
fn example_is_schema_v2_without_secret_fields() {
    let text = example_text();
    assert!(!text.contains("credentials"));
    assert!(!text.contains("device_index"));
    let loaded = LoadedLabConfig::load(example_path()).unwrap();
    assert_eq!(loaded.config.schema_version, 2);
    let semantic = loaded.config.semantic_indexing.as_ref().unwrap();
    assert!(semantic.description.credentials().is_none());
    assert!(
        semantic
            .embeddings
            .as_ref()
            .unwrap()
            .credentials()
            .is_none()
    );
    assert!(semantic.decisions.credentials().is_none());
}

#[test]
fn resolved_mac_video_source_keeps_camera_id_and_auto_format() {
    let resolved = resolve_lab_config(&inputs_for_example()).unwrap();
    assert_eq!(
        resolved.video.disk_retention_secs,
        DEFAULT_DISK_VIDEO_RETENTION_SECS
    );
    assert_eq!(DEFAULT_DISK_VIDEO_RETENTION_SECS, 259_200);
    assert_eq!(resolved.video.sources.len(), 1);
    let source = &resolved.video.sources[0];
    assert_eq!(source.id, "mac-built-in");
    assert_eq!(source.input, LiveCameraInputFormat::Auto);
    assert_eq!(
        source.camera_id,
        Some(a2a_lab_dev_kit::CameraSelector::Index(0))
    );
    assert_eq!(source.device_index, Some(0));
    let declarative = source.to_video_source_config().unwrap();
    assert_eq!(declarative.format, a2a_lab_dev_kit::VideoInputFormat::Auto);
    assert_eq!(
        declarative.camera_id,
        Some(a2a_lab_dev_kit::CameraSelector::Index(0))
    );
}

#[test]
fn semantic_indexing_selects_bedrock_independent_from_chat() {
    let loaded = LoadedLabConfig::load(example_path()).unwrap();
    let chat = loaded.config.chat.as_ref().unwrap();
    assert_eq!(chat.provider, Some(ChatProvider::Bedrock));

    let semantic = loaded.config.semantic_indexing.as_ref().unwrap();
    assert_eq!(
        semantic.description.backend(),
        FrameDescriptionBackend::Bedrock
    );
    assert_eq!(
        semantic.description.model(),
        DEFAULT_BEDROCK_DESCRIPTION_MODEL
    );
    let embeddings = semantic.embeddings.as_ref().unwrap();
    assert_eq!(embeddings.provider(), "bedrock");
    assert_eq!(embeddings.model(), "amazon.titan-embed-text-v2:0");
    assert_eq!(embeddings.space().dimensions, 1024);
    assert_eq!(
        embeddings.space().normalization,
        VectorNormalization::L2Unit
    );
    assert_eq!(embeddings.space().metric, EmbeddingMetric::Cosine);

    let description = description_provider_config(&semantic.description).unwrap();
    assert_eq!(description.backend(), FrameDescriptionBackend::Bedrock);
    let embedding = embedding_provider_config(embeddings).unwrap();
    assert_eq!(embedding.provider(), "bedrock");
    assert_eq!(embedding.model(), "amazon.titan-embed-text-v2:0");
}

#[test]
fn embedding_model_and_vector_space_overrides_leave_chat_unchanged() {
    let customized = example_text()
        .replace(
            "\"model\": \"amazon.titan-embed-text-v2:0\"",
            "\"model\": \"amazon.titan-embed-text-v1\"",
        )
        .replace("\"dimensions\": 1024", "\"dimensions\": 1536");
    let customized_config = A2aLabConfig::parse(&customized).unwrap();
    let customized_embeddings = customized_config
        .semantic_indexing
        .unwrap()
        .embeddings
        .unwrap();
    assert_eq!(customized_embeddings.model(), "amazon.titan-embed-text-v1");
    assert_eq!(customized_embeddings.space().dimensions, 1536);
    assert_eq!(
        customized_config.chat.as_ref().unwrap().model.as_deref(),
        None
    );
    let customized_provider = embedding_provider_config(&customized_embeddings).unwrap();
    assert_eq!(customized_provider.model(), "amazon.titan-embed-text-v1");
    assert_eq!(customized_provider.model_space().dimensions(), 1536);
}

#[tokio::test]
async fn configured_video_registers_image_source_without_opentrons_video_or_semantic() {
    let inputs = inputs_for_example();
    let resolved = resolve_lab_config(&inputs).unwrap();
    assert!(
        resolved
            .video
            .sources
            .iter()
            .all(|source| source.id != OPENTRONS_CAMERA_SOURCE_ID)
    );

    let image_config = ImageServeConfig::new(None, None, 64 * 1024, 10_000, 30)
        .unwrap()
        .with_native_video_sources(&resolved.video.sources);
    let catalog = image_catalog("http://127.0.0.1:31950", &image_config, None, None, None).unwrap();
    let page = catalog
        .list_image_sources(
            ListImageSourcesRequest::new(PageRequest::new(None, 20).unwrap()).unwrap(),
        )
        .await
        .unwrap();
    let ids = page
        .items()
        .iter()
        .map(|item| item.id.as_str())
        .collect::<HashSet<_>>();
    assert!(ids.contains("mac-built-in"));
    assert!(ids.contains(OPENTRONS_CAMERA_SOURCE_ID));
    let opentrons = page
        .items()
        .iter()
        .find(|item| item.id.as_str() == OPENTRONS_CAMERA_SOURCE_ID)
        .unwrap();
    assert!(opentrons.semantic_id.is_none());

    let semantic = resolve_semantic_indexing(&inputs, &resolved.video)
        .unwrap()
        .expect("semantic indexing enabled");
    assert!(semantic.indexed_source_ids.contains("mac-built-in"));
    assert!(
        !semantic
            .indexed_source_ids
            .contains(OPENTRONS_CAMERA_SOURCE_ID)
    );
}
