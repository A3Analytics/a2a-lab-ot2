//! Camera backend contract and the Orbbec color adapter.

use std::sync::Arc;
use std::time::Duration;

use crate::frame_sets::ChannelProfile;
use crate::orbbec::{OrbbecCapture, OrbbecCaptureError};

use super::error::CameraError;

/// One discovered device and the profiles it actually advertises.
#[derive(Debug, Clone, PartialEq)]
pub struct CameraDevice {
    backend: String,
    serial: String,
    model: String,
    channels: Vec<ChannelProfile>,
}

impl CameraDevice {
    /// Builds a device description from already validated profiles.
    #[must_use]
    pub fn new(
        backend: impl Into<String>,
        serial: impl Into<String>,
        model: impl Into<String>,
        channels: Vec<ChannelProfile>,
    ) -> Self {
        Self {
            backend: backend.into(),
            serial: serial.into(),
            model: model.into(),
            channels,
        }
    }

    /// Backend name.
    #[must_use]
    pub fn backend(&self) -> &str {
        &self.backend
    }

    /// Device serial.
    #[must_use]
    pub fn serial(&self) -> &str {
        &self.serial
    }

    /// Device model.
    #[must_use]
    pub fn model(&self) -> &str {
        &self.model
    }

    /// Profiles reported by this device.
    #[must_use]
    pub fn channels(&self) -> &[ChannelProfile] {
        &self.channels
    }
}

/// Backend and optional serial chosen before a device is opened.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CameraSelection {
    backend: String,
    serial: Option<String>,
}

impl CameraSelection {
    /// Rejects an empty backend or serial.
    pub fn new(backend: impl Into<String>, serial: Option<&str>) -> Result<Self, CameraError> {
        let backend = backend.into();
        if backend.is_empty() {
            return Err(CameraError::Unavailable {
                device: "camera".to_owned(),
                detail: "camera backend must not be empty".to_owned(),
            });
        }
        if serial.is_some_and(str::is_empty) {
            return Err(CameraError::Unavailable {
                device: backend,
                detail: "camera serial must not be empty".to_owned(),
            });
        }
        Ok(Self {
            backend,
            serial: serial.map(str::to_owned),
        })
    }

    /// Selected backend.
    #[must_use]
    pub fn backend(&self) -> &str {
        &self.backend
    }

    /// Selected serial.
    #[must_use]
    pub fn serial(&self) -> Option<&str> {
        self.serial.as_deref()
    }
}

/// Discovers devices and captures without exposing a vendor type.
pub trait CameraBackend: Send + Sync {
    /// Stable backend name.
    fn id(&self) -> &'static str;

    /// Devices visible to this backend. This may open a device.
    fn discover(&self) -> Result<Vec<CameraDevice>, CameraError>;

    /// One color JPEG from the selected device.
    fn capture_color(
        &self,
        serial: Option<&str>,
        timeout: Duration,
        max_bytes: u64,
    ) -> Result<Vec<u8>, CameraError>;
}

/// Chooses one device. No serial uses the only device. Several devices require a serial.
pub fn select_camera(devices: &[CameraDevice], wanted: Option<&str>) -> Result<usize, CameraError> {
    if devices.is_empty() {
        return Err(CameraError::Unavailable {
            device: "camera".to_owned(),
            detail: "no camera device is connected".to_owned(),
        });
    }
    if let Some(serial) = wanted {
        return devices
            .iter()
            .position(|device| device.serial() == serial)
            .ok_or_else(|| CameraError::Unavailable {
                device: serial.to_owned(),
                detail: format!("serial {serial} was not found"),
            });
    }
    if devices.len() == 1 {
        return Ok(0);
    }
    let serials = devices
        .iter()
        .map(|device| device.serial().to_owned())
        .collect::<Vec<_>>()
        .join(", ");
    Err(CameraError::Unavailable {
        device: "camera".to_owned(),
        detail: format!("multiple camera devices are connected ({serials}); set a camera serial"),
    })
}

/// Orbbec color capture behind [`CameraBackend`].
pub struct OrbbecColorBackend {
    capture: Arc<dyn OrbbecCapture>,
}

impl OrbbecColorBackend {
    /// Wraps an Orbbec capture. The SDK is not loaded until a method is called.
    #[must_use]
    pub fn new(capture: Arc<dyn OrbbecCapture>) -> Self {
        Self { capture }
    }

    /// Color capture that preserves the existing Orbbec error.
    pub fn capture_color_compat(
        &self,
        serial: Option<&str>,
        timeout: Duration,
        max_bytes: u64,
    ) -> Result<Vec<u8>, OrbbecCaptureError> {
        self.capture.capture_color(serial, timeout, max_bytes)
    }
}

impl CameraBackend for OrbbecColorBackend {
    fn id(&self) -> &'static str {
        "orbbec"
    }

    fn discover(&self) -> Result<Vec<CameraDevice>, CameraError> {
        Err(CameraError::Unavailable {
            device: "orbbec".to_owned(),
            detail: "device discovery happens when a frame is requested".to_owned(),
        })
    }

    fn capture_color(
        &self,
        serial: Option<&str>,
        timeout: Duration,
        max_bytes: u64,
    ) -> Result<Vec<u8>, CameraError> {
        self.capture_color_compat(serial, timeout, max_bytes)
            .map_err(|error| map_orbbec(serial.unwrap_or("orbbec-camera"), &error))
    }
}

fn map_orbbec(device: &str, error: &OrbbecCaptureError) -> CameraError {
    let device = device.to_owned();
    let detail = error.to_string();
    match error {
        OrbbecCaptureError::Permission(_) => CameraError::Permission { device, detail },
        OrbbecCaptureError::Timeout => CameraError::Timeout { device, detail },
        OrbbecCaptureError::Malformed(_)
        | OrbbecCaptureError::Empty
        | OrbbecCaptureError::Oversize => CameraError::Malformed { device, detail },
        OrbbecCaptureError::NoColorProfile(_) => CameraError::UnsupportedProfile { device, detail },
        OrbbecCaptureError::Sdk(message) if message.to_ascii_lowercase().contains("busy") => {
            CameraError::Busy { device, detail }
        }
        OrbbecCaptureError::NoDevice
        | OrbbecCaptureError::MissingSerial(_)
        | OrbbecCaptureError::Ambiguous(_)
        | OrbbecCaptureError::Library(_)
        | OrbbecCaptureError::Sdk(_) => CameraError::Unavailable { device, detail },
    }
}
