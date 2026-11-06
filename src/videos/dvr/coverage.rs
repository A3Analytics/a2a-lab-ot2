//! Retained interval and gap derivation from indexed segment starts.

use a2a_lab_dev_kit::{A2aLabError, TimeRange, UtcTimestamp};

pub(crate) struct IntervalFold {
    period: i64,
    split_after: i64,
    start: Option<UtcTimestamp>,
    last: Option<UtcTimestamp>,
    segments: Vec<TimeRange>,
}

impl IntervalFold {
    pub(crate) fn new(period_us: u64) -> Self {
        let period = i64::try_from(period_us.max(1)).unwrap_or(1);
        Self {
            period,
            // One extra frame period absorbs capture jitter. A longer hole stays a gap.
            split_after: period.saturating_mul(2),
            start: None,
            last: None,
            segments: Vec::new(),
        }
    }

    pub(crate) fn push(&mut self, stamp: UtcTimestamp) -> Result<(), A2aLabError> {
        let Some(last) = self.last else {
            self.start = Some(stamp);
            self.last = Some(stamp);
            return Ok(());
        };
        if stamp.as_micros().saturating_sub(last.as_micros()) > self.split_after {
            let start = self.start.unwrap_or(last);
            self.segments.push(TimeRange::new(
                start,
                last.saturating_add_micros(self.period),
            )?);
            self.start = Some(stamp);
        }
        self.last = Some(stamp);
        Ok(())
    }

    pub(crate) fn finish(mut self) -> Result<Vec<TimeRange>, A2aLabError> {
        if let Some(last) = self.last {
            let start = self.start.unwrap_or(last);
            self.segments.push(TimeRange::new(
                start,
                last.saturating_add_micros(self.period),
            )?);
        }
        Ok(self.segments)
    }
}
