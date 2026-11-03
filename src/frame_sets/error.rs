//! Recoverable frame-set failures.

use std::fmt;

/// Failure from the portable frame-set contract.
#[derive(Debug, Clone, PartialEq)]
pub enum FrameSetError {
    /// A value failed validation.
    Invalid {
        /// Field that failed.
        field: &'static str,
        /// Why the value was rejected.
        detail: String,
    },
    /// The named record does not exist.
    NotFound {
        /// Record kind.
        kind: &'static str,
        /// Missing identifier.
        id: String,
    },
    /// The device or store cannot be used.
    Unavailable(String),
    /// The device rejected access.
    Permission {
        /// Device context.
        device: String,
        /// Why access was denied.
        detail: String,
    },
    /// The device is in use.
    Busy {
        /// Device context.
        device: String,
        /// Why the device is busy.
        detail: String,
    },
    /// The requested profile is not advertised.
    UnsupportedProfile {
        /// Device context.
        device: String,
        /// Profile that was rejected.
        detail: String,
    },
    /// Capture exceeded its deadline.
    Timeout {
        /// Device context.
        device: String,
        /// What timed out.
        detail: String,
    },
    /// A payload or metadata value is not usable.
    Malformed(String),
    /// Channel timestamps are outside the source tolerance.
    Unsynchronized(String),
    /// A payload exceeds the configured limit.
    Oversize,
    /// Persisted history cannot be served.
    Corrupt(String),
}

impl FrameSetError {
    /// Stable class shared by every backend and wire mapping.
    #[must_use]
    pub const fn class(&self) -> &'static str {
        match self {
            Self::Invalid { .. } => "invalid",
            Self::NotFound { .. } => "not_found",
            Self::Unavailable(_) | Self::Corrupt(_) => "unavailable",
            Self::Permission { .. } => "permission",
            Self::Busy { .. } => "busy",
            Self::UnsupportedProfile { .. } => "unsupported_profile",
            Self::Timeout { .. } => "timeout",
            Self::Malformed(_) | Self::Oversize => "malformed",
            Self::Unsynchronized(_) => "unsynchronized",
        }
    }

    pub(crate) fn invalid(field: &'static str, detail: impl Into<String>) -> Self {
        Self::Invalid {
            field,
            detail: detail.into(),
        }
    }
}

impl fmt::Display for FrameSetError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Invalid { field, detail } => write!(formatter, "{field}: {detail}"),
            Self::NotFound { kind, id } => write!(formatter, "{kind} `{id}` was not found"),
            Self::Unavailable(detail) | Self::Malformed(detail) | Self::Unsynchronized(detail) => {
                formatter.write_str(detail)
            }
            Self::Permission { device, detail }
            | Self::Busy { device, detail }
            | Self::UnsupportedProfile { device, detail }
            | Self::Timeout { device, detail } => write!(formatter, "{device}: {detail}"),
            Self::Oversize => formatter.write_str("frame payload exceeds the configured limit"),
            Self::Corrupt(detail) => write!(formatter, "frame-set history is unusable: {detail}"),
        }
    }
}

impl std::error::Error for FrameSetError {}
