//! Retained camera payload resolution aligned with Browse playback.

use a2a_lab_dev_kit::{
    A2aLabConfig, DvrFrameAtRequest, DvrMediaPayload, DvrMediaStore, DvrStoreNamespace,
    MemoryVideos, SemanticIndexingConfig, Sha256Digest, VideoSourceId,
};
use a2a_lab_ot2::IndexedVideoDvr;
use a2a_lab_ot2::videos::dvr::{
    DEFAULT_SEMANTIC_STORE_NAMESPACE, DeterministicIndexingWorker, IndexingJobState,
    IndexingWorkerFaults, ManifestSearchCandidateFilter, MediaPayloadStatus, indexing_job_id,
};

fn temp_root(name: &str) -> std::path::PathBuf {
    std::env::temp_dir().join(format!(
        "a2a-lab-ot2-retained-camera-{name}-{}",
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

async fn worker_for(semantic: &SemanticIndexingConfig) -> DeterministicIndexingWorker {
    DeterministicIndexingWorker::deterministic(
        semantic,
        "retained-camera-worker",
        IndexingWorkerFaults::new(),
    )
    .await
    .unwrap()
}

fn fresh_capture() -> a2a_lab_dev_kit::UtcTimestamp {
    a2a_lab_ot2::time::now().unwrap()
}

#[tokio::test]
async fn legacy_only_historical_camera_is_available_and_playable() {
    let (_dir, store) = open_store("legacy-only").await;
    let source = VideoSourceId::new("camera").unwrap();
    let track = MemoryVideos::mjpeg_track_id(&source).unwrap();
    let capture = fresh_capture();
    let legacy = jpeg(41);
    store
        .write_legacy_segment(&source, &track, capture, 1_000_000, true, &legacy)
        .await
        .unwrap();
    let (frame, _, _) = store
        .ensure_indexing_at_dvr_key(&store_ns(), &source, &track, capture)
        .await
        .unwrap();
    let manifest = store
        .get_manifest_frame(frame.frame_id().as_str())
        .await
        .unwrap();
    assert_eq!(manifest.payload_status(), MediaPayloadStatus::Available);
    assert_eq!(
        *manifest.frame().payload_digest(),
        Sha256Digest::hash_bytes(&legacy)
    );
    let played = store
        .frame_at(DvrFrameAtRequest::new(source, track, capture, None))
        .await
        .unwrap();
    assert!(matches!(
        played.payload(),
        DvrMediaPayload::MjpegFrame(bytes) if *bytes == legacy
    ));
}

#[tokio::test]
async fn dual_storage_uses_legacy_when_chunk_checksum_metadata_is_wrong() {
    let (_dir, store) = open_store("dual-legacy").await;
    let source = VideoSourceId::new("camera").unwrap();
    let track = MemoryVideos::mjpeg_track_id(&source).unwrap();
    let capture = fresh_capture();
    let legacy = jpeg(51);
    let chunk = jpeg(52);
    store
        .write_legacy_segment(&source, &track, capture, 1_000_000, true, &legacy)
        .await
        .unwrap();
    store
        .append_mjpeg_frame(&source, capture, 1_000_000, &chunk)
        .await
        .unwrap();
    sqlx::query("UPDATE dvr_frames SET payload_crc32 = 0 WHERE source_id = ?1 AND start_us = ?2")
        .bind(source.as_str())
        .bind(capture.as_micros())
        .execute(store.media_pool())
        .await
        .unwrap();
    let played = store
        .frame_at(DvrFrameAtRequest::new(
            source.clone(),
            track.clone(),
            capture,
            None,
        ))
        .await
        .unwrap();
    assert!(matches!(
        played.payload(),
        DvrMediaPayload::MjpegFrame(bytes) if *bytes == legacy
    ));
    let (frame, _, job_created) = store
        .ensure_indexing_at_dvr_key(&store_ns(), &source, &track, capture)
        .await
        .unwrap();
    assert!(job_created);
    let manifest = store
        .get_manifest_frame(frame.frame_id().as_str())
        .await
        .unwrap();
    assert_eq!(manifest.payload_status(), MediaPayloadStatus::Available);
    assert_eq!(
        *manifest.frame().payload_digest(),
        Sha256Digest::hash_bytes(&legacy)
    );
    let semantic = semantic_config();
    let worker = worker_for(&semantic).await;
    worker.run_batch(&store, &semantic).await.unwrap();
    let job = store
        .get_indexing_job(&indexing_job_id(frame.frame_id().as_str()))
        .await
        .unwrap();
    assert_eq!(job.state(), IndexingJobState::Ready);
}

#[tokio::test]
async fn restart_preserves_available_payload_and_indexing_outcome() {
    let dir = temp_root("restart");
    let _ = std::fs::remove_dir_all(&dir);
    let source = VideoSourceId::new("camera").unwrap();
    let track = MemoryVideos::mjpeg_track_id(&source).unwrap();
    let capture = fresh_capture();
    let legacy = jpeg(61);
    let frame_id;
    {
        let store = IndexedVideoDvr::open(&dir, 86_400).await.unwrap();
        store
            .write_legacy_segment(&source, &track, capture, 1_000_000, true, &legacy)
            .await
            .unwrap();
        let (frame, _, _) = store
            .ensure_indexing_at_dvr_key(&store_ns(), &source, &track, capture)
            .await
            .unwrap();
        frame_id = frame.frame_id().as_str().to_owned();
        let semantic = semantic_config();
        worker_for(&semantic)
            .await
            .run_batch(&store, &semantic)
            .await
            .unwrap();
    }
    let store = IndexedVideoDvr::open(&dir, 86_400).await.unwrap();
    let manifest = store.get_manifest_frame(&frame_id).await.unwrap();
    assert_eq!(manifest.payload_status(), MediaPayloadStatus::Available);
    assert_eq!(
        store
            .get_indexing_job(&indexing_job_id(&frame_id))
            .await
            .unwrap()
            .state(),
        IndexingJobState::Ready
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn indexed_historical_camera_is_search_retrieval_candidate() {
    let (_dir, store) = open_store("search-hit").await;
    let source = VideoSourceId::new("camera").unwrap();
    let track = MemoryVideos::mjpeg_track_id(&source).unwrap();
    let capture = fresh_capture();
    store
        .write_legacy_segment(&source, &track, capture, 1_000_000, true, &jpeg(71))
        .await
        .unwrap();
    let (frame, _, _) = store
        .ensure_indexing_at_dvr_key(&store_ns(), &source, &track, capture)
        .await
        .unwrap();
    let frame_id = frame.frame_id().as_str().to_owned();
    let semantic = semantic_config();
    worker_for(&semantic)
        .await
        .run_batch(&store, &semantic)
        .await
        .unwrap();
    let manifest = store.get_manifest_frame(&frame_id).await.unwrap();
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
            .any(|record| record.frame().frame_id().as_str() == frame_id),
        "historical camera revision should appear in the semantic retrieval scan"
    );
}
