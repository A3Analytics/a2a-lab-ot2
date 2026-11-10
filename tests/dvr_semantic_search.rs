//! Bounded DVR semantic retrieval and ranking.

use std::collections::HashSet;

use a2a_lab_dev_kit::{
    A2aLabConfig, DecisionHistoryPolicy, DecisionOutcome, DecisionSearchFilter, DecisionSubject,
    DvrStoreNamespace, EmbeddingMetric, FakeEmbeddingBehavior, FakeTextEmbeddingProvider,
    FrameDescriptionProvenance, FrameEmbeddingVector, FrameRoiTransform, JevProducerIdentity,
    LabPredicate, MemoryVideos, ModelSpaceIdentity, PageRequest, ProvenanceGeneration,
    SavedFrameRef, SearchSemanticFramesRequest, SemanticIndexingConfig, SemanticProvenanceSelector,
    SemanticSearchLimits, SemanticSearchQuery, SemanticSearchScope, SemanticVideoSearchProvider,
    Sha256Digest, TextEmbeddingLimits, TextEmbeddingProvenance, TimeRange, UtcTimestamp,
    VectorNormalization, VideoSourceId,
};
use a2a_lab_ot2::IndexedVideoDvr;
use a2a_lab_ot2::lab_config::ResolvedSemanticIndexing;
use a2a_lab_ot2::videos::dvr::{
    DEFAULT_SEMANTIC_STORE_NAMESPACE, DvrSearchEmbedder, DvrSemanticSearchProvider,
    IndexingCheckpoint,
};
use a2a_lab_ot2::videos::semantic_indexing::{embedding_provider_config, text_embedding_limits};

const SEMANTIC_LIMITS: &str = r#""limits":{"worker":{"max_batch_items":4,"max_concurrency":2,"max_queue_backlog":64,"max_retry_attempts":3,"job_deadline":"30s","retry_backoff":"1s"},"search":{"top_k":10,"candidate_limit":1000,"max_neighbor_frames":5,"query_window":"2h"},"roi":{"min_edge_pixels":280,"max_edge_pixels":1568,"max_rois_per_frame":8},"storage":{"vector_index_path":"./state/semantic-index","max_disk_bytes":1073741824},"decisions":{"max_batch_items":8,"max_concurrency":2,"job_deadline":"45s","coverage_policy":"on_demand"}}"#;

fn semantic_config_json() -> String {
    format!(
        r#"{{"schema_version":2,"semantic_indexing":{{"description":{{"provider":"bedrock","model":"custom-luna"}},"embeddings":{{"provider":"bedrock","model":"text-embed-v1","space":{{"dimensions":4,"normalization":"l2_unit","metric":"cosine"}}}},{SEMANTIC_LIMITS},"decisions":{{"provider":"jev","model":"jev-1.14.0"}}}}}}"#
    )
}

fn parse_semantic() -> SemanticIndexingConfig {
    let parsed = A2aLabConfig::parse(&semantic_config_json()).unwrap();
    parsed.semantic_indexing.unwrap()
}

fn resolved_semantic(source: &str) -> ResolvedSemanticIndexing {
    let config = parse_semantic();
    ResolvedSemanticIndexing {
        config,
        vector_index_path: std::path::PathBuf::from("./state/semantic-index"),
        indexed_source_ids: HashSet::from([source.to_owned()]),
    }
}

fn store_ns() -> DvrStoreNamespace {
    DvrStoreNamespace::new(DEFAULT_SEMANTIC_STORE_NAMESPACE).unwrap()
}

fn jpeg(tag: u8) -> Vec<u8> {
    vec![0xFF, 0xD8, tag, 0xD9]
}

fn sample_description(text: &str, generation: u64) -> FrameDescriptionProvenance {
    FrameDescriptionProvenance::new(
        "bedrock",
        "global.openai.gpt-5.6-luna",
        Sha256Digest::hash_bytes(b"prompt-v1"),
        Sha256Digest::hash_bytes(b"preprocess-v1"),
        Some(FrameRoiTransform::new([0.0, 0.0, 1.0, 1.0]).unwrap()),
        Sha256Digest::hash_bytes(text.as_bytes()),
        ProvenanceGeneration::new(generation).unwrap(),
    )
    .unwrap()
}

fn sample_model_space() -> ModelSpaceIdentity {
    ModelSpaceIdentity::new(
        "bedrock",
        "text-embed-v1",
        4,
        VectorNormalization::L2Unit,
        EmbeddingMetric::Cosine,
        ProvenanceGeneration::new(1).unwrap(),
    )
    .unwrap()
}

fn sample_embedding(text: &str) -> TextEmbeddingProvenance {
    TextEmbeddingProvenance::new(
        "bedrock",
        "text-embed-v1",
        4,
        VectorNormalization::L2Unit,
        EmbeddingMetric::Cosine,
        ProvenanceGeneration::new(1).unwrap(),
        Sha256Digest::hash_bytes(text.as_bytes()),
    )
    .unwrap()
}

async fn open_store(name: &str) -> (std::path::PathBuf, IndexedVideoDvr) {
    let dir = std::env::temp_dir().join(format!(
        "a2a-lab-ot2-semantic-search-{name}-{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    let store = IndexedVideoDvr::open(&dir, 86_400).await.unwrap();
    (dir, store)
}

fn embedder(semantic: &SemanticIndexingConfig) -> DvrSearchEmbedder {
    let embeddings = semantic.embeddings.as_ref().unwrap();
    let embedding_config = embedding_provider_config(embeddings).unwrap();
    DvrSearchEmbedder::Deterministic(std::sync::Arc::new(FakeTextEmbeddingProvider::new(
        embedding_config,
        text_embedding_limits(&semantic.limits),
        FakeEmbeddingBehavior::Ok,
    )))
}

fn provider(store: &IndexedVideoDvr, source: &str) -> DvrSemanticSearchProvider {
    let resolved = resolved_semantic(source);
    let embed = embedder(&resolved.config);
    DvrSemanticSearchProvider::new(store.clone(), &resolved, embed).unwrap()
}

async fn index_frame(
    store: &IndexedVideoDvr,
    source: &VideoSourceId,
    capture_offset_us: i64,
    payload_tag: u8,
    text: &str,
    vector: FrameEmbeddingVector,
) -> SavedFrameRef {
    let track = MemoryVideos::mjpeg_track_id(source).unwrap();
    let capture = a2a_lab_ot2::time::now()
        .unwrap()
        .saturating_add_micros(capture_offset_us);
    store
        .append_mjpeg_frame(source, capture, 1_000_000, &jpeg(payload_tag))
        .await
        .unwrap();
    let (frame, _) = store
        .register_manifest_at_dvr_key(&store_ns(), source, &track, capture)
        .await
        .unwrap();
    store
        .upsert_frame_description(&frame, text, &sample_description(text, 1))
        .await
        .unwrap();
    store
        .upsert_frame_embedding(
            &frame,
            &sample_model_space(),
            &sample_embedding(text),
            &vector,
            IndexingCheckpoint::Ready,
        )
        .await
        .unwrap();
    frame
}

fn scope_for(
    source: &VideoSourceId,
    start: UtcTimestamp,
    end: UtcTimestamp,
) -> SemanticSearchScope {
    SemanticSearchScope::new(source.clone(), Some(TimeRange::new(start, end).unwrap())).unwrap()
}

fn selector() -> SemanticProvenanceSelector {
    SemanticProvenanceSelector::new(sample_model_space(), ProvenanceGeneration::new(1).unwrap())
        .unwrap()
}

fn search_request(
    scope: SemanticSearchScope,
    query: SemanticSearchQuery,
    limits: SemanticSearchLimits,
    page: PageRequest,
) -> SearchSemanticFramesRequest {
    SearchSemanticFramesRequest::new(scope, query, selector(), limits, page, None).unwrap()
}

#[tokio::test]
async fn text_query_returns_ranked_hits_with_complete_payload() {
    let (dir, store) = open_store("hits").await;
    let source = VideoSourceId::new("cam-a").unwrap();
    let provider = provider(&store, "cam-a");
    let better = index_frame(
        &store,
        &source,
        1_000_000,
        1,
        "pipette tip visible",
        FrameEmbeddingVector::new(vec![1.0, 0.0, 0.0, 0.0], 4).unwrap(),
    )
    .await;
    let worse = index_frame(
        &store,
        &source,
        2_000_000,
        2,
        "empty deck",
        FrameEmbeddingVector::new(vec![0.2, 0.8, 0.0, 0.0], 4).unwrap(),
    )
    .await;
    let start = better.captured_at();
    let end = worse.captured_at().saturating_add_micros(5_000_000);
    let request = search_request(
        scope_for(&source, start, end),
        SemanticSearchQuery::Text {
            text: "find tip".to_owned(),
        },
        SemanticSearchLimits::new(5, 100, 5).unwrap(),
        PageRequest::new(None, 5).unwrap(),
    );
    let result = provider.search_semantic_frames(request).await.unwrap();
    assert_eq!(result.hits().items().len(), 2);
    assert!(
        result.hits().items()[0].retrieval().value() > result.hits().items()[1].retrieval().value()
    );
    assert_eq!(
        result.hits().items()[0].generated_description(),
        "pipette tip visible"
    );
    assert!(matches!(
        result.hits().items()[0].media_availability(),
        a2a_lab_dev_kit::SemanticFrameMediaAvailability::Ready
    ));
    assert!(!result.hits().items()[0].neighbors().frames().is_empty());
    let _ = std::fs::remove_dir_all(dir);
}

#[tokio::test]
async fn ties_break_on_frame_id_after_score() {
    let (dir, store) = open_store("ties").await;
    let source = VideoSourceId::new("cam-a").unwrap();
    let provider = provider(&store, "cam-a");
    let first = index_frame(
        &store,
        &source,
        10_000_000,
        3,
        "alpha",
        FrameEmbeddingVector::new(vec![1.0, 0.0, 0.0, 0.0], 4).unwrap(),
    )
    .await;
    let second = index_frame(
        &store,
        &source,
        20_000_000,
        4,
        "beta",
        FrameEmbeddingVector::new(vec![1.0, 0.0, 0.0, 0.0], 4).unwrap(),
    )
    .await;
    let scope = scope_for(
        &source,
        first.captured_at(),
        second.captured_at().saturating_add_micros(5_000_000),
    );
    let result = provider
        .search_semantic_frames(search_request(
            scope,
            SemanticSearchQuery::Text {
                text: "query".to_owned(),
            },
            SemanticSearchLimits::new(2, 50, 3).unwrap(),
            PageRequest::new(None, 2).unwrap(),
        ))
        .await
        .unwrap();
    let ids: Vec<_> = result
        .hits()
        .items()
        .iter()
        .map(|hit| hit.frame().frame_id().as_str().to_owned())
        .collect();
    assert_ne!(ids[0], ids[1]);
    assert_eq!(ids[0].cmp(&ids[1]), std::cmp::Ordering::Less);
    let _ = std::fs::remove_dir_all(dir);
}

#[tokio::test]
async fn rejects_unconfigured_source_scope() {
    let (dir, store) = open_store("source-scope").await;
    let provider = provider(&store, "cam-a");
    let foreign = VideoSourceId::new("cam-b").unwrap();
    let now = a2a_lab_ot2::time::now().unwrap();
    let end = now.saturating_add_micros(10_000_000);
    let err = provider
        .search_semantic_frames(search_request(
            scope_for(&foreign, now, end),
            SemanticSearchQuery::Text {
                text: "tip".to_owned(),
            },
            SemanticSearchLimits::pilot(),
            PageRequest::new(None, 10).unwrap(),
        ))
        .await
        .unwrap_err();
    assert!(err.to_string().contains("semantic indexing scope"));
    let _ = std::fs::remove_dir_all(dir);
}

#[tokio::test]
async fn stale_search_current_revision_is_not_retrieved() {
    let (dir, store) = open_store("stale").await;
    let source = VideoSourceId::new("cam-a").unwrap();
    let provider = provider(&store, "cam-a");
    let current = index_frame(
        &store,
        &source,
        30_000_000,
        5,
        "current frame",
        FrameEmbeddingVector::new(vec![1.0, 0.0, 0.0, 0.0], 4).unwrap(),
    )
    .await;
    let stale_capture = current.captured_at().saturating_add_micros(1);
    store
        .append_mjpeg_frame(&source, stale_capture, 1_000_000, &jpeg(6))
        .await
        .unwrap();
    let track = MemoryVideos::mjpeg_track_id(&source).unwrap();
    let (stale, _) = store
        .register_manifest_at_dvr_key(&store_ns(), &source, &track, stale_capture)
        .await
        .unwrap();
    let text = "superseded revision";
    store
        .upsert_frame_description(&stale, text, &sample_description(text, 1))
        .await
        .unwrap();
    store
        .upsert_frame_embedding(
            &stale,
            &sample_model_space(),
            &sample_embedding(text),
            &FrameEmbeddingVector::new(vec![1.0, 0.0, 0.0, 0.0], 4).unwrap(),
            IndexingCheckpoint::Ready,
        )
        .await
        .unwrap();
    sqlx::query("UPDATE semantic_frame_revisions SET search_current = 0 WHERE frame_id = ?1")
        .bind(stale.frame_id().as_str())
        .execute(store.sqlite_pool())
        .await
        .unwrap();
    let scope = scope_for(
        &source,
        current.captured_at(),
        stale_capture.saturating_add_micros(5_000_000),
    );
    let result = provider
        .search_semantic_frames(search_request(
            scope,
            SemanticSearchQuery::Text {
                text: "current".to_owned(),
            },
            SemanticSearchLimits::new(5, 100, 3).unwrap(),
            PageRequest::new(None, 5).unwrap(),
        ))
        .await
        .unwrap();
    assert_eq!(result.hits().items().len(), 1);
    assert_eq!(
        result.hits().items()[0].frame().frame_id(),
        current.frame_id()
    );
    let _ = std::fs::remove_dir_all(dir);
}

#[tokio::test]
async fn mismatched_model_space_is_rejected() {
    let (dir, store) = open_store("mismatch").await;
    let source = VideoSourceId::new("cam-a").unwrap();
    let provider = provider(&store, "cam-a");
    let frame = index_frame(
        &store,
        &source,
        40_000_000,
        7,
        "one",
        FrameEmbeddingVector::new(vec![1.0, 0.0, 0.0, 0.0], 4).unwrap(),
    )
    .await;
    let other_space = ModelSpaceIdentity::new(
        "bedrock",
        "text-embed-v1",
        4,
        VectorNormalization::L2Unit,
        EmbeddingMetric::Cosine,
        ProvenanceGeneration::new(2).unwrap(),
    )
    .unwrap();
    let scope = scope_for(
        &source,
        frame.captured_at(),
        frame.captured_at().saturating_add_micros(5_000_000),
    );
    let err = provider
        .search_semantic_frames(
            SearchSemanticFramesRequest::new(
                scope,
                SemanticSearchQuery::Text {
                    text: "one".to_owned(),
                },
                SemanticProvenanceSelector::new(other_space, ProvenanceGeneration::new(1).unwrap())
                    .unwrap(),
                SemanticSearchLimits::pilot(),
                PageRequest::new(None, 10).unwrap(),
                None,
            )
            .unwrap(),
        )
        .await
        .unwrap_err();
    assert!(err.to_string().contains("model space"));
    let _ = std::fs::remove_dir_all(dir);
}

#[tokio::test]
async fn candidate_budget_exhausted_when_scan_is_truncated() {
    let (dir, store) = open_store("budget").await;
    let source = VideoSourceId::new("cam-a").unwrap();
    let provider = provider(&store, "cam-a");
    let mut start = None;
    let mut end = None;
    for idx in 0..3 {
        let frame = index_frame(
            &store,
            &source,
            50_000_000 + i64::from(idx) * 1_000_000,
            u8::try_from(10 + idx).unwrap(),
            &format!("frame {idx}"),
            FrameEmbeddingVector::new(vec![1.0, 0.0, 0.0, 0.0], 4).unwrap(),
        )
        .await;
        start.get_or_insert(frame.captured_at());
        end = Some(frame.captured_at().saturating_add_micros(5_000_000));
    }
    let result = provider
        .search_semantic_frames(search_request(
            scope_for(&source, start.unwrap(), end.unwrap()),
            SemanticSearchQuery::Text {
                text: "frame".to_owned(),
            },
            SemanticSearchLimits::new(3, 2, 3).unwrap(),
            PageRequest::new(None, 3).unwrap(),
        ))
        .await
        .unwrap();
    assert!(result.candidate_budget_exhausted());
    assert_eq!(result.hits().items().len(), 2);
    let _ = std::fs::remove_dir_all(dir);
}

#[tokio::test]
async fn pagination_slices_ranked_results() {
    let (dir, store) = open_store("page").await;
    let source = VideoSourceId::new("cam-a").unwrap();
    let provider = provider(&store, "cam-a");
    let mut start = None;
    let mut end = None;
    for idx in 0u32..3 {
        let frame = index_frame(
            &store,
            &source,
            70_000_000 + i64::from(idx) * 1_000_000,
            u8::try_from(20 + idx).unwrap(),
            &format!("match {idx}"),
            FrameEmbeddingVector::new(
                {
                    let weight = f32::from(u16::try_from(idx).expect("idx fits u16"));
                    vec![1.0 - weight * 0.1_f32, weight * 0.1_f32, 0.0, 0.0]
                },
                4,
            )
            .unwrap(),
        )
        .await;
        start.get_or_insert(frame.captured_at());
        end = Some(frame.captured_at().saturating_add_micros(5_000_000));
    }
    let scope = scope_for(&source, start.unwrap(), end.unwrap());
    let page0 = provider
        .search_semantic_frames(search_request(
            scope.clone(),
            SemanticSearchQuery::Text {
                text: "match".to_owned(),
            },
            SemanticSearchLimits::new(3, 50, 3).unwrap(),
            PageRequest::new(None, 2).unwrap(),
        ))
        .await
        .unwrap();
    let cursor = page0.hits().next_cursor().expect("next page");
    let page1 = provider
        .search_semantic_frames(search_request(
            scope,
            SemanticSearchQuery::Text {
                text: "match".to_owned(),
            },
            SemanticSearchLimits::new(3, 50, 3).unwrap(),
            PageRequest::new(Some(cursor.to_owned()), 2).unwrap(),
        ))
        .await
        .unwrap();
    assert_eq!(page0.hits().items().len(), 2);
    assert_eq!(page1.hits().items().len(), 1);
    let _ = std::fs::remove_dir_all(dir);
}

#[tokio::test]
async fn provider_failure_surfaces_explicit_error() {
    let (dir, store) = open_store("provider-fail").await;
    let source = VideoSourceId::new("cam-a").unwrap();
    let resolved = resolved_semantic("cam-a");
    let embeddings = resolved.config.embeddings.as_ref().unwrap();
    let embedding_config = embedding_provider_config(embeddings).unwrap();
    let embedder =
        DvrSearchEmbedder::Deterministic(std::sync::Arc::new(FakeTextEmbeddingProvider::new(
            embedding_config,
            TextEmbeddingLimits::pilot(),
            FakeEmbeddingBehavior::Timeout,
        )));
    let provider = DvrSemanticSearchProvider::new(store.clone(), &resolved, embedder).unwrap();
    let now = a2a_lab_ot2::time::now().unwrap();
    let err = provider
        .search_semantic_frames(search_request(
            scope_for(&source, now, now.saturating_add_micros(10_000_000)),
            SemanticSearchQuery::Text {
                text: "fail".to_owned(),
            },
            SemanticSearchLimits::pilot(),
            PageRequest::new(None, 10).unwrap(),
        ))
        .await
        .unwrap_err();
    assert!(err.to_string().contains("deadline") || err.to_string().contains("unavailable"));
    let _ = std::fs::remove_dir_all(dir);
}

#[tokio::test]
async fn incomplete_indexing_surfaces_on_pending_rows() {
    let (dir, store) = open_store("incomplete").await;
    let source = VideoSourceId::new("cam-a").unwrap();
    let provider = provider(&store, "cam-a");
    let track = MemoryVideos::mjpeg_track_id(&source).unwrap();
    let capture = a2a_lab_ot2::time::now()
        .unwrap()
        .saturating_add_micros(90_000_000);
    store
        .append_mjpeg_frame(&source, capture, 1_000_000, &jpeg(90))
        .await
        .unwrap();
    store
        .register_manifest_at_dvr_key(&store_ns(), &source, &track, capture)
        .await
        .unwrap();
    let scope = scope_for(&source, capture, capture.saturating_add_micros(10_000_000));
    let result = provider
        .search_semantic_frames(search_request(
            scope,
            SemanticSearchQuery::Text {
                text: "pending".to_owned(),
            },
            SemanticSearchLimits::pilot(),
            PageRequest::new(None, 10).unwrap(),
        ))
        .await
        .unwrap();
    assert!(result.incomplete_indexing());
    assert!(result.hits().items().is_empty());
    let _ = std::fs::remove_dir_all(dir);
}

#[tokio::test]
async fn decision_filter_is_not_applied_by_provider() {
    let (dir, store) = open_store("decision-filter").await;
    let source = VideoSourceId::new("cam-a").unwrap();
    let provider = provider(&store, "cam-a");
    let frame = index_frame(
        &store,
        &source,
        100_000_000,
        95,
        "unfiltered hit",
        FrameEmbeddingVector::new(vec![1.0, 0.0, 0.0, 0.0], 4).unwrap(),
    )
    .await;
    let filter = DecisionSearchFilter::new(
        LabPredicate::pipette_tip_attached_v1(),
        Some(DecisionSubject::PipetteChannel(
            a2a_lab_dev_kit::PipetteChannelSubject::new("left", "ch1", None).unwrap(),
        )),
        Some(DecisionOutcome::Present),
        JevProducerIdentity::default_production(),
        DecisionHistoryPolicy::LatestAuthorizedProduction,
    )
    .unwrap();
    let scope = scope_for(
        &source,
        frame.captured_at(),
        frame.captured_at().saturating_add_micros(5_000_000),
    );
    let request = SearchSemanticFramesRequest::new(
        scope,
        SemanticSearchQuery::Text {
            text: "unfiltered".to_owned(),
        },
        selector(),
        SemanticSearchLimits::pilot(),
        PageRequest::new(None, 10).unwrap(),
        Some(filter),
    )
    .unwrap();
    let result = provider.search_semantic_frames(request).await.unwrap();
    assert_eq!(result.hits().items().len(), 1);
    let _ = std::fs::remove_dir_all(dir);
}
