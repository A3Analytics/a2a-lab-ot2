//! Stable camera failures.

use std::fmt;

/// Backend-neutral camera failure.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CameraError {
    /// No matching device is available.
    Unavailable {
        /// Device context.
        device: String,
        /// Why it is unavailable.
        detail: String,
    },
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
        /// Why it is busy.
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
    /// The payload could not be decoded.
    Malformed {
        /// Device context.
        device: String,
        /// Why the payload was rejected.
        detail: String,
    },
    /// Channels did not share one synchronized set.
    Unsynchronized {
        /// Device context.
        device: String,
        /// Timestamp spread.
        detail: String,
    },
}

impl CameraError {
    /// Stable class name.
    #[must_use]
    pub const fn class(&self) -> &'static str {
        match self {
            Self::Unavailable { .. } => "unavailable",
            Self::Permission { .. } => "permission",
            Self::Busy { .. } => "busy",
            Self::UnsupportedProfile { .. } => "unsupported_profile",
            Self::Timeout { .. } => "timeout",
            Self::Malformed { .. } => "malformed",
            Self::Unsynchronized { .. } => "unsynchronized",
        }
    }

    /// Device context carried by every class.
    #[must_use]
    pub fn device(&self) -> &str {
        match self {
            Self::Unavailable { device, .. }
            | Self::Permission { device, .. }
            | Self::Busy { device, .. }
            | Self::UnsupportedProfile { device, .. }
            | Self::Timeout { device, .. }
            | Self::Malformed { device, .. }
            | Self::Unsynchronized { device, .. } => device,
        }
    }
}

impl fmt::Display for CameraError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Unavailable { device, detail }
            | Self::Permission { device, detail }
            | Self::Busy { device, detail }
            | Self::UnsupportedProfile { device, detail }
            | Self::Timeout { device, detail }
            | Self::Malformed { device, detail }
            | Self::Unsynchronized { device, detail } => write!(formatter, "{device}: {detail}"),
        }
    }
}

impl std::error::Error for CameraError {}
