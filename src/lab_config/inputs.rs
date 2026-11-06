//! CLI and environment inputs with explicitness metadata for precedence.

use std::path::PathBuf;

use crate::agent::ModelProvider;

/// Clap [`ValueSource`](clap::ValueSource) categories stored without a library Clap dependency.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClapValueSource {
    /// Explicit `--flag`.
    CommandLine,
    /// Explicit environment variable.
    Environment,
    /// Clap `default_value` / `default_value_t`.
    DefaultValue,
}

/// Where a resolved value originated after merge.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SettingProvenance {
    /// Explicit `--flag`.
    CommandLine,
    /// Explicit environment variable (Clap `env =`).
    Environment,
    /// Selected configuration file.
    File,
    /// Application default when nothing else supplied the value.
    ApplicationDefault,
}

/// One setting plus whether CLI or environment explicitly set it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExplicitSetting<T> {
    pub value: T,
    pub source: Option<ClapValueSource>,
}

impl<T> ExplicitSetting<T> {
    pub fn cli(value: T) -> Self {
        Self {
            value,
            source: Some(ClapValueSource::CommandLine),
        }
    }

    pub fn env(value: T) -> Self {
        Self {
            value,
            source: Some(ClapValueSource::Environment),
        }
    }

    pub fn default_value(value: T) -> Self {
        Self {
            value,
            source: Some(ClapValueSource::DefaultValue),
        }
    }

    pub fn unset(value: T) -> Self {
        Self {
            value,
            source: None,
        }
    }

    pub fn is_explicit(&self) -> bool {
        matches!(
            self.source,
            Some(ClapValueSource::CommandLine | ClapValueSource::Environment)
        )
    }

    pub fn provenance(&self) -> Option<SettingProvenance> {
        match self.source {
            Some(ClapValueSource::CommandLine) => Some(SettingProvenance::CommandLine),
            Some(ClapValueSource::Environment) => Some(SettingProvenance::Environment),
            Some(ClapValueSource::DefaultValue) | None => None,
        }
    }

    pub fn explicit_or<F>(&self, file: Option<T>, default: F) -> (T, SettingProvenance)
    where
        T: Clone,
        F: FnOnce() -> T,
    {
        if self.is_explicit() {
            return (
                self.value.clone(),
                self.provenance()
                    .unwrap_or(SettingProvenance::ApplicationDefault),
            );
        }
        if let Some(value) = file {
            return (value, SettingProvenance::File);
        }
        (default(), SettingProvenance::ApplicationDefault)
    }
}

/// Parsed CLI values needed for configuration resolution.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LabConfigCliValues {
    pub config: Option<PathBuf>,
    pub model_provider: ModelProvider,
    pub model: Option<String>,
    pub bedrock_model: Option<String>,
    pub external_camera_index: Option<u32>,
    pub external_camera_description: String,
    pub camera_index: Option<u32>,
    pub video_fps: u32,
    pub video_retention_secs: u64,
    pub video_disk_retention_secs: u64,
    pub video_dvr_root: PathBuf,
    pub max_image_bytes: u64,
}

/// Raw inputs for [`super::resolve_lab_config`]. Construct in tests without Clap races.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LabConfigInputs {
    pub config_path: ExplicitSetting<Option<PathBuf>>,
    pub model_provider: ExplicitSetting<ModelProvider>,
    pub model: ExplicitSetting<Option<String>>,
    pub bedrock_model: ExplicitSetting<Option<String>>,
    pub external_camera_index: ExplicitSetting<Option<u32>>,
    pub external_camera_description: ExplicitSetting<String>,
    pub camera_index: ExplicitSetting<Option<u32>>,
    pub video_fps: ExplicitSetting<u32>,
    pub video_retention_secs: ExplicitSetting<u64>,
    pub video_disk_retention_secs: ExplicitSetting<u64>,
    pub video_dvr_root: ExplicitSetting<PathBuf>,
    pub max_image_bytes: ExplicitSetting<u64>,
}

impl Default for LabConfigInputs {
    fn default() -> Self {
        use crate::camera::DEFAULT_EXTERNAL_CAMERA_DESCRIPTION;
        use crate::videos::{
            DEFAULT_DISK_VIDEO_RETENTION_SECS, DEFAULT_VIDEO_DVR_ROOT, DEFAULT_VIDEO_FPS,
            DEFAULT_VIDEO_RETENTION_SECS,
        };
        Self {
            config_path: ExplicitSetting::unset(None),
            model_provider: ExplicitSetting::default_value(ModelProvider::Bedrock),
            model: ExplicitSetting::unset(None),
            bedrock_model: ExplicitSetting::unset(None),
            external_camera_index: ExplicitSetting::unset(None),
            external_camera_description: ExplicitSetting::default_value(
                DEFAULT_EXTERNAL_CAMERA_DESCRIPTION.to_owned(),
            ),
            camera_index: ExplicitSetting::unset(None),
            video_fps: ExplicitSetting::default_value(DEFAULT_VIDEO_FPS),
            video_retention_secs: ExplicitSetting::default_value(DEFAULT_VIDEO_RETENTION_SECS),
            video_disk_retention_secs: ExplicitSetting::default_value(
                DEFAULT_DISK_VIDEO_RETENTION_SECS,
            ),
            video_dvr_root: ExplicitSetting::default_value(PathBuf::from(DEFAULT_VIDEO_DVR_ROOT)),
            max_image_bytes: ExplicitSetting::default_value(
                a2a_lab_dev_kit::DEFAULT_MAX_IMAGE_BYTES,
            ),
        }
    }
}
