//! Live video catalog plus a cancellable capture pump.

mod catalog;
pub mod dvr;
mod external;
mod h264;
mod nokhwa_camera;
mod raw_jpeg;
mod serve;
mod source;

pub use catalog::{
    ExternalVideoConfig, LiveVideoCatalog, LiveVideoInjections, WebcamVideoConfig,
    start_live_video_catalog, start_live_videos,
};
pub use dvr::{
    DEFAULT_DISK_VIDEO_RETENTION_SECS, DEFAULT_VIDEO_DVR_ROOT, IndexedVideoDvr,
    VIDEO_DVR_CLEANUP_INTERVAL, VideoDvrCatalog, VideoDvrMaintenance,
};
pub use external::LiveJpegSource;
pub use serve::{
    DEFAULT_LIVE_VIDEO_MEMORY_SECS, DEFAULT_VIDEO_FPS, DEFAULT_VIDEO_RETENTION_SECS,
    VideoServeConfig, open_resolved_video_catalog, open_video_catalog, resolved_video_summary,
};
pub use source::{
    LiveCameraInputFormat, LiveVideoDelivery, LiveVideoSourceDescriptor, validate_live_sources,
};
