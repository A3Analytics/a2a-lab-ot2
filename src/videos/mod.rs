//! Live video catalog plus a cancellable capture pump.

pub(crate) mod capture_open;
mod catalog;
pub mod dvr;
mod dvr_cadence;
mod dyn_camera;
mod external;
mod live_encode;
mod native_drain;
mod nokhwa_camera;
mod raw_jpeg;
pub mod semantic_eval;
pub mod semantic_indexing;
mod serve;
mod session_capture;
mod source;

pub use catalog::{
    ExternalVideoConfig, LiveVideoCatalog, LiveVideoInjections, WebcamVideoConfig,
    start_live_video_catalog, start_live_videos,
};
pub use dvr::{
    ANNOTATION_RECONCILE_INTERVAL, DEFAULT_DISK_VIDEO_RETENTION_SECS, DEFAULT_VIDEO_DVR_ROOT,
    IndexedSourceFilter, IndexedVideoDvr, VIDEO_DVR_CLEANUP_INTERVAL, VideoDvrAnnotationQueue,
    VideoDvrCatalog, VideoDvrMaintenance,
};
pub use external::LiveJpegSource;
pub use live_encode::LiveH264Catalog;
pub use serve::{
    DEFAULT_LIVE_VIDEO_MEMORY_SECS, DEFAULT_VIDEO_FPS, DEFAULT_VIDEO_RETENTION_SECS,
    VideoServeConfig, open_resolved_video_catalog, open_resolved_video_catalog_with_semantic,
    open_video_catalog, resolve_serve_semantic_indexing, resolved_video_summary,
};
pub use source::{
    LiveCameraInputFormat, LiveVideoDelivery, LiveVideoSourceDescriptor,
    native_capture_profile_candidates, validate_live_sources,
};
