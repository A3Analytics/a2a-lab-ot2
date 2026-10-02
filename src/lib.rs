//! Example lab agent wrapping an Opentrons OT-2 robot-server simulator.

use std::path::PathBuf;

pub mod agent;
pub mod memory;
pub mod opentrons;
pub mod page;
pub mod time;

pub use agent::{
    DEFAULT_ANTHROPIC_MODEL, DEFAULT_BEDROCK_MODEL, DEFAULT_OPENAI_MODEL, LabAgent, ModelProvider,
    selected_model,
};
pub use memory::ConversationStore;
pub use opentrons::{OpentronsClient, OpentronsLab};

/// Bundled serial-dilution protocol used by `run_serial_dilution`.
#[must_use]
pub fn default_protocol() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("protocols/serial_dilution.py")
}
