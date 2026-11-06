//! Side-effect-free resolution of chat and video settings from file, environment, and CLI.

mod inputs;
mod resolve;
mod selection;

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

use a2a_lab_dev_kit::A2aLabError;

/// Loads the selected file when present and returns merged, validated settings.
pub fn resolve_lab_config(inputs: &LabConfigInputs) -> Result<ResolvedLabConfig, A2aLabError> {
    let path = selected_config_path(inputs)?;
    let loaded = match path.as_deref() {
        Some(path) => Some(a2a_lab_dev_kit::LoadedLabConfig::load(path)?),
        None => None,
    };
    resolve(inputs, loaded.as_ref())
}

#[cfg(test)]
mod tests;
