//! Resolved semantic-indexing settings for serve startup.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use a2a_lab_dev_kit::{
    A2aLabError, LoadedLabConfig, ResolvedImageDecisionClient, SemanticIndexingConfig,
};

use super::inputs::LabConfigInputs;
use super::resolve::ResolvedVideoSettings;
use super::selected_config_path;

/// Semantic indexing resolved from v2 file configuration when present.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedSemanticIndexing {
    pub config: SemanticIndexingConfig,
    pub vector_index_path: PathBuf,
    pub sqlite_path: PathBuf,
    pub indexed_source_ids: HashSet<String>,
}

/// Loads semantic settings when the selected file includes `semantic_indexing`.
pub fn resolve_semantic_indexing(
    inputs: &LabConfigInputs,
    video: &ResolvedVideoSettings,
) -> Result<Option<ResolvedSemanticIndexing>, A2aLabError> {
    let Some(path) = selected_config_path(inputs)? else {
        return Ok(None);
    };
    let loaded = LoadedLabConfig::load(&path)?;
    let Some(config) = loaded.config.semantic_indexing else {
        return Ok(None);
    };
    let config = if let Some(videos) = loaded.config.videos.as_ref() {
        a2a_lab_dev_kit::attach_stream_schedules(config, &videos.sources)
    } else {
        config
    };
    config.validate_before_startup()?;
    let vector_index_path = loaded.semantic_vector_index_path.ok_or_else(|| {
        A2aLabError::invalid(
            "semantic_indexing.limits.storage.vector_index_path",
            "required when semantic_indexing is enabled",
        )
    })?;
    validate_vector_storage(&vector_index_path)?;
    let sqlite_path = loaded
        .semantic_sqlite_path
        .unwrap_or_else(|| video.dvr_storage.path.join("semantic.sqlite"));
    let indexed_source_ids = video
        .sources
        .iter()
        .map(|source| source.id.clone())
        .collect::<HashSet<_>>();
    Ok(Some(ResolvedSemanticIndexing {
        config,
        vector_index_path,
        sqlite_path,
        indexed_source_ids,
    }))
}

/// Loads the generic synchronous image-decision client when the lab file enables it.
///
/// Absent configuration returns `Ok(None)` and leaves typed decision workers alone.
/// Invalid settings and a named credential that is not set return the devkit error.
pub fn resolve_image_decision_client(
    inputs: &LabConfigInputs,
) -> Result<Option<ResolvedImageDecisionClient>, A2aLabError> {
    let Some(path) = selected_config_path(inputs)? else {
        return Ok(None);
    };
    let loaded = LoadedLabConfig::load(&path)?;
    let Some(config) = loaded.config.image_decision else {
        return Ok(None);
    };
    let client = ResolvedImageDecisionClient::from_config(config);
    ensure_image_decision_client_available(&client)?;
    Ok(Some(client))
}

fn ensure_image_decision_client_available(
    client: &ResolvedImageDecisionClient,
) -> Result<(), A2aLabError> {
    if client.config().agentcore().is_some() {
        return Ok(());
    }
    if let Some(name) = client.config().credentials_env() {
        let configured = std::env::var(name).is_ok_and(|value| !value.trim().is_empty());
        if !configured {
            return Err(A2aLabError::unavailable(format!(
                "image decision credentials not configured (env:{name})"
            )));
        }
    }
    client.executor().map(|_| ())
}

fn validate_vector_storage(path: &Path) -> Result<(), A2aLabError> {
    if path.as_os_str().is_empty() {
        return Err(A2aLabError::invalid(
            "semantic_indexing.limits.storage.vector_index_path",
            "must not be empty",
        ));
    }
    let parent = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    std::fs::create_dir_all(parent).map_err(|error| {
        A2aLabError::invalid(
            "semantic_indexing.limits.storage.vector_index_path",
            format!("cannot create parent {}: {error}", parent.display()),
        )
    })?;
    Ok(())
}
