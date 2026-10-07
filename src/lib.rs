//! Example lab agent wrapping an Opentrons OT-2 robot-server simulator.

pub mod agent;
pub mod memory;
pub mod oidc;
pub mod opentrons;
pub mod page;
pub mod sila;
pub mod time;

pub use agent::{
    DEFAULT_ANTHROPIC_MODEL, DEFAULT_BEDROCK_MODEL, DEFAULT_OPENAI_MODEL, LabAgent, ModelProvider,
    selected_model,
};
pub use memory::ConversationStore;
pub use oidc::{OidcConfig, with_oidc};
pub use opentrons::{OpentronsClient, OpentronsLab};
pub use sila::{
    DEFAULT_SILA_PORT, DEFAULT_SILA_UUID, PreparedSila, SilaConfig, prepare_sila, sila_server,
};
