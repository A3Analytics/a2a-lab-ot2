//! Hardware-free OT-2 cutover onto the generic kit semantic engine.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock};

use a2a_lab_dev_kit::{
    A2aLabConfig, ChunkDvrSemanticSearchProvider, DescriptionStageWorker, DvrStoreNamespace,
    EmbeddingStageWorker, IndexingWorkerFaults, IndexingWorkerLane, PageRequest,
    SearchSemanticFramesRequest, SemanticChunkDvrStore, SemanticIndexingConfig, SemanticSchedule,
    SemanticSearchLimits, SemanticSearchQuery, SemanticSearchScope, SemanticStreamSchedule,
    SemanticVideoSearchProvider, SqliteChunkDvrStore, TimeRange, UtcTimestamp, VideoSourceId,
};
use a2a_lab_ot2::IndexedVideoDvr;
use a2a_lab_ot2::lab_config::ResolvedSemanticIndexing;
use a2a_lab_ot2::{
    ExplicitSetting, LabConfigInputs, resolve_lab_config, resolve_semantic_indexing,
};

const FPS_PERIOD_US: i64 = 200_000;
const INTERVAL_US: i64 = 5_000_000;
const ALIGNED_EPOCH_US: i64 = 1_700_000_000_000_000;
const SEMANTIC_LIMITS: &str = r#""limits":{"worker":{"max_batch_items":4,"max_concurrency":2,"max_queue_backlog":64,"max_retry_attempts":3,"job_deadline":"30s","retry_backoff":"1s"},"search":{"top_k":10,"candidate_limit":1000,"max_neighbor_frames":5,"query_window":"2h"},"roi":{"min_edge_pixels":280,"max_edge_pixels":1568,"max_rois_per_frame":8},"storage":{"vector_index_path":"./state/semantic-index","max_disk_bytes":1073741824},"decisions":{"max_batch_items":8,"max_concurrency":2,"job_deadline":"45s","coverage_policy":"on_demand"}}"#;

fn temp_root(name: &str) -> PathBuf {
    std::env::temp_dir().join(format!(
        "a2a-lab-ot2-semantic-cutover-{name}-{}",
        std::process::id()
    ))
}

fn jpeg(tag: u8) -> Vec<u8> {
    vec![0xFF, 0xD8, tag, 0xD9]
}

fn capture_at(offset_us: i64) -> UtcTimestamp {
    UtcTimestamp::from_unix_microseconds(ALIGNED_EPOCH_US + offset_us).unwrap()
}

fn interval_schedule(stream: &str) -> SemanticStreamSchedule {
    SemanticStreamSchedule::with_stages(
        stream,
        SemanticSchedule::Interval {
            micros: u64::try_from(INTERVAL_US).unwrap(),
        },
        SemanticSchedule::Interval {
            micros: u64::try_from(INTERVAL_US).unwrap(),
        },
    )
}

fn semantic_config() -> SemanticIndexingConfig {
    let body = format!(
        r#"{{"schema_version":2,"semantic_indexing":{{"description":{{"provider":"bedrock","model":"custom-luna"}},"embeddings":{{"provider":"bedrock","model":"text-embed-v1","space":{{"dimensions":4,"normalization":"l2_unit","metric":"cosine","generation":1}}}},{SEMANTIC_LIMITS},"decisions":{{"provider":"jev","model":"jev-1.14.0"}}}}}}"#
    );
    A2aLabConfig::parse(&body)
        .unwrap()
        .semantic_indexing
        .unwrap()
        .with_schedules(vec![interval_schedule("camera")])
}

fn write_config(dir: &Path, body: &str) -> PathBuf {
    fs::create_dir_all(dir).unwrap();
    let path = dir.join("a2a-lab.json");
    fs::write(&path, body).unwrap();
    path
}

fn resolve_lab(dir: &Path, body: &str) -> ResolvedSemanticIndexing {
    let path = write_config(dir, body);
    let inputs = LabConfigInputs {
        config_path: ExplicitSetting::cli(Some(path)),
        ..LabConfigInputs::default()
    };
    let resolved = resolve_lab_config(&inputs).unwrap();
    resolve_semantic_indexing(&inputs, &resolved.video)
        .unwrap()
        .expect("semantic")
}

async fn append_offsets(store: &IndexedVideoDvr, source: &VideoSourceId, offsets: &[i64]) {
    for (index, offset) in offsets.iter().copied().enumerate() {
        store
            .append_mjpeg_frame(
                source,
                capture_at(offset),
                u64::try_from(FPS_PERIOD_US).unwrap(),
                &jpeg(u8::try_from(index).unwrap_or(u8::MAX)),
            )
            .await
            .unwrap();
    }
}

async fn run_lanes(store: &IndexedVideoDvr, semantic: &SemanticIndexingConfig, rounds: u32) {
    let desc_live = DescriptionStageWorker::deterministic(
        semantic,
        "desc-live",
        IndexingWorkerFaults::new(),
        IndexingWorkerLane::Live,
    )
    .unwrap();
    let desc_hist = DescriptionStageWorker::deterministic(
        semantic,
        "desc-hist",
        IndexingWorkerFaults::new(),
        IndexingWorkerLane::Backfill,
    )
    .unwrap();
    let embed_live = EmbeddingStageWorker::deterministic(
        semantic,
        "embed-live",
        IndexingWorkerFaults::new(),
        IndexingWorkerLane::Live,
    )
    .unwrap();
    let embed_hist = EmbeddingStageWorker::deterministic(
        semantic,
        "embed-hist",
        IndexingWorkerFaults::new(),
        IndexingWorkerLane::Backfill,
    )
    .unwrap();
    for _ in 0..rounds {
        let _ = desc_live.run_batch(store.engine(), semantic).await.unwrap();
        let _ = desc_hist.run_batch(store.engine(), semantic).await.unwrap();
        let _ = embed_live
            .run_batch(store.engine(), semantic)
            .await
            .unwrap();
        let _ = embed_hist
            .run_batch(store.engine(), semantic)
            .await
            .unwrap();
    }
}

#[tokio::test]
async fn five_fps_capture_does_not_amplify_five_second_schedules() {
    let dir = temp_root("no-amp");
    let _ = fs::remove_dir_all(&dir);
    let store = IndexedVideoDvr::open(&dir, 86_400).await.unwrap();
    let semantic = semantic_config();
    store.set_semantic_indexing_config(&semantic);
    let source = VideoSourceId::new("camera").unwrap();
    let offsets: Vec<i64> = (0..25).map(|i| i * FPS_PERIOD_US).collect();
    append_offsets(&store, &source, &offsets).await;
    let jobs = store.count_indexing_jobs().await.unwrap();
    let eligibility = store.engine().count_stage_eligibility().await.unwrap();
    assert!(jobs <= 4, "jobs={jobs}");
    assert!(eligibility <= 4, "eligibility={eligibility}");
    run_lanes(&store, &semantic, 2).await;
    let live = DescriptionStageWorker::deterministic(
        &semantic,
        "spin-check",
        IndexingWorkerFaults::new(),
        IndexingWorkerLane::Live,
    )
    .unwrap();
    let report = live.run_batch(store.engine(), &semantic).await.unwrap();
    assert_eq!(report.claimed(), 0);
    let _ = fs::remove_dir_all(dir);
}

#[tokio::test]
async fn live_vectors_progress_while_retained_history_drains() {
    let dir = temp_root("lanes");
    let _ = fs::remove_dir_all(&dir);
    let store = IndexedVideoDvr::open(&dir, 86_400).await.unwrap();
    let semantic = semantic_config();
    store.set_semantic_indexing_config(&semantic);
    let source = VideoSourceId::new("camera").unwrap();
    append_offsets(&store, &source, &[0, INTERVAL_US]).await;
    let hist = DescriptionStageWorker::deterministic(
        &semantic,
        "hist",
        IndexingWorkerFaults::new(),
        IndexingWorkerLane::Backfill,
    )
    .unwrap();
    let live = DescriptionStageWorker::deterministic(
        &semantic,
        "live",
        IndexingWorkerFaults::new(),
        IndexingWorkerLane::Live,
    )
    .unwrap();
    let hist_report = hist.run_batch(store.engine(), &semantic).await.unwrap();
    let live_report = live.run_batch(store.engine(), &semantic).await.unwrap();
    assert!(live_report.claimed() > 0 || hist_report.claimed() > 0);
    run_lanes(&store, &semantic, 8).await;
    assert!(store.count_nonempty_embedding_vectors().await.unwrap() >= 1);
    let remaining = store.engine().count_active_indexing_jobs().await.unwrap();
    assert!(remaining < 4);
    let _ = fs::remove_dir_all(dir);
}

#[tokio::test]
async fn restart_resumes_durable_work() {
    let dir = temp_root("restart");
    let _ = fs::remove_dir_all(&dir);
    let source = VideoSourceId::new("camera").unwrap();
    let semantic = semantic_config();
    {
        let store = IndexedVideoDvr::open(&dir, 86_400).await.unwrap();
        store.set_semantic_indexing_config(&semantic);
        append_offsets(&store, &source, &[0, INTERVAL_US]).await;
        run_lanes(&store, &semantic, 8).await;
        assert!(store.count_nonempty_embedding_vectors().await.unwrap() >= 1);
    }
    let store = IndexedVideoDvr::open(&dir, 86_400).await.unwrap();
    store.set_semantic_indexing_config(&semantic);
    assert!(store.count_nonempty_embedding_vectors().await.unwrap() >= 1);
    append_offsets(&store, &source, &[INTERVAL_US * 2]).await;
    run_lanes(&store, &semantic, 8).await;
    assert!(store.count_nonempty_embedding_vectors().await.unwrap() >= 1);
    assert_eq!(
        store.engine().count_active_indexing_jobs().await.unwrap(),
        0
    );
    let _ = fs::remove_dir_all(dir);
}

#[tokio::test]
async fn fresh_semantic_store_rebuilds_from_intact_dvr_and_searches() {
    let dir = temp_root("rebuild");
    let _ = fs::remove_dir_all(&dir);
    let source = VideoSourceId::new("camera").unwrap();
    let semantic = semantic_config();
    let chunk = SqliteChunkDvrStore::open(&dir, 86_400).await.unwrap();
    chunk
        .append_mjpeg_frame(
            &source,
            capture_at(0),
            u64::try_from(FPS_PERIOD_US).unwrap(),
            &jpeg(1),
        )
        .await
        .unwrap();
    assert!(dir.join("index.sqlite").exists());
    let slot = Arc::new(OnceLock::new());
    let engine = SemanticChunkDvrStore::adopt_opened(
        chunk,
        slot,
        DvrStoreNamespace::new(a2a_lab_ot2::videos::dvr::DEFAULT_SEMANTIC_STORE_NAMESPACE).unwrap(),
    )
    .await
    .unwrap();
    let store = IndexedVideoDvr::from_engine(engine).await.unwrap();
    store.set_semantic_indexing_config(&semantic);
    store
        .engine()
        .reconcile_retained_indexing_until_idle(
            &DvrStoreNamespace::new(a2a_lab_ot2::videos::dvr::DEFAULT_SEMANTIC_STORE_NAMESPACE)
                .unwrap(),
        )
        .await
        .unwrap();
    run_lanes(&store, &semantic, 8).await;
    let first: String = sqlx::query_scalar(
        "SELECT frame_id FROM semantic_frame_embeddings ORDER BY frame_id LIMIT 1",
    )
    .fetch_one(store.semantic_pool())
    .await
    .unwrap();
    let search =
        ChunkDvrSemanticSearchProvider::deterministic(store.engine().clone(), semantic.clone())
            .await
            .unwrap();
    let start = capture_at(0);
    let request = SearchSemanticFramesRequest::new(
        SemanticSearchScope::new(
            source,
            Some(TimeRange::new(start, start.saturating_add_micros(10_000_000)).unwrap()),
        )
        .unwrap(),
        SemanticSearchQuery::Text {
            text: format!("desc:{first}"),
        },
        semantic.active_search_provenance().unwrap(),
        SemanticSearchLimits::new(5, 100, 3).unwrap(),
        PageRequest::new(None, 5).unwrap(),
        None,
    )
    .unwrap();
    let result = search.search_semantic_frames(request).await.unwrap();
    assert!(!result.hits().items().is_empty());
    assert!(!result.hits().items()[0].generated_description().is_empty());
    let _ = fs::remove_dir_all(dir);
}

#[tokio::test]
async fn search_hits_carry_configured_lab_source_and_utc_captured_at() {
    let dir = temp_root("kit-links");
    let _ = fs::remove_dir_all(&dir);
    let body = format!(
        r#"{{"schema_version":2,"chat":{{"provider":"openai","model":"chat-only"}},"videos":{{"fps":5,"sources":[{{"id":"camera","device_index":0,"format":"mjpeg","semantic_indexing":{{"description":"5s","embeddings":"5s"}}}}],"dvr":{{"storage_path":"./dvr","retention":"1h"}}}},"semantic_indexing":{{"description":{{"provider":"bedrock","model":"global.openai.gpt-5.6-luna"}},"embeddings":{{"provider":"bedrock","model":"text-embed-v1","space":{{"dimensions":4,"normalization":"l2_unit","metric":"cosine","generation":1}}}},{SEMANTIC_LIMITS},"decisions":{{"provider":"jev","model":"jev-1.14.0"}}}}}}"#
    );
    let semantic = resolve_lab(&dir, &body);
    assert!(
        semantic
            .indexed_source_ids
            .contains(a2a_lab_ot2::camera::CAMERA_SOURCE_ID)
    );
    let source = VideoSourceId::new(a2a_lab_ot2::camera::CAMERA_SOURCE_ID).unwrap();
    let store = IndexedVideoDvr::open(&dir, 86_400).await.unwrap();
    store.set_semantic_indexing_config(&semantic.config);
    let captured = capture_at(0);
    append_offsets(&store, &source, &[0]).await;
    run_lanes(&store, &semantic.config, 8).await;
    let first: String = sqlx::query_scalar(
        "SELECT frame_id FROM semantic_frame_embeddings ORDER BY frame_id LIMIT 1",
    )
    .fetch_one(store.semantic_pool())
    .await
    .unwrap();
    let search = ChunkDvrSemanticSearchProvider::deterministic(
        store.engine().clone(),
        semantic.config.clone(),
    )
    .await
    .unwrap();
    let request = SearchSemanticFramesRequest::new(
        SemanticSearchScope::new(
            source.clone(),
            Some(TimeRange::new(captured, captured.saturating_add_micros(10_000_000)).unwrap()),
        )
        .unwrap(),
        SemanticSearchQuery::Text {
            text: format!("desc:{first}"),
        },
        semantic.config.active_search_provenance().unwrap(),
        SemanticSearchLimits::new(5, 100, 3).unwrap(),
        PageRequest::new(None, 5).unwrap(),
        None,
    )
    .unwrap();
    let result = search.search_semantic_frames(request).await.unwrap();
    let hit = result
        .hits()
        .items()
        .first()
        .expect("readable semantic hit");
    assert_eq!(hit.frame().source_id(), &source);
    assert_eq!(
        hit.frame().source_id().as_str(),
        a2a_lab_ot2::camera::CAMERA_SOURCE_ID
    );
    assert_eq!(hit.captured_at(), captured);
    assert_eq!(hit.frame().captured_at(), captured);
    let rfc3339 = hit.frame().captured_at().to_rfc3339();
    assert!(
        rfc3339.ends_with('Z') || rfc3339.ends_with("+00:00") || rfc3339.ends_with("-00:00"),
        "{rfc3339}"
    );
    UtcTimestamp::parse(&rfc3339).expect("UTC instant");
    let _ = fs::remove_dir_all(dir);
}

#[test]
fn resolved_lab_config_keeps_shared_embedding_identity_and_five_second_schedules() {
    let dir = temp_root("config");
    let body = format!(
        r#"{{"schema_version":2,"chat":{{"provider":"openai","model":"chat-only"}},"videos":{{"fps":5,"sources":[{{"id":"camera","device_index":0,"format":"mjpeg","semantic_indexing":{{"description":"5s","embeddings":"5s"}}}}],"dvr":{{"storage_path":"./dvr","retention":"1h"}}}},"semantic_indexing":{{"description":{{"provider":"bedrock","model":"global.openai.gpt-5.6-luna"}},"embeddings":{{"provider":"bedrock","model":"text-embed-v1","space":{{"dimensions":4,"normalization":"l2_unit","metric":"cosine","generation":1}}}},{SEMANTIC_LIMITS},"decisions":{{"provider":"jev","model":"jev-1.14.0"}}}}}}"#
    );
    let semantic = resolve_lab(&dir, &body);
    let schedule = semantic.config.schedule_for("camera");
    assert_eq!(
        schedule.description(),
        SemanticSchedule::Interval { micros: 5_000_000 }
    );
    assert_eq!(
        schedule.embeddings(),
        SemanticSchedule::Interval { micros: 5_000_000 }
    );
    assert_eq!(schedule.embedding_model(), None);
    assert_eq!(
        semantic.config.embeddings.as_ref().unwrap().model(),
        "text-embed-v1"
    );
    semantic.config.validate_before_startup().unwrap();
    let mismatched = body.replace(
        "\"embeddings\":\"5s\"",
        "\"embeddings\":\"5s\",\"embedding_model\":\"other-embed\"",
    );
    let path = write_config(&dir, &mismatched);
    let inputs = LabConfigInputs {
        config_path: ExplicitSetting::cli(Some(path)),
        ..LabConfigInputs::default()
    };
    let err = resolve_lab_config(&inputs).unwrap_err();
    assert!(err.to_string().contains("embedding_model"));
    let _ = fs::remove_dir_all(dir);
}
