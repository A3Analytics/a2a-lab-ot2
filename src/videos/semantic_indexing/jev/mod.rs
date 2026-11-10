//! Opt-in `TypeSafe` Jev HTTP frame decision adapter (image evidence only).

mod adapter;
mod factory;
mod http;
mod limits;
mod media;
mod questions;
mod smoke;
mod wire;

pub use adapter::{
    TypeSafeJevFrameDecisionEvaluator, jev_api_base, jev_smoke_enabled, live_jev_allowed,
};
pub use factory::{
    LiveDecisionEvaluationWorker, build_deterministic_decision_worker, build_live_decision_worker,
    jev_live_worker_enabled, map_decision_worker_failure,
};
pub use http::JevHttpClient;
pub use media::{EvidenceJpegDecisionMedia, InjectedJevDecisionMedia, JevDecisionMedia};
pub use smoke::{JevSmokeAdoptionGates, JevSmokeReport, run_jev_smoke};
