//! JPEG dimension reader for captured still frames.

use a2a_lab_dev_kit::A2aLabError;

/// Reads width and height from a JPEG start-of-frame marker.
pub(crate) fn jpeg_dimensions(data: &[u8]) -> Result<(u32, u32), A2aLabError> {
    if data.len() < 4 || data[0] != 0xFF || data[1] != 0xD8 {
        return Err(malformed());
    }
    let mut index = 2;
    while index < data.len() {
        let marker = read_marker(data, &mut index)?;
        if marker == 0xD9 || marker == 0xDA {
            return Err(malformed());
        }
        if is_standalone(marker) {
            continue;
        }
        let segment = read_segment(data, &mut index)?;
        if is_sof(marker) {
            return sof_size(segment);
        }
    }
    Err(malformed())
}

fn read_marker(data: &[u8], index: &mut usize) -> Result<u8, A2aLabError> {
    while *index < data.len() && data[*index] == 0xFF {
        *index += 1;
    }
    if *index >= data.len() {
        return Err(malformed());
    }
    let marker = data[*index];
    *index += 1;
    Ok(marker)
}

fn read_segment<'a>(data: &'a [u8], index: &mut usize) -> Result<&'a [u8], A2aLabError> {
    if *index + 1 >= data.len() {
        return Err(malformed());
    }
    let length = usize::from(u16::from_be_bytes([data[*index], data[*index + 1]]));
    if length < 2 {
        return Err(malformed());
    }
    let start = index.checked_add(2).ok_or_else(malformed)?;
    let end = index.checked_add(length).ok_or_else(malformed)?;
    if end > data.len() {
        return Err(malformed());
    }
    *index = end;
    Ok(&data[start..end])
}

fn sof_size(payload: &[u8]) -> Result<(u32, u32), A2aLabError> {
    if payload.len() < 5 {
        return Err(malformed());
    }
    let height = u16::from_be_bytes([payload[1], payload[2]]);
    let width = u16::from_be_bytes([payload[3], payload[4]]);
    if width == 0 || height == 0 {
        return Err(A2aLabError::invalid(
            "camera",
            "JPEG dimensions must be positive",
        ));
    }
    Ok((u32::from(width), u32::from(height)))
}

fn is_standalone(marker: u8) -> bool {
    marker == 0x01 || (0xD0..=0xD7).contains(&marker)
}

fn is_sof(marker: u8) -> bool {
    matches!(
        marker,
        0xC0 | 0xC1 | 0xC2 | 0xC3 | 0xC5 | 0xC6 | 0xC7 | 0xC9 | 0xCA | 0xCB | 0xCD | 0xCE | 0xCF
    )
}

fn malformed() -> A2aLabError {
    A2aLabError::invalid("camera", "malformed JPEG")
}
