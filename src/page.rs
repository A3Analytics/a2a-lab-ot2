//! Cursor pagination helper matching the SDK page contract.

use a2a_lab_sdk::{Page, PageRequest, SdkError};

/// Returns one page of `items` using a numeric offset cursor.
pub fn slice_page<T: Clone>(items: &[T], request: &PageRequest) -> Result<Page<T>, SdkError> {
    request.check()?;
    let start = match request.cursor() {
        None => 0,
        Some(cursor) => cursor
            .parse()
            .map_err(|_| SdkError::invalid("cursor", "is not a valid page cursor"))?,
    };
    if start > items.len() {
        return Err(SdkError::invalid("cursor", "is past the end"));
    }
    let limit = usize::try_from(request.limit()).unwrap_or(usize::MAX);
    let end = start.saturating_add(limit).min(items.len());
    let next_cursor = (end < items.len()).then(|| end.to_string());
    Ok(Page::new(items[start..end].to_vec(), next_cursor))
}
