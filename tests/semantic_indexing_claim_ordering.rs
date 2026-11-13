//! Queue claim ordering: playable retained DVR rows before stale manifest history.

use a2a_lab_dev_kit::{
    A2aLabConfig, DvrStoreNamespace, MemoryVideos, SemanticIndexingConfig, VideoSourceId,
};
use a2a_lab_ot2::IndexedVideoDvr;
use a2a_lab_ot2::videos::dvr::{
    DEFAULT_SEMANTIC_STORE_NAMESPACE, DeterministicIndexingWorker, IndexingJobState,
    IndexingWorkerFaults, ManifestSearchCandidateFilter, MediaPayloadStatus, indexing_job_id,
};

fn temp_root(name: &str) -> std::path::PathBuf {
    std::env::temp_dir().join(format!(
        "a2a-lab-ot2-claim-ordering-{name}-{}",
        std::process::id()
    ))
}

async fn open_store(name: &str) -> (std::path::PathBuf, IndexedVideoDvr) {
    let dir = temp_root(name);
    let _ = std::fs::remove_dir_all(&dir);
    let store = IndexedVideoDvr::open(&dir, 86_400).await.unwrap();
    (dir, store)
}

fn store_ns() -> DvrStoreNamespace {
    DvrStoreNamespace::new(DEFAULT_SEMANTIC_STORE_NAMESPACE).unwrap()
}

fn jpeg(tag: u8) -> Vec<u8> {
    vec![0xFF, 0xD8, tag, 0xD9]
}

fn semantic_config() -> SemanticIndexingConfig {
    const BASE: &str = concat!(include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../a2a-lab-dev-kit-rs/tests/fixtures/a2a-lab-v2-semantic-valid.json"
    )),);
    let mut config = A2aLabConfig::parse(BASE).unwrap();
    let semantic = config.semantic_indexing.as_mut().unwrap();
    semantic.limits.worker.max_batch_items = 4;
    semantic.limits.worker.max_concurrency = 2;
    semantic.limits.worker.max_queue_backlog = 64;
    semantic.limits.worker.max_retry_attempts = 3;
    semantic.clone()
}

async fn seed_stale_and_playable(store: &IndexedVideoDvr) -> (String, String, String, String) {
    let source = VideoSourceId::new("camera").unwrap();
    let track = MemoryVideos::mjpeg_track_id(&source).unwrap();
    let base = a2a_lab_ot2::time::now().unwrap();
    let stale_capture = base.saturating_add_micros(-60_000_000);
    let playable_capture = base.saturating_add_micros(-1_000_000);
    store
        .write_legacy_segment(&source, &track, stale_capture, 1_000_000, true, &jpeg(10))
        .await
        .unwrap();
    let (stale_frame, _, _) = store
        .ensure_indexing_at_dvr_key(&store_ns(), &source, &track, stale_capture)
        .await
        .unwrap();
    sqlx::query("DELETE FROM dvr_segments WHERE source_id = ?1 AND start_us = ?2")
        .bind(source.as_str())
        .bind(stale_capture.as_micros())
        .execute(store.media_pool())
        .await
        .unwrap();
    store
        .write_legacy_segment(
            &source,
            &track,
            playable_capture,
            1_000_000,
            true,
            &jpeg(11),
        )
        .await
        .unwrap();
    let (playable_frame, _, _) = store
        .ensure_indexing_at_dvr_key(&store_ns(), &source, &track, playable_capture)
        .await
        .unwrap();
    let stale_job = indexing_job_id(stale_frame.frame_id().as_str());
    let playable_job = indexing_job_id(playable_frame.frame_id().as_str());
    let stale_frame_id = stale_frame.frame_id().as_str().to_owned();
    let playable_frame_id = playable_frame.frame_id().as_str().to_owned();
    (stale_job, playable_job, stale_frame_id, playable_frame_id)
}

#[tokio::test]
async fn claim_prefers_playable_frame_before_older_stale_manifest() {
    let (_dir, store) = open_store("claim-order").await;
    let (stale_job, playable_job, stale_frame_id, _) = seed_stale_and_playable(&store).await;
    let now_us = a2a_lab_ot2::time::now().unwrap().as_micros();
    let claimed = store
        .claim_indexing_jobs("prio", 1, now_us + 60_000_000, now_us)
        .await
        .unwrap();
    assert_eq!(claimed.len(), 1);
    assert_eq!(claimed[0].job_id(), playable_job);
    assert_ne!(claimed[0].job_id(), stale_job);
    let stale_manifest = store.get_manifest_frame(&stale_frame_id).await.unwrap();
    assert_eq!(stale_manifest.payload_status(), MediaPayloadStatus::Missing);
}

#[tokio::test]
async fn classify_expires_stale_jobs_without_starving_playable_claims() {
    let (_dir, store) = open_store("classify").await;
    let (stale_job, playable_job, _, playable_frame_id) = seed_stale_and_playable(&store).await;
    let now_us = a2a_lab_ot2::time::now().unwrap().as_micros();
    let expired = store
        .classify_expired_indexing_jobs_missing_retained_media(now_us, 256)
        .await
        .unwrap();
    assert_eq!(expired, 1);
    assert_eq!(
        store.get_indexing_job(&stale_job).await.unwrap().state(),
        IndexingJobState::Expired
    );
    assert_eq!(
        store
            .get_indexing_job(&stale_job)
            .await
            .unwrap()
            .last_retry_reason(),
        Some("frame_expired")
    );
    assert_eq!(
        store.get_indexing_job(&playable_job).await.unwrap().state(),
        IndexingJobState::Pending
    );
    let semantic = semantic_config();
    let worker = DeterministicIndexingWorker::deterministic(
        &semantic,
        "classify-worker",
        IndexingWorkerFaults::new(),
    )
    .await
    .unwrap();
    worker.run_batch(&store, &semantic).await.unwrap();
    assert_eq!(
        store.get_indexing_job(&playable_job).await.unwrap().state(),
        IndexingJobState::Ready
    );
    let manifest = store.get_manifest_frame(&playable_frame_id).await.unwrap();
    assert_eq!(manifest.payload_status(), MediaPayloadStatus::Available);
    assert!(manifest.embedding().and_then(|e| e.vector()).is_some());
}

#[tokio::test]
async fn restart_after_playable_indexing_keeps_ready_outcome() {
    let dir = temp_root("restart-order");
    let _ = std::fs::remove_dir_all(&dir);
    let playable_frame_id = {
        let store = IndexedVideoDvr::open(&dir, 86_400).await.unwrap();
        let (_, playable_job, _, playable_frame_id) = seed_stale_and_playable(&store).await;
        let semantic = semantic_config();
        let worker = DeterministicIndexingWorker::deterministic(
            &semantic,
            "restart-worker",
            IndexingWorkerFaults::new(),
        )
        .await
        .unwrap();
        worker.run_batch(&store, &semantic).await.unwrap();
        assert_eq!(
            store.get_indexing_job(&playable_job).await.unwrap().state(),
            IndexingJobState::Ready
        );
        playable_frame_id
    };
    let store = IndexedVideoDvr::open(&dir, 86_400).await.unwrap();
    let manifest = store.get_manifest_frame(&playable_frame_id).await.unwrap();
    assert_eq!(manifest.payload_status(), MediaPayloadStatus::Available);
    assert_eq!(
        store
            .get_indexing_job(&indexing_job_id(&playable_frame_id))
            .await
            .unwrap()
            .state(),
        IndexingJobState::Ready
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn indexed_playable_camera_appears_in_semantic_retrieval_scan() {
    let (_dir, store) = open_store("retrieval").await;
    let (_, _, _, playable_frame_id) = seed_stale_and_playable(&store).await;
    let source = VideoSourceId::new("camera").unwrap();
    let semantic = semantic_config();
    DeterministicIndexingWorker::deterministic(
        &semantic,
        "search-worker",
        IndexingWorkerFaults::new(),
    )
    .await
    .unwrap()
    .run_batch(&store, &semantic)
    .await
    .unwrap();
    let manifest = store.get_manifest_frame(&playable_frame_id).await.unwrap();
    let capture = manifest.frame().captured_at();
    let model_space = semantic
        .embeddings
        .as_ref()
        .expect("embeddings config")
        .model_space_identity()
        .unwrap();
    let description_generation = manifest
        .description()
        .expect("ready description")
        .provenance()
        .generation()
        .get();
    let filter = ManifestSearchCandidateFilter {
        description_generation,
        model_space,
    };
    let scan = store
        .scan_semantic_search_candidates(
            source.as_str(),
            capture.as_micros().saturating_sub(1),
            capture.as_micros().saturating_add(2_000_000),
            &filter,
            16,
        )
        .await
        .unwrap();
    assert!(
        scan.records
            .iter()
            .any(|record| record.frame().frame_id().as_str() == playable_frame_id),
        "playable indexed frame should appear in semantic retrieval scan"
    );
}
