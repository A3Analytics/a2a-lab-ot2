//! Durable indexing job leases, retries, and unified annotation worker.

use std::sync::Arc;

use a2a_lab_dev_kit::{
    A2aLabConfig, DecisionEvaluationCoverage, DecisionEvidenceItem, DecisionIdempotencyKey,
    DecisionOutcome, DecisionSubject, DvrStoreNamespace, FrameDecisionRecord, FrameRoiTransform,
    JevEvaluationId, JevEvaluationProvenance, JevProducerIdentity, JevStructuredOutput,
    LabPredicate, MemoryVideos, PipetteChannelSubject, SavedFrameRef, SemanticIndexingConfig,
    Sha256Digest, VideoSourceId,
};
use a2a_lab_ot2::IndexedVideoDvr;
use a2a_lab_ot2::videos::dvr::{
    DEFAULT_SEMANTIC_STORE_NAMESPACE, DeterministicIndexingWorker,
    IndexingCheckpoint, IndexingJobState, IndexingWorkerFaults, indexing_job_id,
};

const SEMANTIC_LIMITS: &str = r#""limits":{"worker":{"max_batch_items":4,"max_concurrency":2,"max_queue_backlog":64,"max_retry_attempts":3,"job_deadline":"30s","retry_backoff":"1s"},"search":{"top_k":10,"candidate_limit":1000,"max_neighbor_frames":5,"query_window":"2h"},"roi":{"min_edge_pixels":280,"max_edge_pixels":1568,"max_rois_per_frame":8},"storage":{"vector_index_path":"./state/semantic-index","max_disk_bytes":1073741824},"decisions":{"max_batch_items":8,"max_concurrency":2,"job_deadline":"45s","coverage_policy":"on_demand"}}"#;

fn semantic_config() -> SemanticIndexingConfig {
    let document = format!(
        r#"{{"schema_version":2,"semantic_indexing":{{"description":{{"provider":"bedrock"}},"embeddings":{{"provider":"bedrock","model":"text-embed-v1","space":{{"dimensions":4,"normalization":"l2_unit","metric":"cosine","generation":1}}}},{SEMANTIC_LIMITS},"decisions":{{"provider":"jev","model":"jev-1.14.0"}}}}}}"#
    );
    A2aLabConfig::parse(&document)
        .unwrap()
        .semantic_indexing
        .unwrap()
}

fn temp_root(name: &str) -> std::path::PathBuf {
    std::env::temp_dir().join(format!(
        "a2a-lab-ot2-indexing-worker-{name}-{}",
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

async fn seed_job(store: &IndexedVideoDvr) -> (String, String) {
    let source = VideoSourceId::new("camera").unwrap();
    let capture = a2a_lab_ot2::time::now().unwrap();
    store
        .append_mjpeg_frame(&source, capture, 1_000_000, &jpeg(1))
        .await
        .unwrap();
    let track = MemoryVideos::mjpeg_track_id(&source).unwrap();
    let (frame, _, _) = store
        .ensure_indexing_at_dvr_key(&store_ns(), &source, &track, capture)
        .await
        .unwrap();
    let job_id = indexing_job_id(frame.frame_id().as_str());
    (job_id, frame.frame_id().as_str().to_owned())
}

async fn worker_for(
    semantic: &SemanticIndexingConfig,
    faults: Arc<IndexingWorkerFaults>,
) -> DeterministicIndexingWorker {
    DeterministicIndexingWorker::deterministic(semantic, "test-worker", faults)
        .await
        .unwrap()
}

#[tokio::test]
async fn job_states_progress_through_ready_with_attempt_metadata() {
    let (_dir, store) = open_store("states").await;
    let (job_id, _) = seed_job(&store).await;
    let semantic = semantic_config();
    let worker = worker_for(&semantic, IndexingWorkerFaults::new()).await;
    let pending = store.get_indexing_job(&job_id).await.unwrap();
    assert_eq!(pending.state(), IndexingJobState::Pending);
    let report = worker.run_batch(&store, &semantic).await.unwrap();
    assert_eq!(report.descriptions_committed(), 1);
    assert_eq!(report.embeddings_committed(), 1);
    let ready = store.get_indexing_job(&job_id).await.unwrap();
    assert_eq!(ready.state(), IndexingJobState::Ready);
    assert!(ready.description_attempts() >= 1);
    assert!(ready.embedding_attempts() >= 1);
    let manifest = store.get_manifest_frame(ready.frame_id()).await.unwrap();
    assert_eq!(manifest.description_checkpoint(), IndexingCheckpoint::Ready);
    assert_eq!(manifest.embedding_checkpoint(), IndexingCheckpoint::Ready);
}

#[tokio::test]
async fn expired_lease_resumes_after_reclaim() {
    let (_dir, store) = open_store("lease").await;
    let (job_id, _) = seed_job(&store).await;
    let semantic = semantic_config();
    let now_us = a2a_lab_ot2::time::now().unwrap().as_micros();
    let claimed = store
        .claim_indexing_jobs("stale", 1, now_us + 60_000_000, now_us)
        .await
        .unwrap();
    assert_eq!(claimed.len(), 1);
    assert_eq!(claimed[0].job_id(), job_id);
    let expired_at = now_us.saturating_sub(1);
    let updated = sqlx::query(
        "UPDATE semantic_indexing_job_runtime SET lease_expires_at_us = ?1 WHERE job_id = ?2",
    )
    .bind(expired_at)
    .bind(&job_id)
    .execute(store.sqlite_pool())
    .await
    .unwrap();
    assert_eq!(updated.rows_affected(), 1);
    let resume_now = a2a_lab_ot2::time::now().unwrap().as_micros();
    let reclaimed = store
        .reclaim_expired_indexing_leases(resume_now)
        .await
        .unwrap();
    assert!(reclaimed >= 1);
    assert_ne!(
        store.get_indexing_job(&job_id).await.unwrap().state(),
        IndexingJobState::Leased
    );
    let worker = worker_for(&semantic, IndexingWorkerFaults::new()).await;
    worker.run_batch(&store, &semantic).await.unwrap();
    assert_eq!(
        store.get_indexing_job(&job_id).await.unwrap().state(),
        IndexingJobState::Ready
    );
}

#[tokio::test]
async fn stale_worker_cannot_complete_newer_lease() {
    let (_dir, store) = open_store("cas").await;
    let (job_id, frame_id) = seed_job(&store).await;
    let now_us = a2a_lab_ot2::time::now().unwrap().as_micros();
    let claimed = store
        .claim_indexing_jobs("owner-a", 1, now_us + 60_000_000, now_us)
        .await
        .unwrap();
    let job = &claimed[0];
    assert!(
        !store
            .complete_indexing_description_cas(
                &job_id,
                "owner-b",
                job.revision_generation(),
                1,
                now_us
            )
            .await
            .unwrap()
    );
    sqlx::query(
        "UPDATE semantic_frame_revisions SET revision_seq = revision_seq + 1 WHERE frame_id = ?1",
    )
    .bind(&frame_id)
    .execute(store.sqlite_pool())
    .await
    .unwrap();
    assert!(
        !store
            .complete_indexing_description_cas(
                &job_id,
                "owner-a",
                job.revision_generation(),
                1,
                now_us
            )
            .await
            .unwrap()
    );
}

#[tokio::test]
async fn persisted_description_survives_embedding_outage() {
    let (_dir, store) = open_store("embed-retry").await;
    let (job_id, frame_id) = seed_job(&store).await;
    let semantic = semantic_config();
    let faults = IndexingWorkerFaults::new();
    faults.set_force_embedding_failure(true);
    let worker = worker_for(&semantic, Arc::clone(&faults)).await;
    worker.run_batch(&store, &semantic).await.unwrap();
    let failed = store.get_indexing_job(&job_id).await.unwrap();
    assert_eq!(failed.state(), IndexingJobState::RetryableFailed);
    let manifest = store.get_manifest_frame(&frame_id).await.unwrap();
    assert!(manifest.description().is_some());
    assert_eq!(manifest.embedding_checkpoint(), IndexingCheckpoint::Pending);
    faults.set_force_embedding_failure(false);
    sqlx::query("UPDATE semantic_indexing_job_runtime SET next_retry_at_us = 0 WHERE job_id = ?1")
        .bind(&job_id)
        .execute(store.sqlite_pool())
        .await
        .unwrap();
    worker.run_batch(&store, &semantic).await.unwrap();
    assert_eq!(
        store.get_indexing_job(&job_id).await.unwrap().state(),
        IndexingJobState::Ready
    );
}

#[tokio::test]
async fn crash_before_description_commit_preserves_coverage() {
    let (_dir, store) = open_store("crash-before").await;
    let (job_id, frame_id) = seed_job(&store).await;
    let semantic = semantic_config();
    let faults = IndexingWorkerFaults::new();
    faults.set_crash_before_description_commit(true);
    let worker = worker_for(&semantic, Arc::clone(&faults)).await;
    let report = worker.run_batch(&store, &semantic).await.unwrap();
    assert_eq!(report.failures(), 1);
    let manifest = store.get_manifest_frame(&frame_id).await.unwrap();
    assert!(manifest.description().is_none());
    let job = store.get_indexing_job(&job_id).await.unwrap();
    assert_ne!(job.state(), IndexingJobState::Ready);
}

#[tokio::test]
async fn crash_after_description_commit_reuses_text_idempotently() {
    let (_dir, store) = open_store("crash-after").await;
    let (job_id, frame_id) = seed_job(&store).await;
    let semantic = semantic_config();
    let faults = IndexingWorkerFaults::new();
    faults.set_crash_after_description_commit(true);
    let worker = worker_for(&semantic, Arc::clone(&faults)).await;
    let report = worker.run_batch(&store, &semantic).await.unwrap();
    assert_eq!(report.failures(), 1);
    let manifest = store.get_manifest_frame(&frame_id).await.unwrap();
    let text = manifest.description().unwrap().text().to_owned();
    faults.set_crash_after_description_commit(false);
    worker.run_batch(&store, &semantic).await.unwrap();
    let after = store.get_manifest_frame(&frame_id).await.unwrap();
    assert_eq!(after.description().unwrap().text(), text);
    assert_eq!(
        store.get_indexing_job(&job_id).await.unwrap().state(),
        IndexingJobState::Ready
    );
    let decisions: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM frame_decisions WHERE frame_id = ?1")
            .bind(&frame_id)
            .fetch_one(store.sqlite_pool())
            .await
            .unwrap();
    assert_eq!(decisions, 0);
}

fn tip_record(
    saved: &SavedFrameRef,
    outcome: DecisionOutcome,
    key: &str,
    eval_suffix: &str,
) -> FrameDecisionRecord {
    let evaluated_at = saved.captured_at().saturating_add_micros(1_000_000);
    let provenance = JevEvaluationProvenance::new(
        saved.captured_at(),
        evaluated_at,
        JevProducerIdentity::default_production(),
        JevEvaluationId::new(format!("jev-req-{eval_suffix}")).unwrap(),
        Sha256Digest::hash_bytes(b"config-v1"),
        Sha256Digest::hash_bytes(b"prompt-v1"),
        Sha256Digest::hash_bytes(b"criteria-v1"),
        Sha256Digest::hash_bytes(b"threshold-v1"),
        FrameRoiTransform::new([0.0, 0.0, 1.0, 1.0]).unwrap(),
        Sha256Digest::hash_bytes(b"jev-response-bytes"),
        DecisionEvaluationCoverage::Evaluated,
        Option::<&str>::None,
    )
    .unwrap();
    let subject = DecisionSubject::PipetteChannel(
        PipetteChannelSubject::new("left-pipetter", "channel-1", None).unwrap(),
    );
    let evidence = vec![
        DecisionEvidenceItem::saved_frame_revision(
            saved.frame_id().clone(),
            FrameRoiTransform::new([0.0, 0.0, 1.0, 1.0]).unwrap(),
        )
        .unwrap(),
    ];
    FrameDecisionRecord::new(
        DecisionIdempotencyKey::new(key).unwrap(),
        saved.clone(),
        LabPredicate::pipette_tip_attached_v1(),
        subject,
        evidence,
        outcome,
        JevStructuredOutput::empty(),
        provenance,
        None,
    )
    .unwrap()
}

#[tokio::test]
async fn jev_append_never_populates_luna_description_metadata() {
    let (_dir, store) = open_store("jev-only").await;
    let (job_id, frame_id) = seed_job(&store).await;
    let manifest = store.get_manifest_frame(&frame_id).await.unwrap();
    let record = tip_record(
        manifest.frame(),
        DecisionOutcome::Unknown,
        "jev-req-worker-jev:lab.pipette_tip_attached:left-1",
        "worker-jev",
    );
    store.append_frame_decision(record).await.unwrap();
    let after = store.get_manifest_frame(&frame_id).await.unwrap();
    assert!(after.description().is_none());
    assert_eq!(
        store.get_indexing_job(&job_id).await.unwrap().state(),
        IndexingJobState::Pending
    );
}

#[tokio::test]
async fn materialization_pauses_when_backlog_at_limit() {
    let (_dir, store) = open_store("backlog").await;
    for tag in 0..8u8 {
        let source = VideoSourceId::new("camera").unwrap();
        let capture = a2a_lab_ot2::time::now()
            .unwrap()
            .saturating_add_micros(i64::from(tag) * 1_000_000);
        store
            .append_mjpeg_frame(&source, capture, 1_000_000, &jpeg(tag))
            .await
            .unwrap();
    }
    let mut semantic = semantic_config();
    semantic.limits.worker.max_queue_backlog = 2;
    let worker = worker_for(&semantic, IndexingWorkerFaults::new()).await;
    let stats = store
        .indexing_worker_queue_stats(&semantic.limits.worker)
        .await
        .unwrap();
    assert!(stats.materialization_paused);
    store.set_materialization_paused(true).await.unwrap();
    worker.run_batch(&store, &semantic).await.unwrap();
    assert!(
        store.reconcile_cursor().await.unwrap().is_some()
            || store.count_indexing_jobs().await.unwrap() > 0
    );
}
