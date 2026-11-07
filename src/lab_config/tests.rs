//! Table-driven precedence and merge tests for configuration resolution.

use std::fs;
use std::path::{Path, PathBuf};

use crate::ModelProvider;
use crate::lab_config::{
    ClapValueSource, ExplicitSetting, LabConfigInputs, SettingProvenance, resolve,
    resolve_lab_config, selected_config_path,
};
use crate::videos::DEFAULT_VIDEO_DVR_ROOT;
use a2a_lab_dev_kit::LoadedLabConfig;

fn load_config_file(document: &str, path: &Path) -> LoadedLabConfig {
    fs::write(path, document).unwrap();
    LoadedLabConfig::load(path).unwrap()
}

#[test]
fn env_config_path_selects_without_cli_flag() {
    let inputs = LabConfigInputs {
        config_path: ExplicitSetting {
            value: Some(PathBuf::from("/etc/a2a-lab.json")),
            source: Some(ClapValueSource::Environment),
        },
        ..LabConfigInputs::default()
    };
    assert_eq!(
        selected_config_path(&inputs).unwrap(),
        Some(PathBuf::from("/etc/a2a-lab.json"))
    );
}

#[test]
fn cli_config_overrides_env_semantics_in_selection() {
    let inputs = LabConfigInputs {
        config_path: ExplicitSetting::cli(Some(PathBuf::from("/cli.json"))),
        ..LabConfigInputs::default()
    };
    assert_eq!(
        selected_config_path(&inputs).unwrap(),
        Some(PathBuf::from("/cli.json"))
    );
}

#[test]
fn invalid_selected_file_fails_without_fallback() {
    let dir = std::env::temp_dir().join(format!("a2a-lab-ot2-config-{}", std::process::id()));
    fs::create_dir_all(&dir).unwrap();
    let path = dir.join("bad.json");
    fs::write(&path, r#"{"schema_version":1,"chat":{"provider":"nope"}}"#).unwrap();
    let inputs = LabConfigInputs {
        config_path: ExplicitSetting::cli(Some(path)),
        ..LabConfigInputs::default()
    };
    assert!(resolve_lab_config(&inputs).is_err());
}

#[test]
fn precedence_table_cli_env_file_defaults() {
    struct Case {
        name: &'static str,
        file_fps: Option<u32>,
        cli_fps: ExplicitSetting<u32>,
        expected_fps: u32,
        expected: SettingProvenance,
    }
    let cases = [
        Case {
            name: "cli beats file",
            file_fps: Some(9),
            cli_fps: ExplicitSetting::cli(3),
            expected_fps: 3,
            expected: SettingProvenance::CommandLine,
        },
        Case {
            name: "env beats file",
            file_fps: Some(9),
            cli_fps: ExplicitSetting::env(4),
            expected_fps: 4,
            expected: SettingProvenance::Environment,
        },
        Case {
            name: "file beats clap default",
            file_fps: Some(7),
            cli_fps: ExplicitSetting::default_value(5),
            expected_fps: 7,
            expected: SettingProvenance::File,
        },
        Case {
            name: "application default without file",
            file_fps: None,
            cli_fps: ExplicitSetting::default_value(5),
            expected_fps: 5,
            expected: SettingProvenance::ApplicationDefault,
        },
    ];
    for case in cases {
        let document = match case.file_fps {
            Some(fps) => format!(r#"{{"schema_version":1,"videos":{{"sources":[],"fps":{fps}}}}}"#),
            None => String::new(),
        };
        let dir = std::env::temp_dir().join(format!(
            "a2a-lab-ot2-precedence-{}-{}",
            case.name.replace(' ', "-"),
            std::process::id()
        ));
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join("a2a-lab.json");
        let loaded = if case.file_fps.is_some() {
            Some(load_config_file(&document, &path))
        } else {
            None
        };
        let cli_fps = case.cli_fps.clone();
        let inputs = LabConfigInputs {
            video_fps: cli_fps.clone(),
            ..LabConfigInputs::default()
        };
        let resolved = resolve(&inputs, loaded.as_ref()).unwrap();
        assert_eq!(resolved.video.fps, case.expected_fps, "case {}", case.name);
        let (_, provenance) = cli_fps.explicit_or(case.file_fps, || 5);
        assert_eq!(provenance, case.expected, "case {}", case.name);
    }
}

#[test]
fn dvr_storage_path_provenance_from_file_directory() {
    let dir = std::env::temp_dir().join(format!("a2a-lab-ot2-dvr-{}", std::process::id()));
    fs::create_dir_all(&dir).unwrap();
    let path = dir.join("a2a-lab.json");
    let loaded = load_config_file(
        r#"{"schema_version":1,"videos":{"sources":[],"dvr":{"storage_path":"./nested/dvr"}}}"#,
        &path,
    );
    let inputs = LabConfigInputs::default();
    let resolved = resolve(&inputs, Some(&loaded)).unwrap();
    assert_eq!(
        resolved.video.dvr_storage.provenance,
        SettingProvenance::File
    );
    assert_eq!(resolved.video.dvr_storage.path, dir.join("nested/dvr"));
}

#[test]
fn cli_dvr_root_beats_file_storage_path() {
    let dir = std::env::temp_dir().join(format!("a2a-lab-ot2-dvr-cli-{}", std::process::id()));
    fs::create_dir_all(&dir).unwrap();
    let path = dir.join("a2a-lab.json");
    let loaded = load_config_file(
        r#"{"schema_version":1,"videos":{"sources":[],"dvr":{"storage_path":"./nested/dvr"}}}"#,
        &path,
    );
    let cli_root = dir.join("cli-root");
    let inputs = LabConfigInputs {
        video_dvr_root: ExplicitSetting::cli(cli_root.clone()),
        ..LabConfigInputs::default()
    };
    let resolved = resolve(&inputs, Some(&loaded)).unwrap();
    assert_eq!(
        resolved.video.dvr_storage.provenance,
        SettingProvenance::CommandLine
    );
    assert_eq!(resolved.video.dvr_storage.path, cli_root);
}

#[test]
fn legacy_seconds_override_file_durations() {
    let dir = std::env::temp_dir().join(format!("a2a-lab-ot2-duration-{}", std::process::id()));
    fs::create_dir_all(&dir).unwrap();
    let path = dir.join("a2a-lab.json");
    let loaded = load_config_file(
        r#"{"schema_version":1,"videos":{"sources":[],"live_retention":"1h","dvr":{"retention":"2d"}}}"#,
        &path,
    );
    let inputs = LabConfigInputs {
        video_retention_secs: ExplicitSetting::cli(11),
        video_disk_retention_secs: ExplicitSetting::env(22),
        ..LabConfigInputs::default()
    };
    let resolved = resolve(&inputs, Some(&loaded)).unwrap();
    assert_eq!(resolved.video.live_retention_secs, 11);
    assert_eq!(resolved.video.disk_retention_secs, 22);
}

#[test]
fn sources_replace_and_empty_list_stays_off() {
    let dir = std::env::temp_dir().join(format!("a2a-lab-ot2-sources-{}", std::process::id()));
    fs::create_dir_all(&dir).unwrap();
    let path = dir.join("a2a-lab.json");
    let loaded = load_config_file(r#"{"schema_version":1,"videos":{"sources":[]}}"#, &path);
    let inputs = LabConfigInputs::default();
    let resolved = resolve(&inputs, Some(&loaded)).unwrap();
    assert!(resolved.video.sources.is_empty());
}

#[test]
fn file_names_survive_explicit_device_indexes() {
    let dir = std::env::temp_dir().join(format!("a2a-lab-ot2-names-{}", std::process::id()));
    fs::create_dir_all(&dir).unwrap();
    let path = dir.join("a2a-lab.json");
    let loaded = load_config_file(
        r#"{"schema_version":1,"videos":{"sources":[
            {"id":"camera","name":"orbbec","description":"Live MJPEG from the configured camera.","format":"mjpeg","device_index":1},
            {"id":"external-camera","name":"emeet","description":"Still frame from the configured external camera.","format":"mjpeg","device_index":2}
        ]}}"#,
        &path,
    );
    let inputs = LabConfigInputs {
        camera_index: ExplicitSetting::cli(Some(12)),
        external_camera_index: ExplicitSetting::cli(Some(4)),
        ..LabConfigInputs::default()
    };
    let resolved = resolve(&inputs, Some(&loaded)).unwrap();
    let camera = resolved
        .video
        .sources
        .iter()
        .find(|source| source.id == "camera")
        .unwrap();
    let external = resolved
        .video
        .sources
        .iter()
        .find(|source| source.id == "external-camera")
        .unwrap();
    assert_eq!(camera.name, "orbbec");
    assert_eq!(camera.device_index, 12);
    assert_eq!(camera.delivery, crate::videos::LiveVideoDelivery::H264);
    assert_eq!(external.name, "emeet");
    assert_eq!(external.device_index, 4);
    assert_eq!(
        external.description,
        "Still frame from the configured external camera."
    );
}

#[test]
fn explicit_camera_index_enables_source_from_empty_file_list() {
    let dir = std::env::temp_dir().join(format!("a2a-lab-ot2-enable-{}", std::process::id()));
    fs::create_dir_all(&dir).unwrap();
    let path = dir.join("a2a-lab.json");
    let loaded = load_config_file(r#"{"schema_version":1,"videos":{"sources":[]}}"#, &path);
    let inputs = LabConfigInputs {
        camera_index: ExplicitSetting::cli(Some(2)),
        ..LabConfigInputs::default()
    };
    let resolved = resolve(&inputs, Some(&loaded)).unwrap();
    assert_eq!(resolved.video.sources.len(), 1);
    assert_eq!(resolved.video.sources[0].id, "camera");
    assert_eq!(resolved.video.sources[0].device_index, 2);
}

#[test]
fn description_override_does_not_enable_external_source() {
    let dir = std::env::temp_dir().join(format!("a2a-lab-ot2-desc-only-{}", std::process::id()));
    fs::create_dir_all(&dir).unwrap();
    let path = dir.join("a2a-lab.json");
    let loaded = load_config_file(r#"{"schema_version":1,"videos":{"sources":[]}}"#, &path);
    let inputs = LabConfigInputs {
        external_camera_description: ExplicitSetting::cli("Bench overview".to_owned()),
        ..LabConfigInputs::default()
    };
    let resolved = resolve(&inputs, Some(&loaded)).unwrap();
    assert!(resolved.video.sources.is_empty());
}

#[test]
fn duplicate_device_indices_fail_after_merge() {
    let dir = std::env::temp_dir().join(format!("a2a-lab-ot2-dup-{}", std::process::id()));
    fs::create_dir_all(&dir).unwrap();
    let path = dir.join("a2a-lab.json");
    let loaded = load_config_file(
        r#"{"schema_version":1,"videos":{"sources":[
            {"id":"external-camera","format":"mjpeg","device_index":2}
        ]}}"#,
        &path,
    );
    let inputs = LabConfigInputs {
        camera_index: ExplicitSetting::cli(Some(2)),
        ..LabConfigInputs::default()
    };
    assert!(resolve(&inputs, Some(&loaded)).is_err());
}

#[test]
fn provider_before_model_and_bedrock_only_fallback() {
    let dir = std::env::temp_dir().join(format!("a2a-lab-ot2-chat-{}", std::process::id()));
    fs::create_dir_all(&dir).unwrap();
    let path = dir.join("a2a-lab.json");
    let loaded = load_config_file(
        r#"{"schema_version":1,"chat":{"provider":"anthropic"}}"#,
        &path,
    );
    let inputs = LabConfigInputs {
        bedrock_model: ExplicitSetting::cli(Some("ignored-bedrock".to_owned())),
        ..LabConfigInputs::default()
    };
    let resolved = resolve(&inputs, Some(&loaded)).unwrap();
    assert_eq!(resolved.chat.provider, ModelProvider::Anthropic);
    assert_eq!(resolved.chat.model, crate::DEFAULT_ANTHROPIC_MODEL);

    let bedrock_loaded = load_config_file(
        r#"{"schema_version":1,"chat":{"provider":"bedrock"}}"#,
        &path,
    );
    let bedrock_inputs = LabConfigInputs {
        bedrock_model: ExplicitSetting::cli(Some("custom-nova".to_owned())),
        ..LabConfigInputs::default()
    };
    let bedrock = resolve(&bedrock_inputs, Some(&bedrock_loaded)).unwrap();
    assert_eq!(bedrock.chat.model, "custom-nova");
}

#[test]
fn explicit_model_beats_file_and_bedrock_override() {
    let dir = std::env::temp_dir().join(format!("a2a-lab-ot2-model-{}", std::process::id()));
    fs::create_dir_all(&dir).unwrap();
    let path = dir.join("a2a-lab.json");
    let loaded = load_config_file(
        r#"{"schema_version":1,"chat":{"provider":"bedrock","model":"file-model"}}"#,
        &path,
    );
    let inputs = LabConfigInputs {
        model: ExplicitSetting::cli(Some("cli-model".to_owned())),
        bedrock_model: ExplicitSetting::cli(Some("bedrock-model".to_owned())),
        ..LabConfigInputs::default()
    };
    let resolved = resolve(&inputs, Some(&loaded)).unwrap();
    assert_eq!(resolved.chat.model, "cli-model");
}

#[test]
fn clap_default_provider_does_not_block_file_provider() {
    let dir = std::env::temp_dir().join(format!("a2a-lab-ot2-prov-{}", std::process::id()));
    fs::create_dir_all(&dir).unwrap();
    let path = dir.join("a2a-lab.json");
    let loaded = load_config_file(
        r#"{"schema_version":1,"chat":{"provider":"openai","model":"from-file"}}"#,
        &path,
    );
    let inputs = LabConfigInputs::default();
    let resolved = resolve(&inputs, Some(&loaded)).unwrap();
    assert_eq!(resolved.chat.provider, ModelProvider::OpenAi);
    assert_eq!(resolved.chat.model, "from-file");
}

#[test]
fn no_file_resolution_matches_application_defaults() {
    let inputs = LabConfigInputs::default();
    let resolved = resolve(&inputs, None).unwrap();
    assert_eq!(resolved.video.fps, 5);
    assert_eq!(
        resolved.video.dvr_storage.path,
        PathBuf::from(DEFAULT_VIDEO_DVR_ROOT)
    );
    assert_eq!(
        resolved.video.dvr_storage.provenance,
        SettingProvenance::ApplicationDefault
    );
}

#[test]
fn chat_only_file_leaves_video_defaults_unchanged() {
    let dir = std::env::temp_dir().join(format!("a2a-lab-ot2-chat-only-{}", std::process::id()));
    fs::create_dir_all(&dir).unwrap();
    let path = dir.join("a2a-lab.json");
    let loaded = load_config_file(
        r#"{"schema_version":1,"chat":{"provider":"openai","model":"from-chat-file"}}"#,
        &path,
    );
    let baseline = resolve(&LabConfigInputs::default(), None).unwrap();
    let with_chat_file = resolve(&LabConfigInputs::default(), Some(&loaded)).unwrap();
    assert_eq!(with_chat_file.video, baseline.video);
    assert_eq!(with_chat_file.chat.provider, ModelProvider::OpenAi);
    assert_eq!(with_chat_file.chat.model, "from-chat-file");
}

#[test]
fn videos_only_file_leaves_legacy_serve_chat_unchanged() {
    let dir = std::env::temp_dir().join(format!("a2a-lab-ot2-videos-only-{}", std::process::id()));
    fs::create_dir_all(&dir).unwrap();
    let path = dir.join("a2a-lab.json");
    fs::write(
        &path,
        r#"{"schema_version":1,"videos":{"sources":[],"fps":9}}"#,
    )
    .unwrap();
    let inputs = LabConfigInputs {
        config_path: ExplicitSetting::cli(Some(path.clone())),
        ..LabConfigInputs::default()
    };
    let legacy = crate::lab_config::ServeChatLegacyInputs {
        model_provider: ModelProvider::Bedrock,
        model: None,
        bedrock_model: None,
    };
    let chat = crate::lab_config::serve_chat_settings(&inputs, &legacy).unwrap();
    let legacy_only =
        crate::lab_config::serve_chat_settings(&LabConfigInputs::default(), &legacy).unwrap();
    assert_eq!(chat, legacy_only);
    assert_eq!(chat.model, crate::DEFAULT_BEDROCK_MODEL);
}

#[test]
fn file_provider_without_model_uses_provider_default() {
    let dir = std::env::temp_dir().join(format!("a2a-lab-ot2-openai-file-{}", std::process::id()));
    fs::create_dir_all(&dir).unwrap();
    let path = dir.join("a2a-lab.json");
    load_config_file(
        r#"{"schema_version":1,"chat":{"provider":"openai"}}"#,
        &path,
    );
    let inputs = LabConfigInputs {
        config_path: ExplicitSetting::cli(Some(path)),
        ..LabConfigInputs::default()
    };
    let legacy = crate::lab_config::ServeChatLegacyInputs {
        model_provider: ModelProvider::Bedrock,
        model: None,
        bedrock_model: None,
    };
    let chat = crate::lab_config::serve_chat_settings(&inputs, &legacy).unwrap();
    assert_eq!(chat.provider, ModelProvider::OpenAi);
    assert_eq!(chat.model, crate::DEFAULT_OPENAI_MODEL);
}
