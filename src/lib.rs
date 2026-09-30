//! Example lab agent wrapping an Opentrons OT-2 robot-server simulator.

pub mod opentrons;
pub mod page;
pub mod time;

pub use opentrons::{OpentronsClient, OpentronsLab};
