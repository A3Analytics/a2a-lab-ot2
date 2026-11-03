//! Live video catalog plus a cancellable capture pump.

mod catalog;
pub mod dvr;
mod external;
mod serve;

pub use catalog::{
    ExternalVideoConfig, LiveVideoCatalog, LiveVideoInjections, OrbbecVideoConfig,
    start_live_videos,
};
pub use dvr::{
    DEFAULT_DISK_VIDEO_RETENTION_SECS, DEFAULT_VIDEO_DVR_ROOT, IndexedVideoDvr,
    VIDEO_DVR_CLEANUP_INTERVAL, VideoDvrCatalog, VideoDvrMaintenance,
};
pub use external::LiveJpegSource;
pub use serve::{
    DEFAULT_LIVE_VIDEO_MEMORY_SECS, DEFAULT_VIDEO_FPS, DEFAULT_VIDEO_RETENTION_SECS,
    VideoServeConfig, open_video_catalog,
};
