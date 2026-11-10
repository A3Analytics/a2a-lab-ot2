//! `TypeSafe` image dimension and byte limits before upload.

use a2a_lab_dev_kit::{
    FrameDecisionEvaluationFailure, JEV_MAX_EDGE_PIXELS, JEV_MAX_IMAGE_BYTES,
    JEV_MAX_REQUEST_BYTES, JEV_MIN_EDGE_PIXELS,
};

/// Minimum width × height per `TypeSafe` image bounds.
pub const JEV_MIN_TOTAL_PIXELS: u64 = 280 * 280;

/// Maximum width × height per `TypeSafe` image bounds.
pub const JEV_MAX_TOTAL_PIXELS: u64 = 1568 * 1568;

/// One validated still image prepared for a System One request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ValidatedJevImage {
    bytes: Vec<u8>,
    format: &'static str,
    width: u32,
    height: u32,
    content_digest: String,
}

impl ValidatedJevImage {
    /// Raw file bytes (PNG or JPEG).
    #[must_use]
    pub fn bytes(&self) -> &[u8] {
        &self.bytes
    }

    /// Lowercase `png` or `jpg`.
    #[must_use]
    pub const fn format(&self) -> &'static str {
        self.format
    }

    /// Short stable asset reference key.
    #[must_use]
    pub fn asset_ref(&self) -> &str {
        &self.content_digest
    }
}

/// Validates one PNG/JPEG against documented `TypeSafe` bounds.
pub fn validate_jev_image(
    bytes: &[u8],
) -> Result<ValidatedJevImage, FrameDecisionEvaluationFailure> {
    if bytes.is_empty() {
        return Err(FrameDecisionEvaluationFailure::InvalidEvidence {
            message: "empty image".to_owned(),
        });
    }
    if bytes.len() > JEV_MAX_IMAGE_BYTES {
        return Err(FrameDecisionEvaluationFailure::LimitExceeded {
            limit: "max_image_bytes".to_owned(),
            message: format!(
                "image {} bytes exceeds {}",
                bytes.len(),
                JEV_MAX_IMAGE_BYTES
            ),
        });
    }
    let format = detect_format(bytes)?;
    let (width, height) = decode_dimensions(bytes, format)?;
    let max_edge = width.max(height);
    let min_edge = width.min(height);
    if max_edge > JEV_MAX_EDGE_PIXELS || min_edge < JEV_MIN_EDGE_PIXELS {
        return Err(FrameDecisionEvaluationFailure::LimitExceeded {
            limit: "edge_pixels".to_owned(),
            message: format!(
                "image {width}x{height} outside {JEV_MIN_EDGE_PIXELS}..={JEV_MAX_EDGE_PIXELS}"
            ),
        });
    }
    let total = u64::from(width) * u64::from(height);
    if !(JEV_MIN_TOTAL_PIXELS..=JEV_MAX_TOTAL_PIXELS).contains(&total) {
        return Err(FrameDecisionEvaluationFailure::LimitExceeded {
            limit: "total_pixels".to_owned(),
            message: format!(
                "image area {total} outside {JEV_MIN_TOTAL_PIXELS}..={JEV_MAX_TOTAL_PIXELS}"
            ),
        });
    }
    let content_digest = a2a_lab_dev_kit::Sha256Digest::hash_bytes(bytes)
        .as_str()
        .chars()
        .take(16)
        .collect();
    Ok(ValidatedJevImage {
        bytes: bytes.to_vec(),
        format,
        width,
        height,
        content_digest,
    })
}

/// Ensures serialized request JSON plus assets stay within the 32 MiB cap.
pub fn validate_request_bytes(declared: usize) -> Result<(), FrameDecisionEvaluationFailure> {
    if declared > JEV_MAX_REQUEST_BYTES {
        return Err(FrameDecisionEvaluationFailure::LimitExceeded {
            limit: "max_request_bytes".to_owned(),
            message: format!("request {declared} bytes exceeds {JEV_MAX_REQUEST_BYTES}"),
        });
    }
    Ok(())
}

fn detect_format(bytes: &[u8]) -> Result<&'static str, FrameDecisionEvaluationFailure> {
    if bytes.starts_with(&[0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A]) {
        return Ok("png");
    }
    if bytes.len() >= 3 && bytes[0] == 0xFF && bytes[1] == 0xD8 && bytes.last() == Some(&0xD9) {
        return Ok("jpg");
    }
    Err(FrameDecisionEvaluationFailure::InvalidEvidence {
        message: "image must be PNG or baseline JPEG".to_owned(),
    })
}

fn decode_dimensions(
    bytes: &[u8],
    format: &str,
) -> Result<(u32, u32), FrameDecisionEvaluationFailure> {
    let image_format = match format {
        "png" => image::ImageFormat::Png,
        _ => image::ImageFormat::Jpeg,
    };
    let mut reader = image::ImageReader::new(std::io::Cursor::new(bytes));
    reader.set_format(image_format);
    let (width, height) = reader.into_dimensions().map_err(|error| {
        FrameDecisionEvaluationFailure::InvalidEvidence {
            message: format!("cannot decode {format} dimensions: {error}"),
        }
    })?;
    Ok((width, height))
}
