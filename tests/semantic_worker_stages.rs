//! Independent description and embedding worker lanes.

use a2a_lab_dev_kit::{
    A2aLabConfig, DvrStoreNamespace, MemoryVideos, SemanticSchedule, VideoSourceId,
};
use a2a_lab_ot2::IndexedVideoDvr;
use a2a_lab_ot2::videos::dvr::{
    DEFAULT_SEMANTIC_STORE_NAMESPACE, DeterministicDescriptionOnlyWorker,
    DeterministicEmbeddingOnlyWorker, IndexingWorkerFaults,
};

fn semantic_with_intervals() -> a2a_lab_dev_kit::SemanticIndexingConfig {
    const BASE: &str = concat!(include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../a2a-lab-dev-kit-rs/tests/fixtures/a2a-lab-v2-semantic-valid.json"
    )),);
    let config = A2aLabConfig::parse(BASE).unwrap();
    let semantic = config.semantic_indexing.unwrap();
    match config.videos.as_ref() {
        Some(videos) => a2a_lab_dev_kit::attach_stream_schedules(semantic, &videos.sources),
        None => semantic,
    }
}

fn store_ns() -> DvrStoreNamespace {
    DvrStoreNamespace::new(DEFAULT_SEMANTIC_STORE_NAMESPACE).unwrap()
}

async fn open_store(name: &str) -> (std::path::PathBuf, IndexedVideoDvr) {
    let dir = std::env::temp_dir().join(format!(
        "a2a-lab-ot2-stage-workers-{name}-{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    let store = IndexedVideoDvr::open(&dir, 86_400).await.unwrap();
    (dir, store)
}

fn jpeg(tag: u8) -> Vec<u8> {
    vec![0xFF, 0xD8, tag, 0xD9]
}

#[tokio::test]
async fn description_and_embedding_workers_commit_independently() {
    let (_dir, store) = open_store("isolation").await;
    let semantic = semantic_with_intervals();
    store.set_semantic_indexing_config(semantic.clone());
    let source = VideoSourceId::new("camera").unwrap();
    let capture = a2a_lab_ot2::time::now().unwrap();
    store
        .append_mjpeg_frame(&source, capture, 1_000_000, &jpeg(1))
        .await
        .unwrap();
    let track = MemoryVideos::mjpeg_track_id(&source).unwrap();
    store
        .ensure_indexing_at_dvr_key(&store_ns(), &source, &track, capture)
        .await
        .unwrap();

    let description = DeterministicDescriptionOnlyWorker::new(
        a2a_lab_ot2::videos::semantic_indexing::build_deterministic_providers(&semantic)
            .await
            .unwrap()
            .description,
        semantic.limits.worker.clone(),
        "desc-worker",
        IndexingWorkerFaults::new(),
    );
    let desc_report = description.run_batch(&store, &semantic).await.unwrap();
    assert_eq!(desc_report.descriptions_committed(), 1);
    assert_eq!(desc_report.embeddings_committed(), 0);

    let embedding = DeterministicEmbeddingOnlyWorker::new(
        a2a_lab_ot2::videos::semantic_indexing::build_deterministic_providers(&semantic)
            .await
            .unwrap()
            .embedding,
        semantic.limits.worker.clone(),
        "embed-worker",
        IndexingWorkerFaults::new(),
    );
    let embed_report = embedding.run_batch(&store, &semantic).await.unwrap();
    assert_eq!(embed_report.embeddings_committed(), 1);
    assert_eq!(embed_report.descriptions_committed(), 0);
}

#[tokio::test]
async fn schedule_aware_registration_skips_between_interval_frames() {
    let (_dir, store) = open_store("schedule-jobs").await;
    let semantic = semantic_with_intervals();
    let schedule = semantic.schedule_for("camera");
    assert!(matches!(
        schedule.description(),
        SemanticSchedule::Interval { .. }
    ));
    store.set_semantic_indexing_config(semantic.clone());
    let source = VideoSourceId::new("camera").unwrap();
    let track = MemoryVideos::mjpeg_track_id(&source).unwrap();
    let base = a2a_lab_ot2::time::now().unwrap();
    for offset in [0_i64, 200_000, 400_000, 600_000, 800_000] {
        let capture = base.saturating_add_micros(offset);
        store
            .append_mjpeg_frame(
                &source,
                capture,
                1_000_000,
                &jpeg(u8::try_from(offset).unwrap_or(u8::MAX)),
            )
            .await
            .unwrap();
        store
            .ensure_indexing_at_dvr_key(&store_ns(), &source, &track, capture)
            .await
            .unwrap();
    }
    let job_count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM semantic_indexing_jobs")
        .fetch_one(store.semantic_pool())
        .await
        .unwrap();
    assert!(
        job_count <= 2,
        "expected schedule-aware registration to skip most 200ms frames, got {job_count}"
    );
}
