//! Resolved semantic-indexing settings for serve startup.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use a2a_lab_dev_kit::{A2aLabError, LoadedLabConfig, SemanticIndexingConfig};

use super::inputs::LabConfigInputs;
use super::resolve::ResolvedVideoSettings;
use super::selected_config_path;

/// Semantic indexing resolved from v2 file configuration when present.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedSemanticIndexing {
    pub config: SemanticIndexingConfig,
    pub vector_index_path: PathBuf,
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
    let schedules = loaded
        .config
        .videos
        .as_ref()
        .map(|videos| {
            videos
                .sources
                .iter()
                .map(|source| {
                    source.semantic_indexing.clone().unwrap_or_else(|| {
                        a2a_lab_dev_kit::SemanticStreamSchedule::every_saved_frame(
                            source.id.as_str(),
                        )
                    })
                })
                .collect()
        })
        .unwrap_or_default();
    let config = config.with_schedules(schedules);
    let vector_index_path = loaded.semantic_vector_index_path.ok_or_else(|| {
        A2aLabError::invalid(
            "semantic_indexing.limits.storage.vector_index_path",
            "required when semantic_indexing is enabled",
        )
    })?;
    validate_vector_storage(&vector_index_path)?;
    let indexed_source_ids = video
        .sources
        .iter()
        .map(|source| source.id.clone())
        .collect::<HashSet<_>>();
    Ok(Some(ResolvedSemanticIndexing {
        config,
        vector_index_path,
        indexed_source_ids,
    }))
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
