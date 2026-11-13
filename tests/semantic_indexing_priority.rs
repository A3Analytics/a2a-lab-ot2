//! Durable live-vs-backfill indexing priority, fairness, and nonblocking capture.

use std::collections::HashSet;

use a2a_lab_dev_kit::{
    A2aLabConfig, DvrStoreNamespace, DvrTrackId, MemoryVideos, ProvenanceGeneration,
    SemanticEmbeddingsConfig, TimeRange, VideoSourceId,
};
use a2a_lab_ot2::IndexedVideoDvr;
use a2a_lab_ot2::videos::dvr::{
    DEFAULT_SEMANTIC_STORE_NAMESPACE, DeterministicIndexingWorker, INDEXING_PRIORITY_BACKFILL,
    INDEXING_PRIORITY_LIVE, IndexingCheckpoint, IndexingJobRecord, IndexingJobState,
    IndexingWorkerFaults, IndexingWorkerLane, ManifestSearchCandidateFilter, indexing_job_id,
};
use a2a_lab_ot2::videos::semantic_indexing::indexing_claim_batch_size;

fn temp_root(name: &str) -> std::path::PathBuf {
    std::env::temp_dir().join(format!(
        "a2a-lab-ot2-indexing-priority-{name}-{}",
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

fn fresh_capture(offset_us: i64) -> a2a_lab_dev_kit::UtcTimestamp {
    a2a_lab_ot2::time::now()
        .unwrap()
        .saturating_add_micros(offset_us)
}

fn semantic_config() -> a2a_lab_dev_kit::SemanticIndexingConfig {
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

async fn seed_backfill_jobs(store: &IndexedVideoDvr, count: usize) {
    let source = VideoSourceId::new("camera").unwrap();
    for index in 0..count {
        store
            .append_mjpeg_frame(
                &source,
                fresh_capture(i64::try_from(index).unwrap_or(i64::MAX) * 1_000_000),
                1_000_000,
                &jpeg(u8::try_from(index).unwrap_or(u8::MAX)),
            )
            .await
            .unwrap();
    }
    sqlx::query("DELETE FROM semantic_indexing_job_runtime")
        .execute(store.semantic_pool())
        .await
        .unwrap();
    sqlx::query("DELETE FROM semantic_indexing_jobs")
        .execute(store.semantic_pool())
        .await
        .unwrap();
    sqlx::query("DELETE FROM semantic_frame_revisions")
        .execute(store.semantic_pool())
        .await
        .unwrap();
    let report = store
        .reconcile_retained_indexing_until_idle(&store_ns())
        .await
        .unwrap();
    assert_eq!(report.jobs_created(), u32::try_from(count).unwrap());
}

#[tokio::test]
async fn reconcile_discovers_frames_and_legacy_segments() {
    let (_dir, store) = open_store("all-history").await;
    let source = VideoSourceId::new("camera").unwrap();
    let track = DvrTrackId::new("h264").unwrap();
    store
        .write_legacy_segment(&source, &track, fresh_capture(0), 1_000_000, true, &jpeg(1))
        .await
        .unwrap();
    store
        .append_mjpeg_frame(&source, fresh_capture(5_000_000), 1_000_000, &jpeg(2))
        .await
        .unwrap();
    sqlx::query("DELETE FROM semantic_indexing_job_runtime")
        .execute(store.semantic_pool())
        .await
        .unwrap();
    sqlx::query("DELETE FROM semantic_indexing_jobs")
        .execute(store.semantic_pool())
        .await
        .unwrap();
    sqlx::query("DELETE FROM semantic_frame_revisions")
        .execute(store.semantic_pool())
        .await
        .unwrap();
    let report = store
        .reconcile_retained_indexing_until_idle(&store_ns())
        .await
        .unwrap();
    assert_eq!(report.jobs_created(), 2);
    assert_eq!(store.count_indexing_jobs().await.unwrap(), 2);
}

#[tokio::test]
async fn live_jobs_claim_before_backfill() {
    let (_dir, store) = open_store("claim-live").await;
    seed_backfill_jobs(&store, 3).await;
    let source = VideoSourceId::new("camera").unwrap();
    let capture = fresh_capture(10_000_000);
    store
        .append_mjpeg_frame(&source, capture, 1_000_000, &jpeg(9))
        .await
        .unwrap();
    let track = MemoryVideos::mjpeg_track_id(&source).unwrap();
    let (frame, _, _) = store
        .ensure_indexing_at_dvr_key(&store_ns(), &source, &track, capture)
        .await
        .unwrap();
    let live_job = indexing_job_id(frame.frame_id().as_str());
    assert_eq!(
        store
            .indexing_job_enqueue_priority(&live_job)
            .await
            .unwrap(),
        INDEXING_PRIORITY_LIVE
    );
    let now_us = a2a_lab_ot2::time::now().unwrap().as_micros();
    let claimed = store
        .claim_indexing_jobs("w1", 1, now_us + 60_000_000, now_us)
        .await
        .unwrap();
    assert_eq!(claimed.len(), 1);
    assert_eq!(claimed[0].job_id(), live_job);
}

#[tokio::test]
async fn claim_batch_reserves_backfill_slot_while_prioritizing_live() {
    let (_dir, store) = open_store("claim-fair").await;
    seed_backfill_jobs(&store, 4).await;
    let source = VideoSourceId::new("camera").unwrap();
    store
        .append_mjpeg_frame(&source, fresh_capture(20_000_000), 1_000_000, &jpeg(8))
        .await
        .unwrap();
    let now_us = a2a_lab_ot2::time::now().unwrap().as_micros();
    let claimed = store
        .claim_indexing_jobs("w1", 2, now_us + 60_000_000, now_us)
        .await
        .unwrap();
    assert_eq!(claimed.len(), 2);
    let priorities: Vec<i32> = futures_util::future::join_all(
        claimed
            .iter()
            .map(|job| store.indexing_job_enqueue_priority(job.job_id())),
    )
    .await
    .into_iter()
    .map(Result::unwrap)
    .collect();
    assert!(priorities.contains(&INDEXING_PRIORITY_LIVE));
    assert!(priorities.contains(&INDEXING_PRIORITY_BACKFILL));
}

#[tokio::test]
async fn capture_commit_never_blocks_on_semantic_backlog() {
    let (_dir, store) = open_store("no-backpressure").await;
    seed_backfill_jobs(&store, 6).await;
    let source = VideoSourceId::new("camera").unwrap();
    let started = std::time::Instant::now();
    for tag in 10..13u8 {
        store
            .append_mjpeg_frame(
                &source,
                fresh_capture(i64::from(tag) * 2_000_000),
                1_000_000,
                &jpeg(tag),
            )
            .await
            .unwrap();
    }
    assert!(
        started.elapsed() < std::time::Duration::from_secs(2),
        "capture commits waited on semantic queue"
    );
    assert_eq!(store.count_indexing_jobs().await.unwrap(), 9);
}

#[tokio::test]
async fn worker_makes_concurrent_live_and_backfill_progress() {
    let (_dir, store) = open_store("concurrent").await;
    seed_backfill_jobs(&store, 2).await;
    let source = VideoSourceId::new("camera").unwrap();
    store
        .append_mjpeg_frame(&source, fresh_capture(30_000_000), 1_000_000, &jpeg(7))
        .await
        .unwrap();
    let semantic = semantic_config();
    let live_worker = DeterministicIndexingWorker::deterministic_lane(
        &semantic,
        "w-priority-live",
        IndexingWorkerFaults::new(),
        IndexingWorkerLane::Live,
    )
    .await
    .unwrap();
    let backfill_worker = DeterministicIndexingWorker::deterministic_lane(
        &semantic,
        "w-priority-backfill",
        IndexingWorkerFaults::new(),
        IndexingWorkerLane::Backfill,
    )
    .await
    .unwrap();
    let (live_report, backfill_report) = tokio::join!(
        live_worker.run_batch(&store, &semantic),
        backfill_worker.run_batch(&store, &semantic),
    );
    live_report.unwrap();
    backfill_report.unwrap();
    let ready: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM semantic_indexing_job_runtime WHERE job_state = 'ready'",
    )
    .fetch_one(store.semantic_pool())
    .await
    .unwrap();
    assert!(
        ready >= 2,
        "expected live and backfill jobs to advance together"
    );
}

#[tokio::test]
async fn restart_preserves_priority_and_resumes_claims() {
    let dir = temp_root("restart-priority");
    let _ = std::fs::remove_dir_all(&dir);
    let live_job;
    {
        let store = IndexedVideoDvr::open(&dir, 86_400).await.unwrap();
        seed_backfill_jobs(&store, 1).await;
        let source = VideoSourceId::new("camera").unwrap();
        let capture = fresh_capture(40_000_000);
        store
            .append_mjpeg_frame(&source, capture, 1_000_000, &jpeg(6))
            .await
            .unwrap();
        let job_id: String = sqlx::query_scalar(
            "SELECT job_id FROM semantic_indexing_jobs j
             JOIN semantic_frame_revisions r ON r.frame_id = j.frame_id
             WHERE r.source_id = ?1 AND r.start_us = ?2 AND j.enqueue_priority = ?3",
        )
        .bind(source.as_str())
        .bind(capture.as_micros())
        .bind(i64::from(INDEXING_PRIORITY_LIVE))
        .fetch_one(store.semantic_pool())
        .await
        .unwrap();
        live_job = job_id;
        assert_eq!(
            store
                .indexing_job_enqueue_priority(&live_job)
                .await
                .unwrap(),
            INDEXING_PRIORITY_LIVE
        );
    }
    let store = IndexedVideoDvr::open(&dir, 86_400).await.unwrap();
    assert_eq!(
        store
            .indexing_job_enqueue_priority(&live_job)
            .await
            .unwrap(),
        INDEXING_PRIORITY_LIVE
    );
    let now_us = a2a_lab_ot2::time::now().unwrap().as_micros();
    let claimed = store
        .claim_indexing_jobs("w1", 1, now_us + 60_000_000, now_us)
        .await
        .unwrap();
    assert_eq!(claimed[0].job_id(), live_job);
    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn expired_lease_on_backfill_job_reclaims_and_completes() {
    let (_dir, store) = open_store("lease-backfill").await;
    seed_backfill_jobs(&store, 1).await;
    let job_id: String = sqlx::query_scalar(
        "SELECT job_id FROM semantic_indexing_jobs WHERE enqueue_priority = ?1 LIMIT 1",
    )
    .bind(i64::from(INDEXING_PRIORITY_BACKFILL))
    .fetch_one(store.semantic_pool())
    .await
    .unwrap();
    let now_us = a2a_lab_ot2::time::now().unwrap().as_micros();
    let claimed = store
        .claim_indexing_jobs("stale", 1, now_us + 60_000_000, now_us)
        .await
        .unwrap();
    assert_eq!(claimed[0].job_id(), job_id);
    sqlx::query(
        "UPDATE semantic_indexing_job_runtime SET lease_expires_at_us = ?1 WHERE job_id = ?2",
    )
    .bind(now_us.saturating_sub(1))
    .bind(&job_id)
    .execute(store.semantic_pool())
    .await
    .unwrap();
    let resume = a2a_lab_ot2::time::now().unwrap().as_micros();
    assert!(store.reclaim_expired_indexing_leases(resume).await.unwrap() >= 1);
    let semantic = semantic_config();
    let worker = DeterministicIndexingWorker::deterministic_lane(
        &semantic,
        "w-lease",
        IndexingWorkerFaults::new(),
        IndexingWorkerLane::Backfill,
    )
    .await
    .unwrap();
    worker.run_batch(&store, &semantic).await.unwrap();
    assert_eq!(
        store.get_indexing_job(&job_id).await.unwrap().state(),
        IndexingJobState::Ready
    );
}

#[tokio::test]
async fn backfill_indexing_surfaces_searchable_candidates() {
    let (_dir, store) = open_store("search-hit").await;
    seed_backfill_jobs(&store, 1).await;
    let semantic = semantic_config();
    let worker = DeterministicIndexingWorker::deterministic_lane(
        &semantic,
        "w-search",
        IndexingWorkerFaults::new(),
        IndexingWorkerLane::Backfill,
    )
    .await
    .unwrap();
    worker.run_batch(&store, &semantic).await.unwrap();
    let embeddings = semantic.embeddings.as_ref().unwrap();
    let model_space = embeddings.model_space_identity().unwrap();
    let filter = ManifestSearchCandidateFilter {
        description_generation: ProvenanceGeneration::new(1).unwrap().get(),
        model_space,
    };
    let source = VideoSourceId::new("camera").unwrap();
    let range = TimeRange::new(fresh_capture(-10_000_000), fresh_capture(10_000_000)).unwrap();
    let scan = store
        .scan_semantic_search_candidates(
            source.as_str(),
            range.start().as_micros(),
            range.end().as_micros(),
            &filter,
            5,
        )
        .await
        .unwrap();
    assert!(!scan.records.is_empty());
    assert_eq!(
        scan.records[0].embedding_checkpoint(),
        IndexingCheckpoint::Ready
    );
}

fn alternate_embedding_config() -> SemanticEmbeddingsConfig {
    const BASE: &str = concat!(include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../a2a-lab-dev-kit-rs/tests/fixtures/a2a-lab-v2-semantic-valid.json"
    )),);
    let json = BASE.replace("operator-text-embed-v1", "operator-text-embed-v2");
    A2aLabConfig::parse(&json)
        .unwrap()
        .semantic_indexing
        .unwrap()
        .embeddings
        .unwrap()
}

fn alternate_semantic_config() -> a2a_lab_dev_kit::SemanticIndexingConfig {
    let mut config = semantic_config();
    config.embeddings = Some(alternate_embedding_config());
    config
}

#[tokio::test]
async fn atomic_model_space_cutover_wipes_vectors_before_requeue() {
    let (_dir, store) = open_store("atomic-cutover").await;
    seed_backfill_jobs(&store, 2).await;
    let semantic = semantic_config();
    let worker = DeterministicIndexingWorker::deterministic_lane(
        &semantic,
        "w-atomic",
        IndexingWorkerFaults::new(),
        IndexingWorkerLane::Backfill,
    )
    .await
    .unwrap();
    worker.run_batch(&store, &semantic).await.unwrap();
    let embeddings = semantic.embeddings.as_ref().unwrap();
    store.sync_embedding_config(embeddings).await.unwrap();
    assert!(store.count_nonempty_embedding_vectors().await.unwrap() > 0);
    assert!(store.count_searchable_embedding_vectors().await.unwrap() > 0);
    let provenance_rows: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM semantic_frame_embeddings")
        .fetch_one(store.semantic_pool())
        .await
        .unwrap();
    assert_eq!(provenance_rows, 2);
    store
        .sync_embedding_config(&alternate_embedding_config())
        .await
        .unwrap();
    assert_eq!(store.count_nonempty_embedding_vectors().await.unwrap(), 0);
    assert_eq!(store.count_searchable_embedding_vectors().await.unwrap(), 0);
    let ready_embedding: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM semantic_frame_revisions
         WHERE embedding_checkpoint = 'ready' AND search_current = 1",
    )
    .fetch_one(store.semantic_pool())
    .await
    .unwrap();
    assert_eq!(ready_embedding, 0);
    let ready_jobs: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM semantic_indexing_job_runtime WHERE job_state = 'ready'",
    )
    .fetch_one(store.semantic_pool())
    .await
    .unwrap();
    assert_eq!(ready_jobs, 0);
    let provenance_after: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM semantic_frame_embeddings")
            .fetch_one(store.semantic_pool())
            .await
            .unwrap();
    assert_eq!(provenance_after, provenance_rows);
    let rebuilt = alternate_semantic_config();
    store
        .sync_embedding_config(rebuilt.embeddings.as_ref().unwrap())
        .await
        .unwrap();
    let reclaimed = DeterministicIndexingWorker::deterministic_lane(
        &rebuilt,
        "w-atomic-v2",
        IndexingWorkerFaults::new(),
        IndexingWorkerLane::Backfill,
    )
    .await
    .unwrap();
    reclaimed.run_batch(&store, &rebuilt).await.unwrap();
    assert!(store.count_nonempty_embedding_vectors().await.unwrap() > 0);
    assert!(store.count_searchable_embedding_vectors().await.unwrap() > 0);
}

#[tokio::test]
async fn model_space_rebuild_requeues_all_retained_frames_for_reembedding() {
    let (_dir, store) = open_store("rebuild").await;
    seed_backfill_jobs(&store, 2).await;
    let semantic = semantic_config();
    let worker = DeterministicIndexingWorker::deterministic_lane(
        &semantic,
        "w-rebuild",
        IndexingWorkerFaults::new(),
        IndexingWorkerLane::Backfill,
    )
    .await
    .unwrap();
    worker.run_batch(&store, &semantic).await.unwrap();
    let ready_before: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM semantic_indexing_job_runtime WHERE job_state = 'ready'",
    )
    .fetch_one(store.semantic_pool())
    .await
    .unwrap();
    assert_eq!(ready_before, 2);
    let embeddings = semantic.embeddings.as_ref().unwrap();
    store.sync_embedding_config(embeddings).await.unwrap();
    let changed = store
        .sync_embedding_config(&alternate_embedding_config())
        .await
        .unwrap();
    assert!(changed);
    let ready_after: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM semantic_indexing_job_runtime WHERE job_state = 'ready'",
    )
    .fetch_one(store.semantic_pool())
    .await
    .unwrap();
    assert_eq!(ready_after, 0);
    let pending_embedding: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM semantic_frame_revisions
         WHERE embedding_checkpoint = 'pending' AND search_current = 1",
    )
    .fetch_one(store.semantic_pool())
    .await
    .unwrap();
    assert_eq!(pending_embedding, 2);
    let null_vectors: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM semantic_frame_embeddings WHERE embedding_vector IS NULL",
    )
    .fetch_one(store.semantic_pool())
    .await
    .unwrap();
    assert_eq!(null_vectors, 2);
}

#[tokio::test]
async fn live_frames_still_claim_first_after_model_space_rebuild() {
    let (_dir, store) = open_store("rebuild-priority").await;
    seed_backfill_jobs(&store, 2).await;
    let semantic = semantic_config();
    let embeddings = semantic.embeddings.as_ref().unwrap();
    store.sync_embedding_config(embeddings).await.unwrap();
    store
        .sync_embedding_config(&alternate_embedding_config())
        .await
        .unwrap();
    let source = VideoSourceId::new("camera").unwrap();
    store
        .append_mjpeg_frame(&source, fresh_capture(50_000_000), 1_000_000, &jpeg(4))
        .await
        .unwrap();
    let now_us = a2a_lab_ot2::time::now().unwrap().as_micros();
    let claimed = store
        .claim_indexing_jobs("w-live", 1, now_us + 60_000_000, now_us)
        .await
        .unwrap();
    assert_eq!(claimed.len(), 1);
    assert_eq!(
        store
            .indexing_job_enqueue_priority(claimed[0].job_id())
            .await
            .unwrap(),
        INDEXING_PRIORITY_LIVE
    );
}

#[tokio::test]
async fn stale_vectors_excluded_until_new_model_space_drains() {
    let (_dir, store) = open_store("stale-search").await;
    seed_backfill_jobs(&store, 1).await;
    let semantic = semantic_config();
    let worker = DeterministicIndexingWorker::deterministic_lane(
        &semantic,
        "w-stale",
        IndexingWorkerFaults::new(),
        IndexingWorkerLane::Backfill,
    )
    .await
    .unwrap();
    worker.run_batch(&store, &semantic).await.unwrap();
    let embeddings = semantic.embeddings.as_ref().unwrap();
    let model_space = embeddings.model_space_identity().unwrap();
    let mut stale_filter = ManifestSearchCandidateFilter {
        description_generation: ProvenanceGeneration::new(1).unwrap().get(),
        model_space: model_space.clone(),
    };
    let source = VideoSourceId::new("camera").unwrap();
    let range = TimeRange::new(fresh_capture(-10_000_000), fresh_capture(10_000_000)).unwrap();
    let scan = store
        .scan_semantic_search_candidates(
            source.as_str(),
            range.start().as_micros(),
            range.end().as_micros(),
            &stale_filter,
            5,
        )
        .await
        .unwrap();
    assert_eq!(scan.records.len(), 1);
    store.sync_embedding_config(embeddings).await.unwrap();
    store
        .sync_embedding_config(&alternate_embedding_config())
        .await
        .unwrap();
    let empty = store
        .scan_semantic_search_candidates(
            source.as_str(),
            range.start().as_micros(),
            range.end().as_micros(),
            &stale_filter,
            5,
        )
        .await
        .unwrap();
    assert!(empty.records.is_empty());
    let rebuilt = alternate_semantic_config();
    let rebuilt_embeddings = rebuilt.embeddings.as_ref().unwrap();
    store
        .sync_embedding_config(rebuilt_embeddings)
        .await
        .unwrap();
    stale_filter.model_space = rebuilt_embeddings.model_space_identity().unwrap();
    let rebuilt_worker = DeterministicIndexingWorker::deterministic_lane(
        &rebuilt,
        "w-stale-v2",
        IndexingWorkerFaults::new(),
        IndexingWorkerLane::Backfill,
    )
    .await
    .unwrap();
    rebuilt_worker.run_batch(&store, &rebuilt).await.unwrap();
    let refreshed = store
        .scan_semantic_search_candidates(
            source.as_str(),
            range.start().as_micros(),
            range.end().as_micros(),
            &stale_filter,
            5,
        )
        .await
        .unwrap();
    assert_eq!(refreshed.records.len(), 1);
}

async fn seed_live_jobs(store: &IndexedVideoDvr, count: usize) {
    let source = VideoSourceId::new("camera").unwrap();
    for index in 0..count {
        store
            .append_mjpeg_frame(
                &source,
                fresh_capture(i64::try_from(10_000 + index).unwrap_or(i64::MAX) * 2_000_000),
                1_000_000,
                &jpeg(u8::try_from(200 + index).unwrap_or(u8::MAX)),
            )
            .await
            .unwrap();
    }
}

async fn count_ready_jobs(store: &IndexedVideoDvr) -> u64 {
    let count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM semantic_indexing_job_runtime WHERE job_state = 'ready'",
    )
    .fetch_one(store.semantic_pool())
    .await
    .unwrap();
    u64::try_from(count.max(0)).unwrap_or(0)
}

#[tokio::test]
async fn dedicated_lane_claims_only_matching_priority() {
    let (_dir, store) = open_store("dedicated-lanes").await;
    seed_backfill_jobs(&store, 6).await;
    seed_live_jobs(&store, 4).await;
    let now_us = a2a_lab_ot2::time::now().unwrap().as_micros();
    let deadline = now_us + 60_000_000;
    let live_claimed = store
        .claim_indexing_jobs_for_lane("live-only", 8, deadline, now_us, IndexingWorkerLane::Live)
        .await
        .unwrap();
    assert!(!live_claimed.is_empty());
    for job in &live_claimed {
        assert_eq!(
            store
                .indexing_job_enqueue_priority(job.job_id())
                .await
                .unwrap(),
            INDEXING_PRIORITY_LIVE
        );
    }
    let backfill_claimed = store
        .claim_indexing_jobs_for_lane(
            "backfill-only",
            8,
            deadline,
            now_us,
            IndexingWorkerLane::Backfill,
        )
        .await
        .unwrap();
    assert!(!backfill_claimed.is_empty());
    for job in &backfill_claimed {
        assert_eq!(
            store
                .indexing_job_enqueue_priority(job.job_id())
                .await
                .unwrap(),
            INDEXING_PRIORITY_BACKFILL
        );
    }
    let live_ids: HashSet<_> = live_claimed.iter().map(IndexingJobRecord::job_id).collect();
    let backfill_ids: HashSet<_> = backfill_claimed
        .iter()
        .map(IndexingJobRecord::job_id)
        .collect();
    assert!(live_ids.is_disjoint(&backfill_ids));
}

#[tokio::test]
async fn dedicated_live_and_backfill_workers_run_concurrent_batches() {
    let (_dir, store) = open_store("dedicated-workers").await;
    seed_backfill_jobs(&store, 8).await;
    seed_live_jobs(&store, 4).await;
    let semantic = semantic_config();
    let live_worker = DeterministicIndexingWorker::deterministic_lane(
        &semantic,
        "lane-live",
        IndexingWorkerFaults::new(),
        IndexingWorkerLane::Live,
    )
    .await
    .unwrap();
    let backfill_worker = DeterministicIndexingWorker::deterministic_lane(
        &semantic,
        "lane-backfill",
        IndexingWorkerFaults::new(),
        IndexingWorkerLane::Backfill,
    )
    .await
    .unwrap();
    let before = count_ready_jobs(&store).await;
    let (live_report, backfill_report) = tokio::join!(
        live_worker.run_batch(&store, &semantic),
        backfill_worker.run_batch(&store, &semantic),
    );
    let live_report = live_report.unwrap();
    let backfill_report = backfill_report.unwrap();
    assert_eq!(live_report.lane(), IndexingWorkerLane::Live);
    assert_eq!(backfill_report.lane(), IndexingWorkerLane::Backfill);
    assert!(live_report.claimed() > 0);
    assert!(backfill_report.claimed() > 0);
    assert!(count_ready_jobs(&store).await > before);
}

#[test]
fn claim_batch_scales_with_concurrency() {
    let mut semantic = semantic_config();
    semantic.limits.worker.max_batch_items = 2;
    semantic.limits.worker.max_concurrency = 4;
    assert_eq!(indexing_claim_batch_size(&semantic.limits.worker), 8);
}

#[tokio::test]
async fn claim_prefers_live_over_backfill_under_load() {
    let (_dir, store) = open_store("claim-priority-load").await;
    seed_backfill_jobs(&store, 12).await;
    seed_live_jobs(&store, 8).await;
    let mut semantic = semantic_config();
    semantic.limits.worker.max_concurrency = 4;
    semantic.limits.worker.max_batch_items = 4;
    let claim_batch = indexing_claim_batch_size(&semantic.limits.worker);
    let now_us = a2a_lab_ot2::time::now().unwrap().as_micros();
    let claimed = store
        .claim_indexing_jobs("priority-test", claim_batch, now_us + 60_000_000, now_us)
        .await
        .unwrap();
    assert_eq!(claimed.len(), usize::try_from(claim_batch).unwrap());
    let mut live = 0u32;
    let mut backfill = 0u32;
    for job in &claimed {
        let priority = store
            .indexing_job_enqueue_priority(job.job_id())
            .await
            .unwrap();
        if priority == INDEXING_PRIORITY_LIVE {
            live = live.saturating_add(1);
        } else {
            backfill = backfill.saturating_add(1);
        }
    }
    assert!(live >= semantic.limits.worker.max_batch_items.saturating_sub(1));
    assert!(backfill >= 1);
    assert!(live >= backfill);
}

#[tokio::test]
async fn concurrent_claims_receive_disjoint_job_leases() {
    let (_dir, store) = open_store("concurrent-claim").await;
    seed_backfill_jobs(&store, 24).await;
    let semantic = semantic_config();
    let per_worker_claim = semantic.limits.worker.max_batch_items;
    let now_us = a2a_lab_ot2::time::now().unwrap().as_micros();
    let deadline = now_us + 60_000_000;
    let (claimed_a, claimed_b) = tokio::join!(
        store.claim_indexing_jobs("worker-a", per_worker_claim, deadline, now_us),
        store.claim_indexing_jobs("worker-b", per_worker_claim, deadline, now_us),
    );
    let claimed_a = claimed_a.unwrap();
    let claimed_b = claimed_b.unwrap();
    let ids_a: HashSet<_> = claimed_a.iter().map(IndexingJobRecord::job_id).collect();
    let ids_b: HashSet<_> = claimed_b.iter().map(IndexingJobRecord::job_id).collect();
    let all_ids: HashSet<_> = ids_a.union(&ids_b).copied().collect();
    assert!(!all_ids.is_empty());
    assert_eq!(all_ids.len(), ids_a.len() + ids_b.len());
    for job in &claimed_a {
        store
            .release_indexing_job(job.job_id(), "worker-a")
            .await
            .unwrap();
    }
    for job in &claimed_b {
        store
            .release_indexing_job(job.job_id(), "worker-b")
            .await
            .unwrap();
    }
    let _ = semantic;
}

#[tokio::test]
async fn parallel_worker_batches_make_progress_under_load() {
    let (_dir, store) = open_store("parallel-progress").await;
    seed_backfill_jobs(&store, 32).await;
    let semantic = semantic_config();
    let worker_a = DeterministicIndexingWorker::deterministic_lane(
        &semantic,
        "worker-a",
        IndexingWorkerFaults::new(),
        IndexingWorkerLane::Live,
    )
    .await
    .unwrap();
    let worker_b = DeterministicIndexingWorker::deterministic_lane(
        &semantic,
        "worker-b",
        IndexingWorkerFaults::new(),
        IndexingWorkerLane::Backfill,
    )
    .await
    .unwrap();
    let before = count_ready_jobs(&store).await;
    let (report_a, report_b) = tokio::join!(
        worker_a.run_batch(&store, &semantic),
        worker_b.run_batch(&store, &semantic),
    );
    let report_a = report_a.unwrap();
    let report_b = report_b.unwrap();
    let progressed = u64::from(report_a.claimed()).saturating_add(u64::from(report_b.claimed()));
    assert!(progressed > 0);
    assert!(count_ready_jobs(&store).await > before);
    let leased: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM semantic_indexing_job_runtime WHERE job_state = 'leased'",
    )
    .fetch_one(store.semantic_pool())
    .await
    .unwrap();
    assert_eq!(leased, 0);
}

const DRAIN_JOB_COUNT: usize = 24;

#[tokio::test]
async fn worker_drain_increments_ready_under_backfill_load() {
    let (_dir, store) = open_store("drain-throughput").await;
    seed_backfill_jobs(&store, DRAIN_JOB_COUNT).await;
    let semantic = semantic_config();
    let worker = DeterministicIndexingWorker::deterministic_lane(
        &semantic,
        "drain-worker",
        IndexingWorkerFaults::new(),
        IndexingWorkerLane::Backfill,
    )
    .await
    .unwrap();
    let mut passes = 0u32;
    while count_ready_jobs(&store).await < u64::try_from(DRAIN_JOB_COUNT).unwrap() && passes < 16 {
        let report = worker.run_batch(&store, &semantic).await.unwrap();
        assert!(report.claimed() > 0 || count_ready_jobs(&store).await > 0);
        passes = passes.saturating_add(1);
    }
    assert_eq!(
        count_ready_jobs(&store).await,
        u64::try_from(DRAIN_JOB_COUNT).unwrap()
    );
    let pending: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM semantic_indexing_job_runtime WHERE job_state = 'pending'",
    )
    .fetch_one(store.semantic_pool())
    .await
    .unwrap();
    assert_eq!(pending, 0);
}
