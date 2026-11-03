//! JPEG and PNG header checks for channel payloads.

use super::FrameSetError;

pub(super) struct PngInfo {
    pub(super) width: u32,
    pub(super) height: u32,
    pub(super) bit_depth: u8,
    pub(super) color_type: u8,
}

pub(super) fn jpeg_ok(data: &[u8]) -> bool {
    data.len() >= 4 && data[0] == 0xFF && data[1] == 0xD8 && data[2] == 0xFF
}

pub(super) fn png_info(data: &[u8]) -> Result<PngInfo, FrameSetError> {
    const SIGNATURE: &[u8; 8] = b"\x89PNG\r\n\x1a\n";
    if data.len() < 26 || data.get(..8) != Some(SIGNATURE.as_slice()) {
        return Err(FrameSetError::Malformed(
            "PNG signature is missing".to_owned(),
        ));
    }
    let length = u32::from_be_bytes(copy4(data, 8)?);
    if length != 13 || data.get(12..16) != Some(b"IHDR".as_slice()) {
        return Err(FrameSetError::Malformed("PNG IHDR is missing".to_owned()));
    }
    Ok(PngInfo {
        width: u32::from_be_bytes(copy4(data, 16)?),
        height: u32::from_be_bytes(copy4(data, 20)?),
        bit_depth: data[24],
        color_type: data[25],
    })
}

fn copy4(data: &[u8], start: usize) -> Result<[u8; 4], FrameSetError> {
    data.get(start..start + 4)
        .and_then(|bytes| bytes.try_into().ok())
        .ok_or_else(|| FrameSetError::Malformed("PNG header is truncated".to_owned()))
}
