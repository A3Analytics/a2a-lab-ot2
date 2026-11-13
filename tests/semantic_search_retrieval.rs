//! Optional semantic search time scope over the DVR manifest.

use std::collections::HashSet;
use std::path::PathBuf;

use a2a_lab_dev_kit::{
    A2aLabConfig, PageRequest, ProvenanceGeneration, SearchSemanticFramesRequest,
    SemanticProvenanceSelector, SemanticSearchLimits, SemanticSearchQuery, SemanticSearchScope,
    SemanticVideoSearchProvider, TimeRange, UtcTimestamp, VideoSourceId,
};
use a2a_lab_ot2::IndexedVideoDvr;
use a2a_lab_ot2::lab_config::ResolvedSemanticIndexing;
use a2a_lab_ot2::videos::dvr::{
    DEFAULT_SEMANTIC_STORE_NAMESPACE, DvrSearchEmbedder, DvrSemanticSearchProvider,
    IndexingCheckpoint,
};
use a2a_lab_ot2::videos::semantic_indexing::build_deterministic_providers;

fn semantic_config() -> a2a_lab_dev_kit::SemanticIndexingConfig {
    const BASE: &str = concat!(include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../a2a-lab-dev-kit-rs/tests/fixtures/a2a-lab-v2-semantic-valid.json"
    )),);
    A2aLabConfig::parse(BASE)
        .unwrap()
        .semantic_indexing
        .unwrap()
}

fn temp_root(name: &str) -> PathBuf {
    std::env::temp_dir().join(format!(
        "a2a-lab-ot2-semantic-search-{name}-{}",
        std::process::id()
    ))
}

async fn open_store(name: &str) -> (PathBuf, IndexedVideoDvr) {
    let dir = temp_root(name);
    let _ = std::fs::remove_dir_all(&dir);
    let store = IndexedVideoDvr::open(&dir, 86_400).await.unwrap();
    (dir, store)
}

fn store_ns() -> a2a_lab_dev_kit::DvrStoreNamespace {
    a2a_lab_dev_kit::DvrStoreNamespace::new(DEFAULT_SEMANTIC_STORE_NAMESPACE).unwrap()
}

fn jpeg(tag: u8) -> Vec<u8> {
    vec![0xFF, 0xD8, tag, 0xD9]
}

async fn seed_indexed(
    store: &IndexedVideoDvr,
    source: &VideoSourceId,
    capture: UtcTimestamp,
    tag: u8,
    text: &str,
) -> String {
    let track = a2a_lab_dev_kit::MemoryVideos::mjpeg_track_id(source).unwrap();
    store
        .append_mjpeg_frame(source, capture, 1_000_000, &jpeg(tag))
        .await
        .unwrap();
    let (frame, _) = store
        .register_manifest_at_dvr_key(&store_ns(), source, &track, capture)
        .await
        .unwrap();
    let description = a2a_lab_dev_kit::FrameDescriptionProvenance::new(
        "bedrock",
        "global.openai.gpt-5.6-luna",
        a2a_lab_dev_kit::Sha256Digest::hash_bytes(b"prompt-v1"),
        a2a_lab_dev_kit::Sha256Digest::hash_bytes(b"preprocess-v1"),
        Some(a2a_lab_dev_kit::FrameRoiTransform::new([0.0, 0.0, 1.0, 1.0]).unwrap()),
        a2a_lab_dev_kit::Sha256Digest::hash_bytes(text.as_bytes()),
        ProvenanceGeneration::new(1).unwrap(),
    )
    .unwrap();
    store
        .upsert_frame_description(&frame, text, &description)
        .await
        .unwrap();
    let space = semantic_config()
        .embeddings
        .as_ref()
        .unwrap()
        .model_space_identity()
        .unwrap();
    let embedding = a2a_lab_dev_kit::TextEmbeddingProvenance::new(
        space.provider(),
        space.model(),
        space.dimensions(),
        space.normalization(),
        space.metric(),
        space.generation(),
        a2a_lab_dev_kit::Sha256Digest::hash_bytes(text.as_bytes()),
    )
    .unwrap();
    let values = vec![0.1_f32; space.dimensions() as usize];
    let vector = a2a_lab_dev_kit::FrameEmbeddingVector::new(values, space.dimensions()).unwrap();
    store
        .upsert_frame_embedding(
            &frame,
            &space,
            &embedding,
            &vector,
            IndexingCheckpoint::Ready,
        )
        .await
        .unwrap();
    frame.frame_id().as_str().to_owned()
}

async fn provider_for(store: IndexedVideoDvr) -> DvrSemanticSearchProvider {
    let semantic = semantic_config();
    let providers = build_deterministic_providers(&semantic).await.unwrap();
    let embedder = DvrSearchEmbedder::Deterministic(std::sync::Arc::new(providers.embedding));
    let indexed = ResolvedSemanticIndexing {
        config: semantic.clone(),
        vector_index_path: temp_root("vector").join("index"),
        sqlite_path: temp_root("vector").join("semantic.sqlite"),
        indexed_source_ids: HashSet::from(["camera".to_owned()]),
    };
    DvrSemanticSearchProvider::new(store, &indexed, embedder).unwrap()
}

fn selector() -> SemanticProvenanceSelector {
    let space = semantic_config()
        .embeddings
        .as_ref()
        .unwrap()
        .model_space_identity()
        .unwrap();
    SemanticProvenanceSelector::new(space, ProvenanceGeneration::new(1).unwrap()).unwrap()
}

fn text_search(
    scope: SemanticSearchScope,
    limits: SemanticSearchLimits,
) -> SearchSemanticFramesRequest {
    SearchSemanticFramesRequest::new(
        scope,
        SemanticSearchQuery::Text {
            text: "pipette".to_owned(),
        },
        selector(),
        limits,
        PageRequest::new(None, 10).unwrap(),
        None,
    )
    .unwrap()
}

#[tokio::test]
async fn omitted_range_searches_all_retained_candidates() {
    let (_dir, store) = open_store("unbounded").await;
    let source = VideoSourceId::new("camera").unwrap();
    let t0 = UtcTimestamp::parse("2024-06-01T10:00:00Z").unwrap();
    let t1 = UtcTimestamp::parse("2024-06-01T12:00:00Z").unwrap();
    seed_indexed(&store, &source, t0, 1, "early pipette view").await;
    seed_indexed(&store, &source, t1, 2, "late pipette move").await;
    let provider = provider_for(store).await;
    let narrow = SemanticSearchScope::new(
        source.clone(),
        Some(
            TimeRange::new(
                UtcTimestamp::parse("2024-06-01T11:00:00Z").unwrap(),
                UtcTimestamp::parse("2024-06-01T13:00:00Z").unwrap(),
            )
            .unwrap(),
        ),
    )
    .unwrap();
    let bounded = provider
        .search_semantic_frames(text_search(
            narrow,
            SemanticSearchLimits::new(10, 100, 3).unwrap(),
        ))
        .await
        .unwrap();
    assert_eq!(bounded.hits().items().len(), 1);
    let unbounded = provider
        .search_semantic_frames(text_search(
            SemanticSearchScope::new(source, None).unwrap(),
            SemanticSearchLimits::new(10, 100, 3).unwrap(),
        ))
        .await
        .unwrap();
    assert_eq!(unbounded.hits().items().len(), 2);
}

#[tokio::test]
async fn candidate_limit_flags_budget_exhaustion() {
    let (_dir, store) = open_store("budget").await;
    let source = VideoSourceId::new("camera").unwrap();
    let base = UtcTimestamp::parse("2024-06-01T12:00:00Z").unwrap();
    for index in 0..3 {
        let capture = base.saturating_add_micros(i64::from(index) * 1_000_000);
        seed_indexed(
            &store,
            &source,
            capture,
            u8::try_from(10 + index).unwrap(),
            &format!("pipette frame {index}"),
        )
        .await;
    }
    let provider = provider_for(store).await;
    let result = provider
        .search_semantic_frames(text_search(
            SemanticSearchScope::new(source, None).unwrap(),
            SemanticSearchLimits::new(10, 1, 3).unwrap(),
        ))
        .await
        .unwrap();
    assert!(result.candidate_budget_exhausted());
}
