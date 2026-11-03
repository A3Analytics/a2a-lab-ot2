//! Retained interval and gap derivation from indexed segment starts.

use a2a_lab_dev_kit::{A2aLabError, DvrMediaCoverage, TimeRange, UtcTimestamp};

pub(crate) fn intervals_from_starts(
    starts: &[UtcTimestamp],
    period_us: u64,
) -> Result<Vec<TimeRange>, A2aLabError> {
    if starts.is_empty() {
        return Ok(Vec::new());
    }
    let period = i64::try_from(period_us.max(1)).unwrap_or(1);
    let mut segments = Vec::new();
    let mut start = starts[0];
    let mut last = starts[0];
    for stamp in starts.iter().skip(1) {
        let gap = stamp.as_micros().saturating_sub(last.as_micros());
        if gap > period {
            segments.push(TimeRange::new(start, last.saturating_add_micros(period))?);
            start = *stamp;
        }
        last = *stamp;
    }
    segments.push(TimeRange::new(start, last.saturating_add_micros(period))?);
    Ok(segments)
}

pub(crate) fn coverage_from_starts(
    starts: &[UtcTimestamp],
    period_us: u64,
) -> Result<DvrMediaCoverage, A2aLabError> {
    let intervals = intervals_from_starts(starts, period_us)?;
    if intervals.is_empty() {
        return Err(A2aLabError::no_recording(
            "source has no retained DVR media",
        ));
    }
    DvrMediaCoverage::new(intervals)
}
