//! Vendor-neutral camera discovery and capture.
//!
//! Backends report portable channel profiles. Callers do not see Orbbec,
//! `RealSense`, or operating-system camera types.

mod backend;
mod error;
mod orbbec_set;
mod png;
mod session;

pub use orbbec_set::{OrbbecFrameCapture, OrbbecStream};
pub use session::{SessionPump, SharedSession, StereoGap, StreamingDevice, TARGET_FPS};

pub use backend::{
    CameraBackend, CameraDevice, CameraSelection, OrbbecColorBackend, select_camera,
};
pub use error::CameraError;
