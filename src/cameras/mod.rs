//! Vendor-neutral camera discovery and capture.
//!
//! Backends report portable channel profiles.

mod backend;
mod error;

pub use backend::{CameraBackend, CameraDevice, CameraSelection, select_camera};
pub use error::CameraError;
