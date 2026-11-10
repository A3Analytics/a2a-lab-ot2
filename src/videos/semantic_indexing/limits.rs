//! Maps v2 semantic worker limits into provider limit structs.

use std::time::Duration;

use a2a_lab_dev_kit::{FrameDescriptionLimits, SemanticIndexingLimits, TextEmbeddingLimits};

/// Frame-description limits from validated v2 semantic configuration.
#[must_use]
pub fn frame_description_limits(limits: &SemanticIndexingLimits) -> FrameDescriptionLimits {
    let worker = &limits.worker;
    FrameDescriptionLimits::new(
        worker.max_batch_items,
        16 * 1024 * 1024,
        u64::from(limits.roi.max_edge_pixels) * u64::from(limits.roi.max_edge_pixels),
        16 * 1024,
        512,
        Duration::from_secs(worker.job_deadline.as_secs()),
    )
    .unwrap_or_else(|_| FrameDescriptionLimits::pilot())
}

/// Text-embedding limits from validated v2 semantic configuration.
#[must_use]
pub fn text_embedding_limits(limits: &SemanticIndexingLimits) -> TextEmbeddingLimits {
    let worker = &limits.worker;
    TextEmbeddingLimits::new(
        worker.max_batch_items,
        32 * 1024,
        512,
        Duration::from_secs(worker.job_deadline.as_secs()),
    )
    .unwrap_or_else(|_| TextEmbeddingLimits::pilot())
}
