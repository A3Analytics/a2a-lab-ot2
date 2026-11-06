//! Injectable JPEG producers and live frame pacing.

use std::time::Duration;

use a2a_lab_dev_kit::A2aLabError;

/// Pulls encoded JPEG frames for the live video pump.
pub trait LiveJpegSource: Send {
    /// Returns one encoded JPEG.
    fn next_jpeg(&mut self) -> Result<Vec<u8>, A2aLabError>;
}

/// Sleeps between live frames so the catalog stays near `fps`.
#[must_use]
pub fn frame_budget(fps: u32) -> Duration {
    Duration::from_millis(1_000 / u64::from(fps.max(1)))
}
