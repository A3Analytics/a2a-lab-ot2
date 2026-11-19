//! Side-effect-free resolution of chat and video settings from file, environment, and CLI.

mod inputs;
mod resolve;
mod selection;
mod semantic;

pub use inputs::{
    ClapValueSource, ExplicitSetting, LabConfigCliValues, LabConfigInputs, SettingProvenance,
};
pub use resolve::{
    ResolvedChatSettings, ResolvedDvrStorage, ResolvedLabConfig, ResolvedVideoSettings, resolve,
};
pub use selection::{
    ServeChatLegacyInputs, build_serve_lab_agent, selected_config_path, serve_chat_settings,
    uses_resolved_chat_configuration, uses_resolved_video_configuration,
};
pub use semantic::{
    ResolvedSemanticIndexing, resolve_image_decision_client, resolve_semantic_indexing,
};

use a2a_lab_dev_kit::{A2aLabError, LoadedLabConfig, LoggingGuard, init_logging_from_loaded};

/// One-shot file load, settings merge, and optional application logging install.
#[derive(Debug)]
pub struct LabConfigBootstrap {
    /// Merged chat and video settings from the selected file.
    pub resolved: ResolvedLabConfig,
    logging: Option<LoggingGuard>,
}

impl LabConfigBootstrap {
    /// Returns whether a dev-kit logging guard is held for this process.
    #[must_use]
    pub const fn logging_enabled(&self) -> bool {
        self.logging.is_some()
    }
}

/// Loads the selected config file, validates merged settings, and installs logging when configured.
///
/// Call only after [`selected_config_path`] is `Some`. When the file omits `logging`, no subscriber
/// is installed and this returns `logging_enabled() == false`.
pub fn bootstrap_lab_config(inputs: &LabConfigInputs) -> Result<LabConfigBootstrap, A2aLabError> {
    let path = selected_config_path(inputs)?
        .ok_or_else(|| A2aLabError::invalid("config", "configuration file is not selected"))?;
    let loaded = LoadedLabConfig::load(&path)?;
    let resolved = resolve(inputs, Some(&loaded))?;
    resolve_image_decision_client(inputs)?;
    let logging = init_logging_from_loaded(&loaded)?;
    Ok(LabConfigBootstrap { resolved, logging })
}

/// Uses a prior bootstrap result when present to avoid reloading the configuration file.
pub fn resolved_lab_config(
    bootstrap: Option<&LabConfigBootstrap>,
    inputs: &LabConfigInputs,
) -> Result<ResolvedLabConfig, A2aLabError> {
    if let Some(bootstrap) = bootstrap {
        return Ok(bootstrap.resolved.clone());
    }
    resolve_lab_config(inputs)
}

/// Loads the selected file when present and returns merged, validated settings.
pub fn resolve_lab_config(inputs: &LabConfigInputs) -> Result<ResolvedLabConfig, A2aLabError> {
    let path = selected_config_path(inputs)?;
    let loaded = match path.as_deref() {
        Some(path) => Some(LoadedLabConfig::load(path)?),
        None => None,
    };
    resolve(inputs, loaded.as_ref())
}

#[cfg(test)]
mod tests;
