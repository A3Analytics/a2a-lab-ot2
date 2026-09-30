//! UTC timestamps for log and metric records.

use a2a_lab_sdk::{SdkError, UtcTimestamp};

/// Current UTC instant as an SDK timestamp.
pub fn now() -> Result<UtcTimestamp, SdkError> {
    UtcTimestamp::parse(&jiff::Timestamp::now().to_string())
}

/// Parses an Opentrons RFC 3339 timestamp, accepting `+00:00` or `Z`.
pub fn parse_timestamp(value: &str) -> Result<UtcTimestamp, SdkError> {
    UtcTimestamp::parse(value).or_else(|_| {
        let trimmed = value.trim();
        if let Some(prefix) = trimmed.strip_suffix("+00:00") {
            UtcTimestamp::parse(&format!("{prefix}Z"))
        } else {
            Err(SdkError::invalid(
                "timestamp",
                "must be an RFC 3339 UTC timestamp",
            ))
        }
    })
}
