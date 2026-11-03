//! Live video catalog over injectable JPEG producers.

mod catalog;
mod external;
mod serve;

pub use catalog::{
    ExternalVideoConfig, LiveVideoCatalog, LiveVideoInjections, OrbbecVideoConfig,
    start_live_videos,
};
pub use external::LiveJpegSource;
pub use serve::{
    DEFAULT_VIDEO_FPS, DEFAULT_VIDEO_RETENTION_SECS, VideoServeConfig, open_video_catalog,
};
