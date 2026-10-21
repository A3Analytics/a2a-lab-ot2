//! Live image catalog over injectable capture backends.

mod catalog;
mod history;
mod jpeg;
mod serve;

pub(crate) use jpeg::jpeg_dimensions;

pub use catalog::{
    CapturedFrame, DEFAULT_IMAGE_RETENTION_PER_SOURCE, ImageCapture, ImageCatalogConfig,
    ImageHistoryMaintenance, LiveImageCatalog,
};
pub use history::{DEFAULT_IMAGE_HISTORY_DB, IMAGE_HISTORY_MAX_AGE};
pub use serve::{DEFAULT_CAPTURE_TIMEOUT_MS, ImageServeConfig, image_catalog, open_image_catalog};
