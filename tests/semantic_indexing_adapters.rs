//! Recorded-service tests for Luna description and text-embedding adapters.

use std::sync::Arc;

use a2a_lab_dev_kit::{
    A2aLabConfig, DEFAULT_BEDROCK_DESCRIPTION_MODEL, DEFAULT_OPENAI_DESCRIPTION_MODEL,
    DvrStoreNamespace, DvrTrackId, EmbeddingMetric, FakeDescriptionBehavior, FakeEmbeddingBehavior,
    FakeFrameDescriptionProvider, FakeTextEmbeddingProvider, FrameDescriptionBackend,
    FrameDescriptionKey, FrameDescriptionLimits, FrameDescriptionProvider,
    FrameDescriptionProviderConfig, FrameDescriptionRequest, ModelSpaceIdentity,
    ProvenanceGeneration, SavedFrameRef, SemanticIndexingConfig, SemanticIndexingFailure,
    Sha256Digest, TextEmbeddingKey, TextEmbeddingLimits, TextEmbeddingProvider,
    TextEmbeddingProviderConfig, TextEmbeddingRequest, UtcTimestamp, VectorNormalization,
    VideoSourceId, validate_description_batch, validate_embedding_batch,
};
use a2a_lab_ot2::agent::{DEFAULT_BEDROCK_MODEL, DEFAULT_OPENAI_MODEL};
use a2a_lab_ot2::videos::semantic_indexing::{
    InjectedDescriptionMedia, SemanticIndexingAdapterMode,
    build_deterministic_providers_for_source, build_semantic_indexing_providers,
    default_open_luna_description_provider, description_provider_config, embedding_provider_config,
};

const SEMANTIC_LIMITS: &str = r#""limits":{"worker":{"max_batch_items":8,"max_concurrency":4,"max_queue_backlog":5000,"max_retry_attempts":5,"job_deadline":"30s","retry_backoff":"5s"},"search":{"top_k":10,"candidate_limit":1000,"max_neighbor_frames":5,"query_window":"2h"},"roi":{"min_edge_pixels":280,"max_edge_pixels":1568,"max_rois_per_frame":8},"storage":{"vector_index_path":"./state/semantic-index","max_disk_bytes":1073741824},"decisions":{"max_batch_items":8,"max_concurrency":2,"job_deadline":"45s","coverage_policy":"on_demand"}}"#;

fn frame(payload: &[u8]) -> SavedFrameRef {
    SavedFrameRef::from_payload_bytes(
        DvrStoreNamespace::new("chupacabra.dvr").unwrap(),
        VideoSourceId::new("deck-left").unwrap(),
        DvrTrackId::new("mjpeg-0").unwrap(),
        UtcTimestamp::parse("2024-06-01T12:00:00Z").unwrap(),
        payload,
        None,
    )
    .unwrap()
}

fn semantic_config(document: &str) -> SemanticIndexingConfig {
    A2aLabConfig::parse(document)
        .unwrap()
        .semantic_indexing
        .unwrap()
}

fn description_request(payload: &[u8], generation: u64) -> FrameDescriptionRequest {
    let key = FrameDescriptionKey::new(
        frame(payload),
        None,
        ProvenanceGeneration::new(generation).unwrap(),
    );
    FrameDescriptionRequest::new(FrameDescriptionLimits::pilot(), key, 1024, 640 * 480).unwrap()
}

#[test]
fn luna_defaults_match_agent_constants_independent_of_chat() {
    assert_eq!(DEFAULT_BEDROCK_MODEL, DEFAULT_BEDROCK_DESCRIPTION_MODEL);
    assert_eq!(DEFAULT_OPENAI_MODEL, DEFAULT_OPENAI_DESCRIPTION_MODEL);

    let bedrock = default_open_luna_description_provider();
    assert_eq!(bedrock.model(), DEFAULT_BEDROCK_DESCRIPTION_MODEL);

    let openai_doc = format!(
        r#"{{"schema_version":2,"chat":{{"provider":"openai","model":"chat-only"}},"semantic_indexing":{{"description":{{"provider":"openai"}},"embeddings":{{"provider":"bedrock","model":"text-embed-v1","space":{{"dimensions":4,"normalization":"l2_unit","metric":"cosine","generation":1}}}},{SEMANTIC_LIMITS},"decisions":{{"provider":"jev","model":"jev-1.14.0"}}}}}}"#
    );
    let semantic = semantic_config(&openai_doc);
    let description = description_provider_config(&semantic.description).unwrap();
    assert_eq!(description.model(), DEFAULT_OPENAI_DESCRIPTION_MODEL);
    assert_ne!(description.model(), "chat-only");
}

#[test]
fn explicit_description_model_override_is_recorded() {
    let document = format!(
        r#"{{"schema_version":2,"semantic_indexing":{{"description":{{"provider":"bedrock","model":"custom-luna"}},"embeddings":{{"provider":"bedrock","model":"text-embed-v1","space":{{"dimensions":4,"normalization":"l2_unit","metric":"cosine","generation":1}}}},{SEMANTIC_LIMITS},"decisions":{{"provider":"jev","model":"jev-1.14.0"}}}}}}"#
    );
    let semantic = semantic_config(&document);
    let config = description_provider_config(&semantic.description).unwrap();
    assert_eq!(config.model(), "custom-luna");
    assert_eq!(config.backend(), FrameDescriptionBackend::Bedrock);
}

#[tokio::test]
async fn source_model_overrides_wire_independent_deterministic_providers() {
    let document = format!(
        r#"{{"schema_version":2,"videos":{{"sources":[{{"id":"camera","format":"mjpeg","device_index":1,"semantic_indexing":{{"description":"5s","description_model":"luna-camera","embeddings":"1m","embedding_model":"embed-camera"}}}},{{"id":"external-camera","format":"mjpeg","device_index":2,"semantic_indexing":{{"description":"1.5s","description_model":"luna-external","embeddings":"5s","embedding_model":"embed-external"}}}}]}},"semantic_indexing":{{"embeddings":{{"provider":"bedrock","model":"global-embed","space":{{"dimensions":4,"normalization":"l2_unit","metric":"cosine","generation":1}}}},{SEMANTIC_LIMITS},"decisions":{{"provider":"jev","model":"jev-1.14.0"}}}}}}"#
    );
    let config = A2aLabConfig::parse(&document).unwrap();
    let semantic = config.semantic_indexing.unwrap();
    let sources = config.videos.unwrap().sources;
    let camera = build_deterministic_providers_for_source(
        &semantic,
        sources[0].semantic_indexing.as_ref().unwrap(),
    )
    .await
    .unwrap();
    let external = build_deterministic_providers_for_source(
        &semantic,
        sources[1].semantic_indexing.as_ref().unwrap(),
    )
    .await
    .unwrap();
    assert_eq!(camera.description.config().model(), "luna-camera");
    assert_eq!(external.description.config().model(), "luna-external");
    assert_eq!(camera.embedding.config().model(), "embed-camera");
    assert_eq!(external.embedding.config().model(), "embed-external");
}

#[tokio::test]
async fn deterministic_factory_round_trip_carries_provenance() {
    let document = format!(
        r#"{{"schema_version":2,"semantic_indexing":{{"embeddings":{{"provider":"bedrock","model":"text-embed-v1","space":{{"dimensions":4,"normalization":"l2_unit","metric":"cosine","generation":1}}}},{SEMANTIC_LIMITS},"decisions":{{"provider":"jev","model":"jev-1.14.0"}}}}}}"#
    );
    let semantic = semantic_config(&document);
    let media = Arc::new(InjectedDescriptionMedia::new());
    let providers = build_semantic_indexing_providers(
        &semantic,
        media,
        SemanticIndexingAdapterMode::Deterministic,
    )
    .await
    .unwrap();

    let limits = FrameDescriptionLimits::pilot();
    let requests = vec![description_request(b"a", 1), description_request(b"b", 1)];
    let responses = providers
        .description
        .describe_frames(requests.clone())
        .await
        .unwrap();
    let validated = validate_description_batch(limits, &requests, &responses).unwrap();
    let provenance = validated[0].to_provenance().unwrap();
    assert_eq!(
        provenance.resolved_model(),
        providers.description.config().model()
    );
}

#[tokio::test]
async fn fake_description_failures_are_typed() {
    let config = FrameDescriptionProviderConfig::new(
        FrameDescriptionBackend::Bedrock,
        None::<&str>,
        Sha256Digest::hash_bytes(b"p"),
        Sha256Digest::hash_bytes(b"pp"),
    )
    .unwrap();
    let provider = FakeFrameDescriptionProvider::new(
        config,
        FrameDescriptionLimits::pilot(),
        FakeDescriptionBehavior::Empty,
    );
    let request = description_request(b"x", 1);
    let responses = provider
        .describe_frames(vec![request.clone()])
        .await
        .unwrap();
    let error = validate_description_batch(
        FrameDescriptionLimits::pilot(),
        std::slice::from_ref(&request),
        &responses,
    )
    .unwrap_err();
    assert_eq!(error, SemanticIndexingFailure::EmptyDescription);
}

#[tokio::test]
async fn embedding_maps_text_into_configured_space() {
    let space = ModelSpaceIdentity::new(
        "bedrock",
        "text-embed-v1",
        4,
        VectorNormalization::L2Unit,
        EmbeddingMetric::Cosine,
        ProvenanceGeneration::new(1).unwrap(),
    )
    .unwrap();
    let config = TextEmbeddingProviderConfig::new("bedrock", "text-embed-v1", space).unwrap();
    let provider = FakeTextEmbeddingProvider::new(
        config.clone(),
        TextEmbeddingLimits::pilot(),
        FakeEmbeddingBehavior::Ok,
    );
    let description_key =
        FrameDescriptionKey::new(frame(b"t"), None, ProvenanceGeneration::new(1).unwrap());
    let key = TextEmbeddingKey::PersistedDescription(description_key);
    let request =
        TextEmbeddingRequest::new(TextEmbeddingLimits::pilot(), key, "visible tip", 3).unwrap();
    let responses = provider.embed_texts(vec![request.clone()]).await.unwrap();
    validate_embedding_batch(
        &config,
        TextEmbeddingLimits::pilot(),
        &[request],
        &responses,
    )
    .unwrap();
}

#[tokio::test]
async fn cancelled_embedding_provider_reports_cancelled() {
    let document = format!(
        r#"{{"schema_version":2,"semantic_indexing":{{"embeddings":{{"provider":"bedrock","model":"text-embed-v1","space":{{"dimensions":4,"normalization":"l2_unit","metric":"cosine","generation":1}}}},{SEMANTIC_LIMITS},"decisions":{{"provider":"jev","model":"jev-1.14.0"}}}}}}"#
    );
    let semantic = semantic_config(&document);
    let embedding_config = embedding_provider_config(semantic.embeddings.as_ref().unwrap()).unwrap();
    let provider = FakeTextEmbeddingProvider::new(
        embedding_config,
        TextEmbeddingLimits::pilot(),
        FakeEmbeddingBehavior::Timeout,
    );
    let key = TextEmbeddingKey::SearchQuery(Sha256Digest::hash_bytes(b"query"));
    let request =
        TextEmbeddingRequest::new(TextEmbeddingLimits::pilot(), key, "pipette", 1).unwrap();
    let error = provider.embed_texts(vec![request]).await.unwrap_err();
    assert_eq!(error, SemanticIndexingFailure::DeadlineExceeded);
}
