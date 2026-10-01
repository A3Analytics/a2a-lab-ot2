//! UTC timestamps for log and metric records.

use std::str::FromStr;

use a2a_lab_dev_kit::{A2aLabError, UtcTimestamp};

/// Current UTC instant as a dev kit timestamp.
pub fn now() -> Result<UtcTimestamp, A2aLabError> {
    UtcTimestamp::parse(&jiff::Timestamp::now().to_string())
}

/// Parses an Opentrons RFC 3339 timestamp, accepting `+00:00` or `Z`.
pub fn parse_timestamp(value: &str) -> Result<UtcTimestamp, A2aLabError> {
    UtcTimestamp::parse(value).or_else(|_| {
        let trimmed = value.trim();
        if let Some(prefix) = trimmed.strip_suffix("+00:00") {
            UtcTimestamp::parse(&format!("{prefix}Z"))
        } else {
            Err(A2aLabError::invalid(
                "timestamp",
                "must be an RFC 3339 UTC timestamp",
            ))
        }
    })
}

/// Converts a Unix microsecond timestamp from journald JSON.
pub fn from_unix_microseconds(micros: i64) -> Result<UtcTimestamp, A2aLabError> {
    let timestamp = jiff::Timestamp::from_microsecond(micros)
        .map_err(|_| A2aLabError::invalid("timestamp", "out of range"))?;
    parse_timestamp(&timestamp.to_string())
}

/// Shifts an RFC 3339 UTC timestamp by whole seconds.
pub fn shift_seconds(stamp: UtcTimestamp, seconds: i64) -> Result<UtcTimestamp, A2aLabError> {
    let timestamp = jiff::Timestamp::from_str(&stamp.to_string())
        .map_err(|_| A2aLabError::invalid("timestamp", "unparseable"))?;
    let nanos = timestamp
        .as_nanosecond()
        .saturating_add(i128::from(seconds).saturating_mul(1_000_000_000));
    let shifted = jiff::Timestamp::from_nanosecond(nanos)
        .map_err(|_| A2aLabError::invalid("timestamp", "out of range"))?;
    parse_timestamp(&shifted.to_string())
}
