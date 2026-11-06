//! Config file path selection from `--config` and `A2ALAB_CONFIG`.

use std::future::Future;
use std::path::PathBuf;

use a2a_lab_dev_kit::A2aLabError;

use crate::agent::{ModelProvider, selected_model};

use super::inputs::{ClapValueSource, LabConfigInputs};
use super::resolve::ResolvedChatSettings;
use super::resolve_lab_config;

/// Whether serve should build video capture from [`super::resolve_lab_config`].
pub fn uses_resolved_video_configuration(inputs: &LabConfigInputs) -> Result<bool, A2aLabError> {
    if selected_config_path(inputs)?.is_some() {
        return Ok(true);
    }
    Ok(inputs.external_camera_index.is_explicit() || inputs.camera_index.is_explicit())
}

/// Whether serve should build the chat agent from [`super::resolve_lab_config`].
pub fn uses_resolved_chat_configuration(inputs: &LabConfigInputs) -> Result<bool, A2aLabError> {
    if selected_config_path(inputs)?.is_some() {
        return Ok(true);
    }
    Ok(inputs.model_provider.is_explicit()
        || inputs.model.is_explicit()
        || inputs.bedrock_model.is_explicit())
}

/// CLI values used when chat is not resolved from file or explicit overrides.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServeChatLegacyInputs {
    pub model_provider: ModelProvider,
    pub model: Option<String>,
    pub bedrock_model: Option<String>,
}

/// Provider and model id for serve startup after precedence merge or legacy selection.
pub fn serve_chat_settings(
    inputs: &LabConfigInputs,
    legacy: &ServeChatLegacyInputs,
) -> Result<ResolvedChatSettings, A2aLabError> {
    if uses_resolved_chat_configuration(inputs)? {
        return Ok(resolve_lab_config(inputs)?.chat);
    }
    let provider = legacy.model_provider;
    let model = selected_model(
        provider,
        legacy.model.as_deref(),
        legacy.bedrock_model.as_deref(),
    );
    Ok(ResolvedChatSettings { provider, model })
}

/// Builds the serve agent after resolving chat settings, invoking `factory` exactly once.
pub async fn build_serve_lab_agent<F, Fut, A>(
    inputs: &LabConfigInputs,
    legacy: &ServeChatLegacyInputs,
    mcp_url: &str,
    store: crate::ConversationStore,
    factory: F,
) -> Result<(A, ResolvedChatSettings), A2aLabError>
where
    F: FnOnce(ModelProvider, String, String, crate::ConversationStore) -> Fut,
    Fut: Future<Output = Result<A, A2aLabError>>,
{
    let chat = serve_chat_settings(inputs, legacy)?;
    let built = factory(chat.provider, chat.model.clone(), mcp_url.to_owned(), store).await?;
    Ok((built, chat))
}

/// Returns the selected config path, if any. `--config` wins over `A2ALAB_CONFIG`.
pub fn selected_config_path(inputs: &LabConfigInputs) -> Result<Option<PathBuf>, A2aLabError> {
    match inputs.config_path.source {
        Some(ClapValueSource::CommandLine | ClapValueSource::Environment) => {
            validate_some_path(inputs.config_path.value.as_ref())
        }
        _ => Ok(None),
    }
}

fn validate_some_path(path: Option<&PathBuf>) -> Result<Option<PathBuf>, A2aLabError> {
    let Some(path) = path else {
        return Err(A2aLabError::invalid("config", "path must not be empty"));
    };
    if path.as_os_str().is_empty() {
        return Err(A2aLabError::invalid("config", "path must not be empty"));
    }
    Ok(Some(path.clone()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ModelProvider;
    use crate::lab_config::inputs::{ExplicitSetting, LabConfigInputs};

    #[test]
    fn cli_path_with_spaces_is_selected() {
        let path = PathBuf::from("/tmp/my config/a2a-lab.json");
        let inputs = LabConfigInputs {
            config_path: ExplicitSetting::cli(Some(path.clone())),
            ..LabConfigInputs::default()
        };
        assert_eq!(selected_config_path(&inputs).unwrap(), Some(path));
    }

    #[test]
    fn empty_explicit_cli_path_is_rejected() {
        let inputs = LabConfigInputs {
            config_path: ExplicitSetting::cli(Some(PathBuf::new())),
            ..LabConfigInputs::default()
        };
        assert!(selected_config_path(&inputs).is_err());
    }

    #[test]
    fn resolved_video_requires_file_or_explicit_legacy_index() {
        assert!(!uses_resolved_video_configuration(&LabConfigInputs::default()).unwrap());
        let with_file = LabConfigInputs {
            config_path: ExplicitSetting::cli(Some(PathBuf::from("/tmp/a2a-lab.json"))),
            ..LabConfigInputs::default()
        };
        assert!(uses_resolved_video_configuration(&with_file).unwrap());
        let with_camera = LabConfigInputs {
            camera_index: ExplicitSetting::cli(Some(1)),
            ..LabConfigInputs::default()
        };
        assert!(uses_resolved_video_configuration(&with_camera).unwrap());
    }

    #[test]
    fn resolved_chat_requires_file_or_explicit_override() {
        assert!(!uses_resolved_chat_configuration(&LabConfigInputs::default()).unwrap());
        let with_file = LabConfigInputs {
            config_path: ExplicitSetting::cli(Some(PathBuf::from("/tmp/a2a-lab.json"))),
            ..LabConfigInputs::default()
        };
        assert!(uses_resolved_chat_configuration(&with_file).unwrap());
        let with_model = LabConfigInputs {
            model: ExplicitSetting::cli(Some("gpt-test".to_owned())),
            ..LabConfigInputs::default()
        };
        assert!(uses_resolved_chat_configuration(&with_model).unwrap());
        let clap_default_provider = LabConfigInputs::default();
        assert!(!uses_resolved_chat_configuration(&clap_default_provider).unwrap());
    }

    #[test]
    fn legacy_serve_chat_matches_selected_model() {
        let legacy = ServeChatLegacyInputs {
            model_provider: ModelProvider::Bedrock,
            model: None,
            bedrock_model: Some("custom-bedrock".to_owned()),
        };
        let chat = serve_chat_settings(&LabConfigInputs::default(), &legacy).unwrap();
        assert_eq!(chat.provider, ModelProvider::Bedrock);
        assert_eq!(chat.model, "custom-bedrock");
    }

    #[tokio::test]
    async fn serve_agent_factory_receives_chat_once() {
        let recorder = crate::RecordingServeAgentFactory::new();
        let hook = recorder.clone();
        let legacy = ServeChatLegacyInputs {
            model_provider: ModelProvider::OpenAi,
            model: None,
            bedrock_model: None,
        };
        let expected = serve_chat_settings(&LabConfigInputs::default(), &legacy).unwrap();
        let store = crate::ConversationStore::open(
            &std::env::temp_dir().join(format!("serve-chat-factory-{}", std::process::id())),
            1,
        )
        .await
        .expect("store");
        let result = build_serve_lab_agent(
            &LabConfigInputs::default(),
            &legacy,
            "http://127.0.0.1:9/mcp",
            store,
            |provider, model, _mcp_url, _store| {
                let hook = hook.clone();
                async move {
                    hook.record(provider, &model)?;
                    Err::<crate::LabAgent, _>(A2aLabError::unavailable(
                        "test factory stops before provider",
                    ))
                }
            },
        )
        .await;
        assert!(result.is_err());
        assert_eq!(
            recorder.take_recorded().unwrap(),
            Some((expected.provider, expected.model))
        );
    }
}
