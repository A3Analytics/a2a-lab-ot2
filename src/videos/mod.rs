//! Declarative video runtime wiring and OT-2 DVR semantic extensions.

mod declarative_serve;
pub mod dvr;
mod external;
mod injections;
mod jpeg_camera;
pub mod semantic_eval;
pub mod semantic_indexing;
mod serve;
mod source;

pub use declarative_serve::DeclarativeVideoServe;
pub use dvr::{
    ANNOTATION_RECONCILE_INTERVAL, DEFAULT_DISK_VIDEO_RETENTION_SECS, DEFAULT_VIDEO_DVR_ROOT,
    IndexedSourceFilter, IndexedVideoDvr, VIDEO_DVR_CLEANUP_INTERVAL, VideoDvrAnnotationQueue,
    VideoDvrCatalog, VideoDvrMaintenance,
};
pub use external::LiveJpegSource;
pub use injections::{ExternalVideoConfig, LiveVideoInjections, WebcamVideoConfig};
pub use serve::{
    DEFAULT_LIVE_VIDEO_MEMORY_SECS, DEFAULT_VIDEO_FPS, DEFAULT_VIDEO_RETENTION_SECS,
    VideoServeConfig, open_resolved_video_catalog, open_resolved_video_catalog_with_semantic,
    open_video_catalog, resolve_serve_semantic_indexing, resolved_video_summary,
    start_fixture_declarative_serve, start_legacy_fixture_videos,
};
pub use source::{
    LiveCameraInputFormat, LiveVideoDelivery, LiveVideoSourceDescriptor, index_camera_fields,
    native_capture_profile_candidates, validate_live_sources,
};
pub mod archived;
