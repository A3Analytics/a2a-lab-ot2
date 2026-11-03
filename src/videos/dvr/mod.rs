//! Live memory window plus indexed disk DVR.

pub mod chunk;
mod coverage;
mod provider;
mod store;

pub use provider::{VideoDvrCatalog, VideoDvrMaintenance};
pub use store::{
    DEFAULT_DISK_VIDEO_RETENTION_SECS, DEFAULT_VIDEO_DVR_ROOT, IndexedVideoDvr,
    VIDEO_DVR_CLEANUP_INTERVAL,
};
