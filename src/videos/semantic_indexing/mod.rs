//! Luna frame-description and text-embedding adapters (retrieval metadata only).

mod bundle;
mod config;
mod credentials;
mod description;
mod embedding;
mod factory;
pub mod jev;
mod limits;
mod media;
mod prompt;
mod serve;
mod smoke;

pub use bundle::SemanticIndexingProviders;
pub use config::{
    default_open_luna_description_provider, default_open_luna_embedding_provider,
    description_provider_config, description_provider_config_for_model, embedding_provider_config,
    embedding_provider_config_for_model,
};
pub use credentials::redacted_credential_env;
pub use description::LunaFrameDescriptionProvider;
pub use embedding::ConfiguredTextEmbeddingProvider;
pub use factory::{SemanticIndexingAdapterMode, build_semantic_indexing_providers};
pub use factory::{
    build_deterministic_description_provider, build_deterministic_providers,
    build_deterministic_providers_for_source, build_live_description_provider,
    build_live_providers_for_source,
};
pub use jev::{
    EvidenceJpegDecisionMedia, InjectedJevDecisionMedia, JevDecisionMedia, JevHttpClient,
    JevSmokeAdoptionGates, JevSmokeReport, LiveDecisionEvaluationWorker,
    TypeSafeJevFrameDecisionEvaluator, build_deterministic_decision_worker,
    build_live_decision_worker, jev_api_base, jev_live_worker_enabled, jev_smoke_enabled,
    live_jev_allowed, map_decision_worker_failure, run_jev_smoke,
};
pub use limits::{frame_description_limits, text_embedding_limits};
pub use media::{EvidenceJpegMedia, FrameDescriptionMedia, InjectedDescriptionMedia};
pub use prompt::{
    DESCRIPTION_PREPROCESSING_LABEL, DESCRIPTION_PROMPT, description_preprocessing_digest,
    description_prompt_digest,
};
pub use serve::{SemanticServeBundle, SemanticServeInjections, prepare_semantic_serve_bundle};
pub use smoke::{IndexingSmokeReport, run_indexing_smoke};
