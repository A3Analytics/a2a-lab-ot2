//! Example lab agent wrapping an Opentrons OT-2 robot-server simulator.

pub mod agent;
pub mod camera;
pub mod cameras;
pub mod fixture;
pub mod frame_set_service;
pub mod frame_sets;
pub mod images;
pub mod lab_config;
pub mod memory;
pub mod oidc;
pub mod opentrons;
pub mod orbbec;
mod orbbec_frames;
pub mod page;
pub mod sila;
pub mod time;
pub mod videos;

pub use agent::{
    DEFAULT_ANTHROPIC_MODEL, DEFAULT_BEDROCK_MODEL, DEFAULT_OPENAI_MODEL, LabAgent, ModelProvider,
    RecordingServeAgentFactory, selected_model,
};
pub use cameras::{OrbbecStream, SessionPump, SharedSession, StreamingDevice};
pub use fixture::{
    FixtureConfig, FixtureReadiness, FixtureServer, FixtureVariant, fixture_declaration,
};
pub use frame_sets::{
    DEFAULT_SYNC_CHUNK_ROOT, DEFAULT_SYNC_DB, SyncRecorder, SyncStore, validate_sync_paths,
};
pub use images::{
    CapturedFrame, DEFAULT_CAPTURE_TIMEOUT_MS, DEFAULT_IMAGE_HISTORY_DB,
    DEFAULT_IMAGE_RETENTION_PER_SOURCE, IMAGE_HISTORY_MAX_AGE, ImageCapture, ImageCatalogConfig,
    ImageHistoryMaintenance, ImageServeConfig, LiveImageCatalog, image_catalog, open_image_catalog,
};
pub use lab_config::{
    ClapValueSource, ExplicitSetting, LabConfigCliValues, LabConfigInputs, ResolvedChatSettings,
    ResolvedDvrStorage, ResolvedLabConfig, ResolvedVideoSettings, ServeChatLegacyInputs,
    SettingProvenance, build_serve_lab_agent, resolve_lab_config, selected_config_path,
    serve_chat_settings, uses_resolved_chat_configuration, uses_resolved_video_configuration,
};
pub use memory::ConversationStore;
pub use oidc::{OidcConfig, with_oidc};
pub use opentrons::{OpentronsClient, OpentronsLab};
pub use sila::{
    DEFAULT_SILA_PORT, DEFAULT_SILA_UUID, PreparedSila, SilaConfig, prepare_sila, sila_server,
};
pub use videos::{
    DEFAULT_DISK_VIDEO_RETENTION_SECS, DEFAULT_LIVE_VIDEO_MEMORY_SECS, DEFAULT_VIDEO_DVR_ROOT,
    DEFAULT_VIDEO_FPS, DEFAULT_VIDEO_RETENTION_SECS, ExternalVideoConfig, IndexedVideoDvr,
    LiveCameraInputFormat, LiveJpegSource, LiveVideoCatalog, LiveVideoDelivery,
    LiveVideoInjections, LiveVideoSourceDescriptor, VideoDvrCatalog, VideoServeConfig,
    WebcamVideoConfig, open_resolved_video_catalog, open_video_catalog, resolved_video_summary,
    start_live_video_catalog, start_live_videos,
};
