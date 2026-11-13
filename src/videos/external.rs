//! Injectable JPEG producers and live frame pacing.

use a2a_lab_dev_kit::A2aLabError;

/// Pulls encoded JPEG frames for the live video pump.
pub trait LiveJpegSource: Send {
    /// Returns one encoded JPEG.
    fn next_jpeg(&mut self) -> Result<Vec<u8>, A2aLabError>;
}
