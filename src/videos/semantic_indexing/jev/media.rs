//! Loads original saved frame and operator reference stills for Jev.

use std::collections::HashMap;
use std::sync::Arc;

use a2a_lab_dev_kit::{
    FrameDecisionEvaluationFailure, FrameRoiTransform, SavedFrameRef, Sha256Digest,
};

use crate::videos::dvr::IndexedVideoDvr;
use crate::videos::dvr::frame_evidence::{FrameEvidenceLimits, FrameEvidenceRequest};

/// Supplies exact PNG/JPEG bytes for frame and operator reference evidence.
pub trait JevDecisionMedia: Send + Sync {
    /// Oriented still for one saved frame revision (optionally ROI-cropped).
    fn frame_still(
        &self,
        frame: &SavedFrameRef,
        roi: FrameRoiTransform,
    ) -> impl std::future::Future<Output = Result<Vec<u8>, FrameDecisionEvaluationFailure>> + Send;

    /// Operator-authored reference still by content digest.
    fn reference_still(
        &self,
        digest: &Sha256Digest,
    ) -> impl std::future::Future<Output = Result<Vec<u8>, FrameDecisionEvaluationFailure>> + Send;
}

/// In-memory map for deterministic HTTP fixtures.
#[derive(Debug, Default, Clone)]
pub struct InjectedJevDecisionMedia {
    frames: HashMap<String, Vec<u8>>,
    references: HashMap<String, Vec<u8>>,
}

impl InjectedJevDecisionMedia {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    pub fn insert_frame(&mut self, frame: &SavedFrameRef, jpeg: Vec<u8>) {
        self.frames
            .insert(frame.frame_id().as_str().to_owned(), jpeg);
    }

    pub fn insert_reference(&mut self, digest: &Sha256Digest, jpeg: Vec<u8>) {
        self.references.insert(digest.as_str().to_owned(), jpeg);
    }
}

impl JevDecisionMedia for InjectedJevDecisionMedia {
    fn frame_still(
        &self,
        frame: &SavedFrameRef,
        _roi: FrameRoiTransform,
    ) -> impl std::future::Future<Output = Result<Vec<u8>, FrameDecisionEvaluationFailure>> + Send
    {
        let frame_id = frame.frame_id().as_str().to_owned();
        let frames = self.frames.clone();
        async move {
            frames
                .get(&frame_id)
                .cloned()
                .ok_or_else(|| FrameDecisionEvaluationFailure::InvalidEvidence {
                    message: format!("no injected frame still for {frame_id}"),
                })
        }
    }

    fn reference_still(
        &self,
        digest: &Sha256Digest,
    ) -> impl std::future::Future<Output = Result<Vec<u8>, FrameDecisionEvaluationFailure>> + Send
    {
        let key = digest.as_str().to_owned();
        let references = self.references.clone();
        async move {
            references
                .get(&key)
                .cloned()
                .ok_or_else(|| FrameDecisionEvaluationFailure::InvalidEvidence {
                    message: format!("no injected reference for {key}"),
                })
        }
    }
}

/// Encodes retained DVR evidence as JPEG for live Jev calls.
pub struct EvidenceJpegDecisionMedia {
    dvr: Arc<IndexedVideoDvr>,
    evidence_limits: FrameEvidenceLimits,
    jpeg_quality: u8,
}

impl EvidenceJpegDecisionMedia {
    #[must_use]
    pub fn new(dvr: Arc<IndexedVideoDvr>) -> Self {
        Self {
            dvr,
            evidence_limits: FrameEvidenceLimits::standard(),
            jpeg_quality: 90,
        }
    }
}

#[allow(clippy::unused_async)]
impl JevDecisionMedia for EvidenceJpegDecisionMedia {
    async fn frame_still(
        &self,
        frame: &SavedFrameRef,
        roi: FrameRoiTransform,
    ) -> Result<Vec<u8>, FrameDecisionEvaluationFailure> {
        let pixels = self
            .dvr
            .read_bounded_frame_evidence(
                FrameEvidenceRequest::new(frame, Some(roi)),
                self.evidence_limits,
            )
            .await
            .map_err(|error| FrameDecisionEvaluationFailure::InvalidEvidence {
                message: error.to_string(),
            })?;
        let mut buffer = Vec::new();
        let width = pixels.width();
        let height = pixels.height();
        image::codecs::jpeg::JpegEncoder::new_with_quality(&mut buffer, self.jpeg_quality)
            .encode(pixels.rgb(), width, height, image::ExtendedColorType::Rgb8)
            .map_err(|error| FrameDecisionEvaluationFailure::InvalidEvidence {
                message: format!("jpeg encode failed: {error}"),
            })?;
        Ok(buffer)
    }

    fn reference_still(
        &self,
        digest: &Sha256Digest,
    ) -> impl std::future::Future<Output = Result<Vec<u8>, FrameDecisionEvaluationFailure>> + Send
    {
        let _ = digest;
        async {
            Err(FrameDecisionEvaluationFailure::InvalidEvidence {
                message: "operator reference stills require injected media or fixture store"
                    .to_owned(),
            })
        }
    }
}
