//! Merge file, environment, and CLI layers into validated settings.

use std::path::PathBuf;

use a2a_lab_dev_kit::{
    A2aLabError, ChatProvider, LoadedLabConfig, VideoInputFormat, VideoSourceConfig,
};

use crate::agent::{ModelProvider, selected_model};
use crate::camera::{
    CAMERA_SOURCE_ID, DEFAULT_EXTERNAL_CAMERA_DESCRIPTION, EXTERNAL_CAMERA_HEIGHT,
    EXTERNAL_CAMERA_SOURCE_ID, EXTERNAL_CAMERA_WIDTH,
};
use crate::videos::{
    DEFAULT_DISK_VIDEO_RETENTION_SECS, DEFAULT_VIDEO_DVR_ROOT, DEFAULT_VIDEO_FPS,
    DEFAULT_VIDEO_RETENTION_SECS,
};
use crate::videos::{
    LiveCameraInputFormat, LiveVideoDelivery, LiveVideoSourceDescriptor, validate_live_sources,
};

use super::inputs::{LabConfigInputs, SettingProvenance};

/// Resolved chat provider and model id.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedChatSettings {
    pub provider: ModelProvider,
    pub model: String,
}

/// Resolved DVR storage path and how it was chosen.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedDvrStorage {
    pub path: PathBuf,
    pub provenance: SettingProvenance,
}

/// Resolved video timing, storage, and source descriptors.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedVideoSettings {
    pub sources: Vec<LiveVideoSourceDescriptor>,
    pub fps: u32,
    pub live_retention_secs: u64,
    pub disk_retention_secs: u64,
    pub dvr_storage: ResolvedDvrStorage,
    pub max_image_bytes: u64,
}

/// Validated chat and video settings after precedence merge.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedLabConfig {
    pub chat: ResolvedChatSettings,
    pub video: ResolvedVideoSettings,
}

/// Merges optional loaded file content with CLI inputs and validates the result.
pub fn resolve(
    inputs: &LabConfigInputs,
    loaded: Option<&LoadedLabConfig>,
) -> Result<ResolvedLabConfig, A2aLabError> {
    let chat = resolve_chat(inputs, loaded);
    let video = resolve_video(inputs, loaded)?;
    Ok(ResolvedLabConfig { chat, video })
}

fn resolve_chat(
    inputs: &LabConfigInputs,
    loaded: Option<&LoadedLabConfig>,
) -> ResolvedChatSettings {
    let file_provider = loaded
        .and_then(|loaded| loaded.config.chat.as_ref())
        .and_then(|chat| chat.provider);
    let (provider, _) = inputs
        .model_provider
        .explicit_or(file_provider.map(chat_provider_to_model), || {
            ModelProvider::Bedrock
        });
    let file_model = loaded
        .and_then(|loaded| loaded.config.chat.as_ref())
        .and_then(|chat| chat.model.as_deref());
    let merged_model = merge_optional_string(&inputs.model, file_model);
    let bedrock_override = if inputs.bedrock_model.is_explicit() {
        inputs.bedrock_model.value.as_deref()
    } else {
        None
    };
    let model = selected_model(provider, merged_model.as_deref(), bedrock_override);
    ResolvedChatSettings { provider, model }
}

fn resolve_video(
    inputs: &LabConfigInputs,
    loaded: Option<&LoadedLabConfig>,
) -> Result<ResolvedVideoSettings, A2aLabError> {
    let file_videos = loaded.and_then(|loaded| loaded.config.videos.as_ref());
    let file_fps = file_videos.and_then(|videos| videos.fps);
    let (fps, _) = inputs.video_fps.explicit_or(file_fps, || DEFAULT_VIDEO_FPS);
    if fps == 0 {
        return Err(A2aLabError::invalid(
            "video_fps",
            "must be greater than zero",
        ));
    }

    let file_live = file_videos
        .and_then(|videos| videos.live_retention.as_ref())
        .map(|duration| duration.as_secs());
    let (live_retention_secs, _) = inputs
        .video_retention_secs
        .explicit_or(file_live, || DEFAULT_VIDEO_RETENTION_SECS);
    if live_retention_secs == 0 {
        return Err(A2aLabError::invalid(
            "video_retention_secs",
            "must be greater than zero",
        ));
    }

    let file_disk = file_videos
        .and_then(|videos| videos.dvr.as_ref())
        .and_then(|dvr| dvr.retention.as_ref())
        .map(|duration| duration.as_secs());
    let (disk_retention_secs, _) = inputs
        .video_disk_retention_secs
        .explicit_or(file_disk, || DEFAULT_DISK_VIDEO_RETENTION_SECS);
    if disk_retention_secs == 0 {
        return Err(A2aLabError::invalid(
            "video_disk_retention_secs",
            "must be greater than zero",
        ));
    }

    let dvr_storage = resolve_dvr_storage(inputs, loaded)?;
    let (max_image_bytes, _) = inputs
        .max_image_bytes
        .explicit_or(None, || a2a_lab_dev_kit::DEFAULT_MAX_IMAGE_BYTES);

    let mut sources = file_sources(loaded.and_then(|loaded| loaded.config.videos.clone()));
    apply_legacy_source_upserts(inputs, &mut sources)?;
    apply_description_override(inputs, &mut sources)?;
    validate_live_sources(&sources)?;

    Ok(ResolvedVideoSettings {
        sources,
        fps,
        live_retention_secs,
        disk_retention_secs,
        dvr_storage,
        max_image_bytes,
    })
}

fn resolve_dvr_storage(
    inputs: &LabConfigInputs,
    loaded: Option<&LoadedLabConfig>,
) -> Result<ResolvedDvrStorage, A2aLabError> {
    if inputs.video_dvr_root.is_explicit() {
        let path = inputs.video_dvr_root.value.clone();
        if path.as_os_str().is_empty() {
            return Err(A2aLabError::invalid("video_dvr_root", "must not be empty"));
        }
        return Ok(ResolvedDvrStorage {
            path,
            provenance: inputs
                .video_dvr_root
                .provenance()
                .unwrap_or(SettingProvenance::CommandLine),
        });
    }
    if let Some(path) = loaded.and_then(|loaded| loaded.dvr_storage_path.clone()) {
        if path.as_os_str().is_empty() {
            return Err(A2aLabError::invalid(
                "videos.dvr.storage_path",
                "must not be empty",
            ));
        }
        return Ok(ResolvedDvrStorage {
            path,
            provenance: SettingProvenance::File,
        });
    }
    Ok(ResolvedDvrStorage {
        path: PathBuf::from(DEFAULT_VIDEO_DVR_ROOT),
        provenance: SettingProvenance::ApplicationDefault,
    })
}

fn file_sources(videos: Option<a2a_lab_dev_kit::VideoConfig>) -> Vec<LiveVideoSourceDescriptor> {
    let Some(videos) = videos else {
        return Vec::new();
    };
    videos
        .sources
        .iter()
        .map(descriptor_from_file_source)
        .collect()
}

fn descriptor_from_file_source(source: &VideoSourceConfig) -> LiveVideoSourceDescriptor {
    let input = input_format(source.format);
    let id = source.id.as_str();
    match id {
        EXTERNAL_CAMERA_SOURCE_ID => LiveVideoSourceDescriptor {
            id: EXTERNAL_CAMERA_SOURCE_ID.to_owned(),
            name: source.display_name().to_owned(),
            description: source.display_description().to_owned(),
            device_index: source.device_index,
            input,
            width: EXTERNAL_CAMERA_WIDTH,
            height: EXTERNAL_CAMERA_HEIGHT,
            delivery: LiveVideoDelivery::H264,
        },
        CAMERA_SOURCE_ID => LiveVideoSourceDescriptor {
            id: CAMERA_SOURCE_ID.to_owned(),
            name: source.display_name().to_owned(),
            description: source.display_description().to_owned(),
            device_index: source.device_index,
            input,
            width: 1280,
            height: 720,
            delivery: LiveVideoDelivery::H264,
        },
        _ => LiveVideoSourceDescriptor {
            id: id.to_owned(),
            name: source.display_name().to_owned(),
            description: source.display_description().to_owned(),
            device_index: source.device_index,
            input,
            width: 1280,
            height: 720,
            delivery: LiveVideoDelivery::H264,
        },
    }
}

fn apply_legacy_source_upserts(
    inputs: &LabConfigInputs,
    sources: &mut Vec<LiveVideoSourceDescriptor>,
) -> Result<(), A2aLabError> {
    if inputs.external_camera_index.is_explicit() {
        let index = inputs.external_camera_index.value.ok_or_else(|| {
            A2aLabError::invalid("external_camera_index", "must be set when explicit")
        })?;
        if let Some(position) = sources
            .iter()
            .position(|source| source.id == EXTERNAL_CAMERA_SOURCE_ID)
        {
            let existing = sources[position].clone();
            sources[position] = LiveVideoSourceDescriptor {
                device_index: index,
                input: existing.input,
                ..existing
            };
        } else {
            sources.push(legacy_external_descriptor(
                index,
                LiveCameraInputFormat::Mjpeg,
                default_external_description(inputs),
            ));
        }
    }
    if inputs.camera_index.is_explicit() {
        let index = inputs
            .camera_index
            .value
            .ok_or_else(|| A2aLabError::invalid("camera_index", "must be set when explicit"))?;
        if let Some(position) = sources
            .iter()
            .position(|source| source.id == CAMERA_SOURCE_ID)
        {
            let existing = sources[position].clone();
            sources[position] = LiveVideoSourceDescriptor {
                device_index: index,
                input: existing.input,
                ..existing
            };
        } else {
            sources.push(legacy_camera_descriptor(
                index,
                LiveCameraInputFormat::Mjpeg,
            ));
        }
    }
    Ok(())
}

fn apply_description_override(
    inputs: &LabConfigInputs,
    sources: &mut [LiveVideoSourceDescriptor],
) -> Result<(), A2aLabError> {
    if !inputs.external_camera_description.is_explicit() {
        return Ok(());
    }
    let description = inputs.external_camera_description.value.trim();
    if description.is_empty() {
        return Err(A2aLabError::invalid(
            "external_camera_description",
            "must not be empty",
        ));
    }
    if let Some(source) = sources
        .iter_mut()
        .find(|source| source.id == EXTERNAL_CAMERA_SOURCE_ID)
    {
        description.clone_into(&mut source.description);
    }
    Ok(())
}

fn default_external_description(inputs: &LabConfigInputs) -> String {
    if inputs.external_camera_description.is_explicit() {
        inputs.external_camera_description.value.clone()
    } else {
        DEFAULT_EXTERNAL_CAMERA_DESCRIPTION.to_owned()
    }
}

fn legacy_external_descriptor(
    index: u32,
    input: LiveCameraInputFormat,
    description: String,
) -> LiveVideoSourceDescriptor {
    LiveVideoSourceDescriptor {
        id: EXTERNAL_CAMERA_SOURCE_ID.to_owned(),
        name: "External camera".to_owned(),
        description,
        device_index: index,
        input,
        width: EXTERNAL_CAMERA_WIDTH,
        height: EXTERNAL_CAMERA_HEIGHT,
        delivery: LiveVideoDelivery::H264,
    }
}

fn legacy_camera_descriptor(index: u32, input: LiveCameraInputFormat) -> LiveVideoSourceDescriptor {
    LiveVideoSourceDescriptor {
        id: CAMERA_SOURCE_ID.to_owned(),
        name: "Camera".to_owned(),
        description: "Live MJPEG from the configured camera.".to_owned(),
        device_index: index,
        input,
        width: 1280,
        height: 720,
        delivery: LiveVideoDelivery::H264,
    }
}

fn merge_optional_string(
    cli: &super::inputs::ExplicitSetting<Option<String>>,
    file: Option<&str>,
) -> Option<String> {
    if cli.is_explicit() {
        return cli
            .value
            .as_deref()
            .map(str::trim)
            .filter(|text| !text.is_empty())
            .map(str::to_owned);
    }
    file.map(str::trim)
        .filter(|text| !text.is_empty())
        .map(str::to_owned)
}

fn chat_provider_to_model(provider: ChatProvider) -> ModelProvider {
    match provider {
        ChatProvider::Bedrock => ModelProvider::Bedrock,
        ChatProvider::Openai => ModelProvider::OpenAi,
        ChatProvider::Anthropic => ModelProvider::Anthropic,
    }
}

const fn input_format(format: VideoInputFormat) -> LiveCameraInputFormat {
    match format {
        VideoInputFormat::Mjpeg => LiveCameraInputFormat::Mjpeg,
        VideoInputFormat::Yuy2 => LiveCameraInputFormat::Yuy2,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lab_config::inputs::ExplicitSetting;

    #[test]
    fn legacy_upsert_preserves_file_input_format() {
        let mut sources = vec![LiveVideoSourceDescriptor {
            id: CAMERA_SOURCE_ID.to_owned(),
            name: "Camera".to_owned(),
            description: "from file".to_owned(),
            device_index: 1,
            input: LiveCameraInputFormat::Yuy2,
            width: 1280,
            height: 720,
            delivery: LiveVideoDelivery::Mjpeg,
        }];
        let inputs = LabConfigInputs {
            camera_index: ExplicitSetting::cli(Some(9)),
            ..LabConfigInputs::default()
        };
        apply_legacy_source_upserts(&inputs, &mut sources).unwrap();
        assert_eq!(sources.len(), 1);
        assert_eq!(sources[0].device_index, 9);
        assert_eq!(sources[0].input, LiveCameraInputFormat::Yuy2);
    }
}
