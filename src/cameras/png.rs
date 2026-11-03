//! Lossless grayscale PNG encoding for depth, infrared, and confidence.

use image::ExtendedColorType;
use image::ImageEncoder;
use image::codecs::png::PngEncoder;

use crate::frame_sets::FrameSetError;

/// Encodes native grayscale samples as a PNG.
///
/// 16-bit samples are big-endian, which is the PNG layout.
pub fn encode_gray(
    samples: &[u8],
    width: u32,
    height: u32,
    bit_depth: u8,
) -> Result<Vec<u8>, FrameSetError> {
    let bytes_per_sample = match bit_depth {
        8 => 1,
        16 => 2,
        _ => {
            return Err(FrameSetError::Malformed(
                "grayscale PNG bit depth must be 8 or 16".to_owned(),
            ));
        }
    };
    let expected = usize::try_from(width)
        .ok()
        .and_then(|value| value.checked_mul(usize::try_from(height).unwrap_or(0)))
        .and_then(|pixels| pixels.checked_mul(bytes_per_sample))
        .ok_or_else(|| FrameSetError::Malformed("image dimensions overflow".to_owned()))?;
    if samples.len() != expected {
        return Err(FrameSetError::Malformed(format!(
            "expected {expected} grayscale bytes, found {}",
            samples.len()
        )));
    }
    let color = if bit_depth == 16 {
        ExtendedColorType::L16
    } else {
        ExtendedColorType::L8
    };
    let mut output = std::io::Cursor::new(Vec::new());
    PngEncoder::new(&mut output)
        .write_image(samples, width, height, color)
        .map_err(|error| FrameSetError::Malformed(format!("PNG encode failed: {error}")))?;
    Ok(output.into_inner())
}

/// Converts little-endian `Y16` samples to the endianness the PNG encoder expects.
pub fn y16_native(little_endian: &[u8]) -> Result<Vec<u8>, FrameSetError> {
    if !little_endian.len().is_multiple_of(2) {
        return Err(FrameSetError::Malformed(
            "Y16 sample buffer has an odd length".to_owned(),
        ));
    }
    #[cfg(target_endian = "little")]
    {
        Ok(little_endian.to_vec())
    }
    #[cfg(target_endian = "big")]
    {
        let mut native = vec![0; little_endian.len()];
        for (index, sample) in little_endian.as_chunks::<2>().0.iter().enumerate() {
            native[index * 2] = sample[1];
            native[index * 2 + 1] = sample[0];
        }
        Ok(native)
    }
}
