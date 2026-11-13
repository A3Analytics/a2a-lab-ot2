//! Bounded retained-media decode for one exact [`SavedFrameRef`] revision.

use std::io::Cursor;

use a2a_lab_dev_kit::A2aLabError;
use a2a_lab_dev_kit::videos::{
    ColorMatrix, ColorProperties, ColorRange, FrameRoiTransform, SavedFrameRef, Sha256Digest,
    VideoCodec, frame_h264_picture,
};
use image::{DynamicImage, ImageFormat, ImageReader, Limits};
use openh264::decoder::Decoder;
use openh264::formats::YUVSource;
use sqlx::Row;

use super::frame_manifest::ManifestRetainedState;
use super::store::{IndexedVideoDvr, parse_codec};

/// Stable reason for bounded evidence failures.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FrameEvidenceFailureKind {
    /// Requested digest does not match loaded payload bytes.
    DigestMismatch,
    /// Retained row or bytes are not available for this revision.
    MissingOrExpired,
    /// CRC, framing, or decode input is not intact.
    CorruptPayload,
    /// Track codec is outside the supported reader set.
    UnsupportedCodec,
    /// Required decoder crates are not linked for this codec.
    DecoderUnavailable,
    /// Allocation or dimension caps would be exceeded.
    DecodeLimitsExceeded,
    /// Compressed media could not be decoded to canonical pixels.
    CorruptMedia,
}

/// One bounded evidence read failure.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FrameEvidenceFailure {
    kind: FrameEvidenceFailureKind,
    message: String,
}

impl FrameEvidenceFailure {
    /// Failure category for metrics and tests.
    #[must_use]
    pub const fn kind(&self) -> FrameEvidenceFailureKind {
        self.kind
    }

    /// Stable `snake_case` identifier.
    #[must_use]
    pub const fn code(&self) -> &'static str {
        match self.kind {
            FrameEvidenceFailureKind::DigestMismatch => "digest_mismatch",
            FrameEvidenceFailureKind::MissingOrExpired => "missing_or_expired",
            FrameEvidenceFailureKind::CorruptPayload => "corrupt_payload",
            FrameEvidenceFailureKind::UnsupportedCodec => "unsupported_codec",
            FrameEvidenceFailureKind::DecoderUnavailable => "decoder_unavailable",
            FrameEvidenceFailureKind::DecodeLimitsExceeded => "decode_limits_exceeded",
            FrameEvidenceFailureKind::CorruptMedia => "corrupt_media",
        }
    }

    fn new(kind: FrameEvidenceFailureKind, message: impl Into<String>) -> Self {
        Self {
            kind,
            message: message.into(),
        }
    }
}

impl std::fmt::Display for FrameEvidenceFailure {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "{}: {}", self.code(), self.message)
    }
}

impl std::error::Error for FrameEvidenceFailure {}

/// Caps for one evidence decode; separate from vector batching elsewhere.
#[allow(clippy::struct_field_names)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FrameEvidenceLimits {
    max_payload_bytes: usize,
    max_rgb_bytes: usize,
    max_edge: u32,
}

impl FrameEvidenceLimits {
    /// Rejects zero caps and edges above the encoder limit.
    pub fn new(
        max_payload_bytes: usize,
        max_rgb_bytes: usize,
        max_edge: u32,
    ) -> Result<Self, A2aLabError> {
        if max_payload_bytes == 0 || max_rgb_bytes == 0 || max_edge == 0 {
            return Err(A2aLabError::invalid(
                "frame_evidence_limits",
                "limits must be positive",
            ));
        }
        Ok(Self {
            max_payload_bytes,
            max_rgb_bytes,
            max_edge,
        })
    }

    /// Production-style source decode caps for retained 1920x1080 media.
    ///
    /// Jev/model input limits remain enforced downstream; this bound only
    /// prevents the source decoder from rejecting a native external-camera
    /// frame before preprocessing.
    #[must_use]
    pub fn standard() -> Self {
        Self {
            max_payload_bytes: 8 * 1024 * 1024,
            max_rgb_bytes: 16 * 1024 * 1024,
            max_edge: 1920,
        }
    }

    #[must_use]
    pub const fn max_payload_bytes(self) -> usize {
        self.max_payload_bytes
    }

    #[must_use]
    pub const fn max_rgb_bytes(self) -> usize {
        self.max_rgb_bytes
    }

    #[must_use]
    pub const fn max_edge(self) -> u32 {
        self.max_edge
    }
}

/// Optional region of interest on the oriented original frame.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FrameEvidenceRequest<'a> {
    frame: &'a SavedFrameRef,
    roi: Option<FrameRoiTransform>,
}

impl<'a> FrameEvidenceRequest<'a> {
    /// Reads the full oriented frame when `roi` is absent.
    #[must_use]
    pub const fn new(frame: &'a SavedFrameRef, roi: Option<FrameRoiTransform>) -> Self {
        Self { frame, roi }
    }

    #[must_use]
    pub const fn frame(&self) -> &'a SavedFrameRef {
        self.frame
    }

    #[must_use]
    pub const fn roi(&self) -> Option<FrameRoiTransform> {
        self.roi
    }
}

/// Canonical oriented RGB8 and transforms for Jev evidence.
#[derive(Debug, Clone, PartialEq)]
pub struct FrameEvidencePixels {
    frame: SavedFrameRef,
    width: u32,
    height: u32,
    rgb: Vec<u8>,
    properties: ColorProperties,
    original_transform: FrameRoiTransform,
    model_input_transform: FrameRoiTransform,
    verified_digest: Sha256Digest,
}

impl FrameEvidencePixels {
    #[must_use]
    pub const fn frame(&self) -> &SavedFrameRef {
        &self.frame
    }

    #[must_use]
    pub const fn width(&self) -> u32 {
        self.width
    }

    #[must_use]
    pub const fn height(&self) -> u32 {
        self.height
    }

    #[must_use]
    pub fn rgb(&self) -> &[u8] {
        &self.rgb
    }

    #[must_use]
    pub const fn properties(&self) -> ColorProperties {
        self.properties
    }

    /// Identity mapping for the stored full oriented frame.
    #[must_use]
    pub const fn original_transform(&self) -> FrameRoiTransform {
        self.original_transform
    }

    /// Mapping from returned pixels back to original-frame coordinates.
    #[must_use]
    pub const fn model_input_transform(&self) -> FrameRoiTransform {
        self.model_input_transform
    }

    #[must_use]
    pub const fn verified_digest(&self) -> &Sha256Digest {
        &self.verified_digest
    }
}

impl IndexedVideoDvr {
    /// Loads, verifies, and decodes one saved revision without scanning unrelated media.
    pub async fn read_bounded_frame_evidence(
        &self,
        request: FrameEvidenceRequest<'_>,
        limits: FrameEvidenceLimits,
    ) -> Result<FrameEvidencePixels, FrameEvidenceFailure> {
        let frame = request.frame();
        self.guard_retained_revision(frame).await?;
        let (codec, codec_init) = self.track_codec(frame).await?;
        let payload = self
            .load_verified_payload(frame, limits.max_payload_bytes())
            .await?;
        let (width, height, rgb, properties) =
            decode_payload(codec, &codec_init, &payload, limits)?;
        let original_transform = full_frame_transform();
        let model_input_transform = request.roi().unwrap_or(original_transform);
        let (rgb, width, height) = apply_roi(rgb, width, height, model_input_transform, limits)?;
        Ok(FrameEvidencePixels {
            frame: frame.clone(),
            width,
            height,
            rgb,
            properties,
            original_transform,
            model_input_transform,
            verified_digest: Sha256Digest::hash_bytes(&payload),
        })
    }

    async fn guard_retained_revision(
        &self,
        frame: &SavedFrameRef,
    ) -> Result<(), FrameEvidenceFailure> {
        let row =
            sqlx::query("SELECT retained_state FROM semantic_frame_revisions WHERE frame_id = ?1")
                .bind(frame.frame_id().as_str())
                .fetch_optional(self.semantic_pool())
                .await
                .map_err(|error| db_failure(error.to_string()))?;
        if let Some(row) = row {
            let state: String = row
                .try_get("retained_state")
                .map_err(|error| db_failure(error.to_string()))?;
            if parse_retained(&state)? == ManifestRetainedState::Expired {
                return Err(FrameEvidenceFailure::new(
                    FrameEvidenceFailureKind::MissingOrExpired,
                    "manifest revision is expired",
                ));
            }
        }
        Ok(())
    }

    async fn track_codec(
        &self,
        frame: &SavedFrameRef,
    ) -> Result<(VideoCodec, Vec<u8>), FrameEvidenceFailure> {
        let source_id = frame.source_id();
        let track_id = frame.track_id();
        let row = sqlx::query(
            "SELECT codec, codec_init FROM dvr_tracks WHERE source_id = ?1 AND track_id = ?2",
        )
        .bind(source_id.as_str())
        .bind(track_id.as_str())
        .fetch_optional(self.media_pool())
        .await
        .map_err(|error| db_failure(error.to_string()))?;
        let Some(row) = row else {
            return Err(FrameEvidenceFailure::new(
                FrameEvidenceFailureKind::MissingOrExpired,
                "DVR track is not indexed",
            ));
        };
        let codec = parse_codec(
            &row.try_get::<String, _>("codec")
                .map_err(|error| db_failure(error.to_string()))?,
        )
        .map_err(|error| db_failure(error.to_string()))?;
        let mut init: Vec<u8> = row
            .try_get("codec_init")
            .map_err(|error| db_failure(error.to_string()))?;
        if codec == VideoCodec::H264 {
            let epoch_init = super::h264_epoch::codec_init_for_frame(
                self.media_pool(),
                source_id,
                track_id,
                frame.captured_at(),
            )
            .await
            .map_err(|error| db_failure(error.to_string()))?;
            let Some(epoch_init) = epoch_init else {
                return Err(FrameEvidenceFailure::new(
                    FrameEvidenceFailureKind::CorruptPayload,
                    "H.264 epoch is missing decoder initialization",
                ));
            };
            init = epoch_init;
        }
        Ok((codec, init))
    }

    async fn load_verified_payload(
        &self,
        frame: &SavedFrameRef,
        max_bytes: usize,
    ) -> Result<Vec<u8>, FrameEvidenceFailure> {
        let bytes = self
            .load_retained_payload_at_key(frame.source_id(), frame.track_id(), frame.captured_at())
            .await
            .map_err(|error| map_load_error(error.to_string()))?
            .0;
        if bytes.len() > max_bytes {
            return Err(FrameEvidenceFailure::new(
                FrameEvidenceFailureKind::DecodeLimitsExceeded,
                "payload length exceeds the configured byte cap",
            ));
        }
        let digest = Sha256Digest::hash_bytes(&bytes);
        if digest != *frame.payload_digest() {
            return Err(FrameEvidenceFailure::new(
                FrameEvidenceFailureKind::DigestMismatch,
                "loaded bytes do not match the requested revision digest",
            ));
        }
        Ok(bytes)
    }
}

fn decode_payload(
    codec: VideoCodec,
    codec_init: &[u8],
    payload: &[u8],
    limits: FrameEvidenceLimits,
) -> Result<(u32, u32, Vec<u8>, ColorProperties), FrameEvidenceFailure> {
    match codec {
        VideoCodec::Mjpeg => decode_mjpeg_rgb(payload, limits),
        VideoCodec::H264 => decode_h264_rgb(codec_init, payload, limits),
        VideoCodec::H265 => Err(FrameEvidenceFailure::new(
            FrameEvidenceFailureKind::UnsupportedCodec,
            "H.265 evidence decode is not supported",
        )),
    }
}

fn decode_mjpeg_rgb(
    jpeg: &[u8],
    limits: FrameEvidenceLimits,
) -> Result<(u32, u32, Vec<u8>, ColorProperties), FrameEvidenceFailure> {
    let image = read_jpeg_bounded(jpeg, limits)?;
    let rgb = image.to_rgb8();
    if rgb.width() > limits.max_edge() || rgb.height() > limits.max_edge() {
        return Err(FrameEvidenceFailure::new(
            FrameEvidenceFailureKind::DecodeLimitsExceeded,
            "decoded MJPEG edge exceeds the configured limit",
        ));
    }
    let pixel_count = u64::from(rgb.width()).saturating_mul(u64::from(rgb.height()));
    let rgb_len = pixel_count.saturating_mul(3);
    if rgb_len > u64::try_from(limits.max_rgb_bytes()).unwrap_or(u64::MAX) {
        return Err(FrameEvidenceFailure::new(
            FrameEvidenceFailureKind::DecodeLimitsExceeded,
            "decoded MJPEG exceeds the RGB byte cap",
        ));
    }
    Ok((
        rgb.width(),
        rgb.height(),
        rgb.into_raw(),
        jpeg_color_properties(),
    ))
}

fn decode_h264_rgb(
    codec_init: &[u8],
    sample: &[u8],
    limits: FrameEvidenceLimits,
) -> Result<(u32, u32, Vec<u8>, ColorProperties), FrameEvidenceFailure> {
    if codec_init.is_empty() {
        return Err(FrameEvidenceFailure::new(
            FrameEvidenceFailureKind::CorruptPayload,
            "H.264 epoch is missing decoder initialization",
        ));
    }
    let annex_b = annex_b_from_dvr_sample(codec_init, sample)?;
    let mut decoder = Decoder::new().map_err(|error| {
        FrameEvidenceFailure::new(
            FrameEvidenceFailureKind::DecoderUnavailable,
            format!("OpenH264 decoder init failed: {error}"),
        )
    })?;
    let picture = decoder.decode(&annex_b).map_err(|error| {
        FrameEvidenceFailure::new(
            FrameEvidenceFailureKind::CorruptMedia,
            format!("H.264 decode failed: {error}"),
        )
    })?;
    let Some(picture) = picture else {
        return Err(FrameEvidenceFailure::new(
            FrameEvidenceFailureKind::CorruptMedia,
            "H.264 decoder returned no picture",
        ));
    };
    let (width, height) = (
        u32::try_from(picture.dimensions().0).unwrap_or(0),
        u32::try_from(picture.dimensions().1).unwrap_or(0),
    );
    if width == 0 || height == 0 || width > limits.max_edge() || height > limits.max_edge() {
        return Err(FrameEvidenceFailure::new(
            FrameEvidenceFailureKind::DecodeLimitsExceeded,
            "decoded H.264 dimensions exceed limits",
        ));
    }
    let planes = picture_planes(&picture, width, height)?;
    let rgb = i420_to_rgb8(&planes, limits)?;
    Ok((width, height, rgb, h264_color_properties()))
}

fn read_jpeg_bounded(
    jpeg: &[u8],
    limits: FrameEvidenceLimits,
) -> Result<DynamicImage, FrameEvidenceFailure> {
    if jpeg.len() > limits.max_payload_bytes() {
        return Err(FrameEvidenceFailure::new(
            FrameEvidenceFailureKind::DecodeLimitsExceeded,
            "MJPEG payload exceeds byte cap",
        ));
    }
    let mut reader = ImageReader::new(Cursor::new(jpeg));
    reader.set_format(ImageFormat::Jpeg);
    let mut image_limits = Limits::default();
    image_limits.max_image_width = Some(limits.max_edge());
    image_limits.max_image_height = Some(limits.max_edge());
    image_limits.max_alloc = Some(u64::try_from(limits.max_rgb_bytes().max(1)).unwrap_or(u64::MAX));
    reader.limits(image_limits);
    reader.decode().map_err(|error| {
        FrameEvidenceFailure::new(
            FrameEvidenceFailureKind::CorruptMedia,
            format!("MJPEG decode failed: {error}"),
        )
    })
}

fn annex_b_from_dvr_sample(
    codec_init: &[u8],
    sample: &[u8],
) -> Result<Vec<u8>, FrameEvidenceFailure> {
    let mut annex_b = codec_init.to_vec();
    let mut index = 0_usize;
    while index + 4 <= sample.len() {
        let length = u32::from_be_bytes([
            sample[index],
            sample[index + 1],
            sample[index + 2],
            sample[index + 3],
        ]);
        let length = usize::try_from(length).map_err(|_| {
            FrameEvidenceFailure::new(
                FrameEvidenceFailureKind::CorruptPayload,
                "H.264 NAL length overflow",
            )
        })?;
        index += 4;
        if index.saturating_add(length) > sample.len() {
            return Err(FrameEvidenceFailure::new(
                FrameEvidenceFailureKind::CorruptPayload,
                "H.264 length-prefixed sample is truncated",
            ));
        }
        annex_b.extend_from_slice(&[0, 0, 0, 1]);
        annex_b.extend_from_slice(&sample[index..index + length]);
        index += length;
    }
    if annex_b.len() == codec_init.len() {
        let framed = frame_h264_picture(sample).map_err(|error| {
            FrameEvidenceFailure::new(FrameEvidenceFailureKind::CorruptPayload, error.to_string())
        })?;
        return annex_b_from_dvr_sample(codec_init, framed.sample());
    }
    Ok(annex_b)
}

struct TightI420 {
    width: u32,
    height: u32,
    y: Vec<u8>,
    u: Vec<u8>,
    v: Vec<u8>,
}

fn picture_planes(
    picture: &impl YUVSource,
    width: u32,
    height: u32,
) -> Result<TightI420, FrameEvidenceFailure> {
    let (stride_y, stride_u, stride_v) = picture.strides();
    let y_plane = extract_plane(picture.y(), stride_y, width, height)?;
    let chroma_w = width / 2;
    let chroma_h = height / 2;
    let u_plane = extract_plane(picture.u(), stride_u, chroma_w, chroma_h)?;
    let v_plane = extract_plane(picture.v(), stride_v, chroma_w, chroma_h)?;
    Ok(TightI420 {
        width,
        height,
        y: y_plane,
        u: u_plane,
        v: v_plane,
    })
}

fn extract_plane(
    data: &[u8],
    stride: usize,
    width: u32,
    height: u32,
) -> Result<Vec<u8>, FrameEvidenceFailure> {
    let width = usize::try_from(width).unwrap_or(0);
    let height = usize::try_from(height).unwrap_or(0);
    let mut plane = Vec::with_capacity(width.saturating_mul(height));
    for row in 0..height {
        let start = row.saturating_mul(stride);
        let end = start.saturating_add(width);
        if end > data.len() {
            return Err(FrameEvidenceFailure::new(
                FrameEvidenceFailureKind::CorruptMedia,
                "decoder plane stride exceeds buffer",
            ));
        }
        plane.extend_from_slice(&data[start..end]);
    }
    Ok(plane)
}

fn i420_to_rgb8(
    picture: &TightI420,
    limits: FrameEvidenceLimits,
) -> Result<Vec<u8>, FrameEvidenceFailure> {
    let width = picture.width;
    let height = picture.height;
    let rgb_len = u64::from(width)
        .saturating_mul(u64::from(height))
        .saturating_mul(3);
    if rgb_len > u64::try_from(limits.max_rgb_bytes()).unwrap_or(u64::MAX) {
        return Err(FrameEvidenceFailure::new(
            FrameEvidenceFailureKind::DecodeLimitsExceeded,
            "I420 conversion exceeds RGB byte cap",
        ));
    }
    let y_stride = width;
    let uv_stride = width / 2;
    let planar = yuv::YuvPlanarImage::<u8> {
        y_plane: &picture.y,
        y_stride,
        u_plane: &picture.u,
        u_stride: uv_stride,
        v_plane: &picture.v,
        v_stride: uv_stride,
        width,
        height,
    };
    let mut rgb = vec![0_u8; usize::try_from(rgb_len).unwrap_or(0)];
    let (range, matrix) = h264_yuv_basis();
    yuv::yuv420_to_rgb(&planar, &mut rgb, width.saturating_mul(3), range, matrix).map_err(
        |error| {
            FrameEvidenceFailure::new(
                FrameEvidenceFailureKind::CorruptMedia,
                format!("I420 to RGB failed: {error:?}"),
            )
        },
    )?;
    Ok(rgb)
}

fn h264_yuv_basis() -> (yuv::YuvRange, yuv::YuvStandardMatrix) {
    (yuv::YuvRange::Limited, yuv::YuvStandardMatrix::Bt709)
}

fn apply_roi(
    rgb: Vec<u8>,
    width: u32,
    height: u32,
    roi: FrameRoiTransform,
    limits: FrameEvidenceLimits,
) -> Result<(Vec<u8>, u32, u32), FrameEvidenceFailure> {
    if roi_is_full_frame(roi) {
        ensure_rgb_bounds(rgb.len(), limits)?;
        return Ok((rgb, width, height));
    }
    let [x_min, y_min, x_max, y_max] = roi.bounds();
    let x0 = pixel_edge(x_min, width, false);
    let y0 = pixel_edge(y_min, height, false);
    let x1 = pixel_edge(x_max, width, true);
    let y1 = pixel_edge(y_max, height, true);
    let crop_w = x1.saturating_sub(x0).max(1);
    let crop_h = y1.saturating_sub(y0).max(1);
    if crop_w > limits.max_edge() || crop_h > limits.max_edge() {
        return Err(FrameEvidenceFailure::new(
            FrameEvidenceFailureKind::DecodeLimitsExceeded,
            "region of interest crop edge exceeds the configured limit",
        ));
    }
    let stride = usize::try_from(width).unwrap_or(0).saturating_mul(3);
    let mut out = Vec::with_capacity(usize::try_from(crop_w * crop_h * 3).unwrap_or(0));
    for row in y0..y1 {
        let start =
            usize::try_from(row).unwrap_or(0) * stride + usize::try_from(x0).unwrap_or(0) * 3;
        let end = start + usize::try_from(crop_w).unwrap_or(0) * 3;
        if end > rgb.len() {
            return Err(FrameEvidenceFailure::new(
                FrameEvidenceFailureKind::CorruptPayload,
                "region of interest crop exceeds RGB buffer",
            ));
        }
        out.extend_from_slice(&rgb[start..end]);
    }
    ensure_rgb_bounds(out.len(), limits)?;
    Ok((out, crop_w, crop_h))
}

fn pixel_edge(fraction: f32, edge: u32, max_edge: bool) -> u32 {
    #[allow(clippy::cast_precision_loss)]
    let scaled = fraction * edge as f32;
    let value = if max_edge {
        scaled.ceil().max(1.0)
    } else {
        scaled.floor()
    };
    #[allow(
        clippy::cast_possible_truncation,
        clippy::cast_precision_loss,
        clippy::cast_sign_loss
    )]
    let capped = value.clamp(0.0, edge as f32) as u32;
    capped
}

#[allow(clippy::float_cmp)]
fn roi_is_full_frame(roi: FrameRoiTransform) -> bool {
    let [x_min, y_min, x_max, y_max] = roi.bounds();
    x_min == 0.0 && y_min == 0.0 && x_max == 1.0 && y_max == 1.0
}

fn full_frame_transform() -> FrameRoiTransform {
    FrameRoiTransform::full_frame()
}

fn ensure_rgb_bounds(len: usize, limits: FrameEvidenceLimits) -> Result<(), FrameEvidenceFailure> {
    if len > limits.max_rgb_bytes() {
        Err(FrameEvidenceFailure::new(
            FrameEvidenceFailureKind::DecodeLimitsExceeded,
            "RGB buffer exceeds configured decode cap",
        ))
    } else {
        Ok(())
    }
}

fn jpeg_color_properties() -> ColorProperties {
    ColorProperties::new(ColorMatrix::Bt601, ColorRange::Full)
}

fn h264_color_properties() -> ColorProperties {
    ColorProperties::new(ColorMatrix::Bt709, ColorRange::Limited)
}

fn map_load_error(message: String) -> FrameEvidenceFailure {
    if message.contains("checksum mismatch") {
        FrameEvidenceFailure::new(FrameEvidenceFailureKind::CorruptPayload, message)
    } else if message.contains("not found") {
        FrameEvidenceFailure::new(FrameEvidenceFailureKind::MissingOrExpired, message)
    } else {
        FrameEvidenceFailure::new(FrameEvidenceFailureKind::CorruptPayload, message)
    }
}

fn db_failure(message: String) -> FrameEvidenceFailure {
    FrameEvidenceFailure::new(FrameEvidenceFailureKind::MissingOrExpired, message)
}

fn parse_retained(label: &str) -> Result<ManifestRetainedState, FrameEvidenceFailure> {
    match label {
        "retained" => Ok(ManifestRetainedState::Retained),
        "expired" => Ok(ManifestRetainedState::Expired),
        "superseded" => Ok(ManifestRetainedState::Superseded),
        other => Err(FrameEvidenceFailure::new(
            FrameEvidenceFailureKind::CorruptPayload,
            format!("unknown retained state {other}"),
        )),
    }
}

#[cfg(test)]
mod plane_tests {
    use super::*;

    #[test]
    fn digest_mismatch_is_distinct_from_missing() {
        let missing = FrameEvidenceFailure::new(FrameEvidenceFailureKind::MissingOrExpired, "x");
        let digest = FrameEvidenceFailure::new(FrameEvidenceFailureKind::DigestMismatch, "x");
        assert_ne!(missing.code(), digest.code());
    }
}
