//! Injects oriented JPEG bytes for Luna description requests.

use std::collections::HashMap;
use std::sync::Arc;

use a2a_lab_dev_kit::videos::SemanticIndexingFailure;
use a2a_lab_dev_kit::{FrameDescriptionRequest, SavedFrameRef};
use image::codecs::jpeg::JpegEncoder;
use image::{ExtendedColorType, ImageEncoder, RgbImage};

use crate::videos::dvr::IndexedVideoDvr;
use crate::videos::dvr::frame_evidence::{FrameEvidenceLimits, FrameEvidenceRequest};

/// Supplies JPEG inputs keyed by frame revision (separate from the dev-kit request struct).
pub trait FrameDescriptionMedia: Send + Sync {
    /// Loads a bounded JPEG for one description key.
    fn jpeg_for(
        &self,
        request: &FrameDescriptionRequest,
    ) -> impl std::future::Future<Output = Result<Vec<u8>, SemanticIndexingFailure>> + Send;
}

/// In-memory JPEG map for deterministic tests and smoke fixtures.
#[derive(Debug, Default, Clone)]
pub struct InjectedDescriptionMedia {
    frames: HashMap<String, Vec<u8>>,
}

impl InjectedDescriptionMedia {
    /// Creates an empty injector.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Registers JPEG bytes for `frame_id`.
    pub fn insert(&mut self, frame: &SavedFrameRef, jpeg: Vec<u8>) {
        self.frames
            .insert(frame.frame_id().as_str().to_owned(), jpeg);
    }
}

impl FrameDescriptionMedia for InjectedDescriptionMedia {
    fn jpeg_for(
        &self,
        request: &FrameDescriptionRequest,
    ) -> impl std::future::Future<Output = Result<Vec<u8>, SemanticIndexingFailure>> + Send {
        let key = request.key().clone();
        let frames = self.frames.clone();
        async move {
            let frame_id = key.frame().frame_id().as_str();
            frames.get(frame_id).cloned().ok_or_else(|| {
                SemanticIndexingFailure::ProviderUnavailable {
                    message: format!("no injected JPEG for frame {frame_id}"),
                }
            })
        }
    }
}

/// Encodes retained evidence as JPEG for Luna description (Chupacabra smoke path).
pub struct EvidenceJpegMedia {
    dvr: Arc<IndexedVideoDvr>,
    evidence_limits: FrameEvidenceLimits,
    jpeg_quality: u8,
}

impl EvidenceJpegMedia {
    /// Uses standard evidence caps and JPEG quality 80.
    #[must_use]
    pub fn new(dvr: Arc<IndexedVideoDvr>) -> Self {
        Self {
            dvr,
            evidence_limits: FrameEvidenceLimits::standard(),
            jpeg_quality: 80,
        }
    }
}

impl FrameDescriptionMedia for EvidenceJpegMedia {
    async fn jpeg_for(
        &self,
        request: &FrameDescriptionRequest,
    ) -> Result<Vec<u8>, SemanticIndexingFailure> {
        let evidence = FrameEvidenceRequest::new(request.key().frame(), request.key().roi());
        let pixels = self
            .dvr
            .read_bounded_frame_evidence(evidence, self.evidence_limits)
            .await
            .map_err(|failure| SemanticIndexingFailure::ProviderUnavailable {
                message: failure.to_string(),
            })?;
        encode_rgb_jpeg(
            pixels.rgb(),
            pixels.width(),
            pixels.height(),
            self.jpeg_quality,
        )
    }
}

fn encode_rgb_jpeg(
    rgb: &[u8],
    width: u32,
    height: u32,
    quality: u8,
) -> Result<Vec<u8>, SemanticIndexingFailure> {
    let expected = usize::try_from(width)
        .unwrap_or(0)
        .saturating_mul(usize::try_from(height).unwrap_or(0))
        .saturating_mul(3);
    if rgb.len() != expected {
        return Err(SemanticIndexingFailure::LimitExceeded {
            limit: "rgb_bytes".to_owned(),
            message: format!("expected {expected} RGB bytes, got {}", rgb.len()),
        });
    }
    let image = RgbImage::from_raw(width, height, rgb.to_vec()).ok_or_else(|| {
        SemanticIndexingFailure::ProviderUnavailable {
            message: "could not build RGB image for JPEG encoding".to_owned(),
        }
    })?;
    let mut out = Vec::new();
    let encoder = JpegEncoder::new_with_quality(&mut out, quality);
    encoder
        .write_image(image.as_raw(), width, height, ExtendedColorType::Rgb8)
        .map_err(|error| SemanticIndexingFailure::ProviderUnavailable {
            message: format!("jpeg encode failed: {error}"),
        })?;
    Ok(out)
}
