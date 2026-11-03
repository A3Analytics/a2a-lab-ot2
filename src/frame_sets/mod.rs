//! Portable synchronized frame-set contract.
//!
//! This module has no OT-2, camera-vendor, protocol, or dev-kit model imports.
//! A later extraction can move it unchanged into the dev kit.

mod catalog;
mod error;
mod history;
mod id;
mod model;
mod payload;
mod store;

pub use catalog::FrameSetCatalog;
pub use error::FrameSetError;
pub use history::{DEFAULT_FRAME_SET_DB, FramePage, FrameSetHistory};
pub use id::{FrameChannelId, FrameSetId, FrameSetSourceId};
pub use model::{
    CaptureRequest, CapturedFrameSet, ChannelCalibration, ChannelMetadata, ChannelProfile,
    ChannelRole, DepthScale, DepthUnit, Distortion, Extrinsics, FrameChannel, FrameChannelParts,
    FrameSet, FrameSetMetadata, FrameSetSource, Intrinsics, frame_channel,
};
pub use store::{FrameSetCapture, FrameSetStore, MemoryFrameSetStore, publish_capture};
