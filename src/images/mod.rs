//! Process-local live image catalog over injectable capture backends.

mod catalog;
mod jpeg;
mod serve;

pub(crate) use jpeg::jpeg_dimensions;

pub use catalog::{
    CapturedFrame, DEFAULT_IMAGE_RETENTION_PER_SOURCE, ImageCapture, ImageCatalogConfig,
    LiveImageCatalog,
};
pub use serve::{DEFAULT_CAPTURE_TIMEOUT_MS, ImageServeConfig, image_catalog};
