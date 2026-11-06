//! Converts negotiated raw capture into JPEG bytes for the live pump.

use a2a_lab_dev_kit::A2aLabError;
use a2a_lab_dev_kit::videos::{
    CaptureFormat, CapturedColorFrame, JpegDerivativeLimits, jpeg_derivative,
};

use crate::camera::trim_mjpeg_buffer;

/// Returns the bounded MJPEG access unit, or encodes packed YUY2 through the dev-kit derivative API.
pub fn captured_frame_jpeg(
    frame: &CapturedColorFrame,
    max_bytes: u64,
) -> Result<Vec<u8>, A2aLabError> {
    match frame.format() {
        CaptureFormat::Mjpeg => bounded_mjpeg(frame.bytes(), max_bytes),
        CaptureFormat::Yuy2 => {
            let limits =
                JpegDerivativeLimits::new(usize::try_from(max_bytes).unwrap_or(usize::MAX))?;
            Ok(jpeg_derivative(frame, 80, limits)?.jpeg().to_vec())
        }
    }
}

fn bounded_mjpeg(bytes: &[u8], max_bytes: u64) -> Result<Vec<u8>, A2aLabError> {
    let jpeg = trim_mjpeg_buffer(bytes);
    let cap = usize::try_from(max_bytes).unwrap_or(usize::MAX);
    if jpeg.len() > cap {
        return Err(A2aLabError::invalid(
            "jpeg",
            "compressed JPEG exceeds the byte limit",
        ));
    }
    Ok(jpeg.to_vec())
}

#[cfg(test)]
mod tests {
    use a2a_lab_dev_kit::videos::{
        CaptureClock, CaptureIdentity, CaptureTiming, CapturedColorFrame, ColorMatrix,
        ColorProperties, ColorRange, JpegDerivativeLimits, jpeg_derivative,
    };
    use a2a_lab_dev_kit::{UtcTimestamp, VideoSourceId};

    use super::captured_frame_jpeg;

    fn mjpeg_frame(bytes: &[u8]) -> CapturedColorFrame {
        CapturedColorFrame::mjpeg(
            CaptureIdentity::new(VideoSourceId::new("camera").unwrap(), 1),
            CaptureTiming::new(
                UtcTimestamp::parse("2024-06-01T00:00:00Z").unwrap(),
                CaptureClock::monotonic(0),
            ),
            1,
            1,
            bytes,
            ColorProperties::new(ColorMatrix::Unknown, ColorRange::Unknown),
        )
        .unwrap()
    }

    fn jpeg() -> Vec<u8> {
        vec![
            0xFF, 0xD8, 0xFF, 0xC0, 0x00, 0x0B, 0x08, 0x00, 0x01, 0x00, 0x01, 0x01, 0x01, 0x11,
            0x00, 0xFF, 0xDA, 0x00, 0x08, 0x01, 0x01, 0x00, 0x00, 0x3F, 0x00, 0xFF, 0xD9,
        ]
    }

    #[test]
    fn trims_padding_before_the_byte_limit() {
        let exact = jpeg();
        let mut padded = exact.clone();
        padded.extend_from_slice(&[0, 0, 0, 0]);
        let frame = mjpeg_frame(&padded);
        let stored = captured_frame_jpeg(&frame, exact.len() as u64).unwrap();
        assert_eq!(stored, exact);
    }

    #[test]
    fn rejects_the_normalized_jpeg_when_it_exceeds_the_limit() {
        let exact = jpeg();
        let mut padded = exact.clone();
        padded.extend_from_slice(&[0, 0, 0, 0]);
        let frame = mjpeg_frame(&padded);
        let error = captured_frame_jpeg(&frame, (exact.len() - 1) as u64).unwrap_err();
        assert_eq!(error.code(), "invalid");
        assert_eq!(
            error.to_string(),
            "invalid jpeg: compressed JPEG exceeds the byte limit"
        );
    }

    #[test]
    fn keeps_mjpeg_without_an_end_marker() {
        let bytes = [0xFF, 0xD8, 0xFF, 0xC0, 0x00, 0x03, 0x08, 0, 0, 0];
        let frame = mjpeg_frame(&bytes);
        let stored = captured_frame_jpeg(&frame, 64).unwrap();
        assert_eq!(stored, bytes);
    }

    #[test]
    fn leaves_yuy2_derivative_bytes_unchanged() {
        let frame = CapturedColorFrame::yuy2(
            CaptureIdentity::new(VideoSourceId::new("camera").unwrap(), 1),
            CaptureTiming::new(
                UtcTimestamp::parse("2024-06-01T00:00:00Z").unwrap(),
                CaptureClock::monotonic(0),
            ),
            2,
            1,
            4,
            &[16, 128, 16, 128],
            ColorProperties::new(ColorMatrix::Bt601, ColorRange::Limited),
            None,
        )
        .unwrap();
        let limits = JpegDerivativeLimits::new(64_000).unwrap();
        let expected = jpeg_derivative(&frame, 80, limits).unwrap();
        let stored = captured_frame_jpeg(&frame, 64_000).unwrap();
        assert_eq!(stored, expected.jpeg());
    }
}
