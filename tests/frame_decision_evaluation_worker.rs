//! Durable Jev-only evaluation jobs: leases, coverage policies, and isolation from Luna.

#![allow(clippy::implicit_clone, clippy::useless_vec)]

use std::io::Cursor;
use std::sync::Arc;

use image::ExtendedColorType;
use image::codecs::jpeg::JpegEncoder;

use a2a_lab_dev_kit::{
    A2aLabConfig, DecisionCoveragePolicy, DecisionEvaluationCoverage, DvrStoreNamespace,
    FakeJevBehavior, FrameDecisionEvaluatorLimits, LabPredicate, MemoryVideos,
    SemanticIndexingConfig, UtcTimestamp, VideoSourceId,
};
use a2a_lab_ot2::IndexedVideoDvr;
use a2a_lab_ot2::videos::dvr::{
    DecisionEvaluationFrameSelection, DecisionEvaluationTarget, DecisionEvaluationWorker,
    DecisionEvaluationWorkerFaults, DeterministicDecisionEvaluationWorker,
    EnqueueDecisionEvaluationsRequest, IndexingCheckpoint, IndexingJobState, IndexingWorkerFaults,
    decision_evaluation_job_id, pilot_evaluator_config, pilot_predicate_targets, subject_digest,
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
        "a2a-lab-ot2-decision-eval-{name}-{}",
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
    DvrStoreNamespace::new(a2a_lab_ot2::videos::dvr::DEFAULT_SEMANTIC_STORE_NAMESPACE).unwrap()
}

fn encode_jpeg(tag: u8) -> Vec<u8> {
    let width = 320u32;
    let height = 240u32;
    let mut pixels = Vec::new();
    for _ in 0..(width * height) {
        pixels.extend_from_slice(&[tag, 0, 0]);
    }
    let mut cursor = Cursor::new(Vec::new());
    JpegEncoder::new_with_quality(&mut cursor, 90)
        .encode(&pixels, width, height, ExtendedColorType::Rgb8)
        .unwrap();
    cursor.into_inner()
}

async fn seed_frame(store: &IndexedVideoDvr, tag: u8) -> String {
    let source = VideoSourceId::new("camera").unwrap();
    let capture = UtcTimestamp::parse("2024-06-01T12:00:00Z")
        .unwrap()
        .saturating_add_micros(i64::from(tag) * 100_000);
    store
        .append_mjpeg_frame(&source, capture, 1_000_000, &encode_jpeg(tag))
        .await
        .unwrap();
    let track = MemoryVideos::mjpeg_track_id(&source).unwrap();
    let (frame, _, _) = store
        .ensure_indexing_at_dvr_key(&store_ns(), &source, &track, capture)
        .await
        .unwrap();
    frame.frame_id().as_str().to_owned()
}

fn pilot_targets() -> Vec<DecisionEvaluationTarget> {
    pilot_predicate_targets()
        .into_iter()
        .enumerate()
        .map(|(index, (predicate, subject))| {
            DecisionEvaluationTarget::new(format!("pilot-{index}"), predicate, subject).unwrap()
        })
        .collect()
}

fn worker_ok(semantic: &SemanticIndexingConfig) -> DeterministicDecisionEvaluationWorker {
    let config = pilot_evaluator_config();
    let limits = FrameDecisionEvaluatorLimits::pilot();
    DecisionEvaluationWorker::deterministic(
        &config,
        limits,
        semantic.limits.decisions.clone(),
        FakeJevBehavior::OkPresent,
        "test-decision-worker",
        DecisionEvaluationWorkerFaults::new(),
    )
}

#[tokio::test]
async fn bounded_enqueue_is_idempotent_with_evaluator_identity() {
    let (_dir, store) = open_store("enqueue").await;
    let frame_id = seed_frame(&store, 1).await;
    let config = pilot_evaluator_config();
    let request =
        EnqueueDecisionEvaluationsRequest::on_demand_pilot_frames(config.clone(), vec![frame_id])
            .unwrap();
    let first = store
        .enqueue_decision_evaluation_jobs(&request)
        .await
        .unwrap();
    assert_eq!(first.jobs_created(), 3);
    let second = store
        .enqueue_decision_evaluation_jobs(&request)
        .await
        .unwrap();
    assert_eq!(second.idempotent(), 3);
    let subject = pilot_predicate_targets()[0].1.clone();
    let sample = store
        .get_decision_evaluation_job(&decision_evaluation_job_id(
            "jev-req-pilot-0",
            &LabPredicate::pipette_tip_attached_v1(),
            &subject_digest(&subject),
            config.config_digest().as_str(),
            config.producer().model_revision(),
        ))
        .await
        .unwrap();
    assert_eq!(sample.config_digest(), config.config_digest().as_str());
    assert_eq!(
        sample.producer_revision(),
        config.producer().model_revision()
    );
}

#[tokio::test]
async fn worker_evaluates_all_three_predicates_without_luna() {
    let (_dir, store) = open_store("predicates").await;
    let frame_id = seed_frame(&store, 2).await;
    let config = pilot_evaluator_config();
    let targets = pilot_targets();
    let request = EnqueueDecisionEvaluationsRequest::new(
        DecisionCoveragePolicy::OnDemand,
        config.clone(),
        DecisionEvaluationFrameSelection::FrameIds(vec![frame_id.clone()]),
        targets.clone().to_vec(),
        1,
    )
    .unwrap();
    store
        .enqueue_decision_evaluation_jobs(&request)
        .await
        .unwrap();
    let semantic = semantic_config();
    let faults = DecisionEvaluationWorkerFaults::new();
    let mut worker = DecisionEvaluationWorker::deterministic(
        &config,
        FrameDecisionEvaluatorLimits::pilot(),
        semantic.limits.decisions.clone(),
        FakeJevBehavior::OkPresent,
        "worker-3x",
        Arc::clone(&faults),
    );
    let report = worker.run_batch(&store, &semantic, &targets).await.unwrap();
    assert_eq!(report.failures(), 0, "worker failures");
    assert_eq!(report.claimed(), 3, "worker claimed jobs");
    assert_eq!(faults.luna_spy_calls(), 0);
    assert_eq!(faults.chat_spy_calls(), 0);
    let decisions: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM frame_decisions WHERE frame_id = ?1")
            .bind(&frame_id)
            .fetch_one(store.sqlite_pool())
            .await
            .unwrap();
    assert_eq!(decisions, 3);
    let manifest = store.get_manifest_frame(&frame_id).await.unwrap();
    assert_eq!(manifest.embedding_checkpoint(), IndexingCheckpoint::Pending);
    assert!(manifest.description().is_none());
    let indexing = store
        .get_indexing_job(&a2a_lab_ot2::videos::dvr::indexing_job_id(&frame_id))
        .await
        .unwrap();
    assert_eq!(indexing.state(), IndexingJobState::Pending);
}

#[tokio::test]
async fn policies_report_coverage_gaps_separately() {
    let (_dir, store) = open_store("policy").await;
    let frame_ids: Vec<_> = vec![
        seed_frame(&store, 10).await,
        seed_frame(&store, 11).await,
        seed_frame(&store, 12).await,
    ];
    let config = pilot_evaluator_config();
    let targets = vec![
        DecisionEvaluationTarget::new(
            "sample-tip",
            LabPredicate::pipette_tip_attached_v1(),
            pilot_predicate_targets()[0].1.clone(),
        )
        .unwrap(),
    ];
    let sampled = EnqueueDecisionEvaluationsRequest::new(
        DecisionCoveragePolicy::Sampled,
        config.clone(),
        DecisionEvaluationFrameSelection::FrameIds(frame_ids.clone()),
        targets.clone().to_vec(),
        4,
    )
    .unwrap();
    let sampled_report = store
        .enqueue_decision_evaluation_jobs(&sampled)
        .await
        .unwrap();
    assert!(sampled_report.policy_skipped() >= 1);
    let on_demand = EnqueueDecisionEvaluationsRequest::new(
        DecisionCoveragePolicy::OnDemand,
        config,
        DecisionEvaluationFrameSelection::FrameIds(vec![frame_ids[0].clone()]),
        targets,
        1,
    )
    .unwrap();
    store
        .enqueue_decision_evaluation_jobs(&on_demand)
        .await
        .unwrap();
    let coverage = store
        .summarize_decision_evaluation_coverage(&frame_ids, "sampled")
        .await
        .unwrap();
    assert!(
        sampled_report.policy_skipped() >= 1 || coverage.not_evaluated() >= 1,
        "sampled policy should expose skips or pending gaps"
    );
}

#[tokio::test]
async fn timeout_retry_and_terminal_failure_never_call_luna() {
    let (_dir, store) = open_store("timeout").await;
    let frame_id = seed_frame(&store, 3).await;
    let config = pilot_evaluator_config();
    let mut pilot = pilot_targets();
    let targets = [pilot.remove(0)];
    let request = EnqueueDecisionEvaluationsRequest::on_demand_pilot_frames(
        config.clone(),
        vec![frame_id.clone()],
    )
    .unwrap();
    store
        .enqueue_decision_evaluation_jobs(&request)
        .await
        .unwrap();
    let semantic = semantic_config();
    let faults = DecisionEvaluationWorkerFaults::new();
    let mut worker = DecisionEvaluationWorker::deterministic(
        &config,
        FrameDecisionEvaluatorLimits::pilot(),
        semantic.limits.decisions.clone(),
        FakeJevBehavior::Timeout,
        "timeout-worker",
        Arc::clone(&faults),
    );
    worker.run_batch(&store, &semantic, &targets).await.unwrap();
    worker.run_batch(&store, &semantic, &targets).await.unwrap();
    worker.run_batch(&store, &semantic, &targets).await.unwrap();
    assert_eq!(faults.luna_spy_calls(), 0);
    let job = store
        .get_decision_evaluation_job(&decision_evaluation_job_id(
            "jev-req-pilot-0",
            &LabPredicate::pipette_tip_attached_v1(),
            &subject_digest(targets[0].subject()),
            config.config_digest().as_str(),
            config.producer().model_revision(),
        ))
        .await
        .unwrap();
    assert_eq!(job.coverage(), Some(DecisionEvaluationCoverage::TimedOut));
}

#[tokio::test]
async fn provider_outage_records_failed_coverage_without_luna() {
    let (_dir, store) = open_store("outage").await;
    let frame_id = seed_frame(&store, 5).await;
    let config = pilot_evaluator_config();
    let mut pilot = pilot_targets();
    let targets = [pilot.remove(0)];
    let request =
        EnqueueDecisionEvaluationsRequest::on_demand_pilot_frames(config.clone(), vec![frame_id])
            .unwrap();
    store
        .enqueue_decision_evaluation_jobs(&request)
        .await
        .unwrap();
    let semantic = semantic_config();
    let faults = DecisionEvaluationWorkerFaults::new();
    let mut worker = DecisionEvaluationWorker::deterministic(
        &config,
        FrameDecisionEvaluatorLimits::pilot(),
        semantic.limits.decisions.clone(),
        FakeJevBehavior::ProviderOutage,
        "outage-worker",
        Arc::clone(&faults),
    );
    worker.run_batch(&store, &semantic, &targets).await.unwrap();
    assert_eq!(faults.luna_spy_calls(), 0);
    assert_eq!(faults.chat_spy_calls(), 0);
    let job = store
        .get_decision_evaluation_job(&decision_evaluation_job_id(
            "jev-req-pilot-0",
            &LabPredicate::pipette_tip_attached_v1(),
            &subject_digest(targets[0].subject()),
            config.config_digest().as_str(),
            config.producer().model_revision(),
        ))
        .await
        .unwrap();
    assert_eq!(job.coverage(), Some(DecisionEvaluationCoverage::Failed));
}

#[tokio::test]
async fn poison_evidence_records_failed_coverage() {
    let (_dir, store) = open_store("poison").await;
    let frame_id = seed_frame(&store, 4).await;
    let config = pilot_evaluator_config();
    let mut pilot = pilot_targets();
    let targets = [pilot.remove(0)];
    let request = EnqueueDecisionEvaluationsRequest::new(
        DecisionCoveragePolicy::OnDemand,
        config.clone(),
        DecisionEvaluationFrameSelection::FrameIds(vec![frame_id]),
        targets.clone().to_vec(),
        1,
    )
    .unwrap();
    store
        .enqueue_decision_evaluation_jobs(&request)
        .await
        .unwrap();
    let semantic = semantic_config();
    let faults = DecisionEvaluationWorkerFaults::new();
    faults.set_force_evidence_failure(true);
    let mut worker = DecisionEvaluationWorker::deterministic(
        &config,
        FrameDecisionEvaluatorLimits::pilot(),
        semantic.limits.decisions.clone(),
        FakeJevBehavior::OkPresent,
        "poison-worker",
        Arc::clone(&faults),
    );
    worker.run_batch(&store, &semantic, &targets).await.unwrap();
    let job = store
        .get_decision_evaluation_job(&decision_evaluation_job_id(
            "jev-req-pilot-0",
            &LabPredicate::pipette_tip_attached_v1(),
            &subject_digest(targets[0].subject()),
            config.config_digest().as_str(),
            config.producer().model_revision(),
        ))
        .await
        .unwrap();
    assert_eq!(job.coverage(), Some(DecisionEvaluationCoverage::Failed));
}

#[tokio::test]
async fn cancellation_preserves_terminal_coverage() {
    let (_dir, store) = open_store("cancel").await;
    let frame_id = seed_frame(&store, 5).await;
    let config = pilot_evaluator_config();
    let request =
        EnqueueDecisionEvaluationsRequest::on_demand_pilot_frames(config.clone(), vec![frame_id])
            .unwrap();
    store
        .enqueue_decision_evaluation_jobs(&request)
        .await
        .unwrap();
    let job_id = decision_evaluation_job_id(
        "jev-req-pilot-1",
        &LabPredicate::foreign_object_present_v1(),
        &subject_digest(&pilot_predicate_targets()[1].1),
        config.config_digest().as_str(),
        config.producer().model_revision(),
    );
    let now = a2a_lab_ot2::time::now().unwrap().as_micros();
    assert!(
        store
            .cancel_decision_evaluation_job(&job_id, now)
            .await
            .unwrap()
    );
    let job = store.get_decision_evaluation_job(&job_id).await.unwrap();
    assert_eq!(job.coverage(), Some(DecisionEvaluationCoverage::Cancelled));
}

#[tokio::test]
async fn commit_replay_keeps_idempotent_append_after_crash() {
    let (_dir, store) = open_store("replay").await;
    let frame_id = seed_frame(&store, 6).await;
    let config = pilot_evaluator_config();
    let mut pilot = pilot_targets();
    let targets = [pilot.remove(0)];
    let request = EnqueueDecisionEvaluationsRequest::new(
        DecisionCoveragePolicy::OnDemand,
        config.clone(),
        DecisionEvaluationFrameSelection::FrameIds(vec![frame_id.clone()]),
        targets.clone().to_vec(),
        1,
    )
    .unwrap();
    store
        .enqueue_decision_evaluation_jobs(&request)
        .await
        .unwrap();
    let semantic = semantic_config();
    let faults = DecisionEvaluationWorkerFaults::new();
    faults.set_crash_before_decision_commit(true);
    let mut worker = DecisionEvaluationWorker::deterministic(
        &config,
        FrameDecisionEvaluatorLimits::pilot(),
        semantic.limits.decisions.clone(),
        FakeJevBehavior::OkPresent,
        "replay-worker",
        Arc::clone(&faults),
    );
    worker.run_batch(&store, &semantic, &targets).await.unwrap();
    faults.set_crash_before_decision_commit(false);
    sqlx::query(
        "UPDATE frame_decision_evaluation_job_runtime SET next_retry_at_us = 0, job_state = 'pending', lease_owner = NULL",
    )
    .execute(store.sqlite_pool())
    .await
    .unwrap();
    worker.run_batch(&store, &semantic, &targets).await.unwrap();
    let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM frame_decisions WHERE frame_id = ?1")
        .bind(&frame_id)
        .fetch_one(store.sqlite_pool())
        .await
        .unwrap();
    assert_eq!(count, 1);
}

#[tokio::test]
async fn distinct_jev_revisions_coexist_as_separate_jobs() {
    let (_dir, store) = open_store("revision").await;
    let frame_id = seed_frame(&store, 7).await;
    let config_a = pilot_evaluator_config();
    let config_b = a2a_lab_dev_kit::FrameDecisionEvaluatorProviderConfig::new(
        "jev-1.14.1",
        a2a_lab_dev_kit::Sha256Digest::hash_bytes(b"rev-b-config"),
        config_a.threshold_policy_digest().clone(),
        config_a.supported_predicates().to_vec(),
    )
    .unwrap();
    let target = DecisionEvaluationTarget::new(
        "rev-a",
        LabPredicate::pipette_tip_attached_v1(),
        pilot_predicate_targets()[0].1.clone(),
    )
    .unwrap();
    let target_b = DecisionEvaluationTarget::new(
        "rev-b",
        LabPredicate::pipette_tip_attached_v1(),
        pilot_predicate_targets()[0].1.clone(),
    )
    .unwrap();
    for (config, targets) in [
        (config_a.clone(), vec![target]),
        (config_b.clone(), vec![target_b]),
    ] {
        let request = EnqueueDecisionEvaluationsRequest::new(
            DecisionCoveragePolicy::OnDemand,
            config,
            DecisionEvaluationFrameSelection::FrameIds(vec![frame_id.clone()]),
            targets,
            1,
        )
        .unwrap();
        store
            .enqueue_decision_evaluation_jobs(&request)
            .await
            .unwrap();
    }
    let semantic = semantic_config();
    let mut worker = worker_ok(&semantic);
    let targets_a = vec![
        DecisionEvaluationTarget::new(
            "rev-a",
            LabPredicate::pipette_tip_attached_v1(),
            pilot_predicate_targets()[0].1.clone(),
        )
        .unwrap(),
    ];
    worker
        .run_batch(&store, &semantic, &targets_a)
        .await
        .unwrap();
    let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM frame_decisions WHERE frame_id = ?1")
        .bind(&frame_id)
        .fetch_one(store.sqlite_pool())
        .await
        .unwrap();
    assert_eq!(count, 1);
    let jobs: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM frame_decision_evaluation_jobs WHERE frame_id = ?1",
    )
    .bind(&frame_id)
    .fetch_one(store.sqlite_pool())
    .await
    .unwrap();
    assert_eq!(jobs, 2);
}

#[tokio::test]
async fn partial_predicate_support_marks_unsupported_without_llm_fallback() {
    let (_dir, store) = open_store("partial").await;
    let frame_id = seed_frame(&store, 8).await;
    let config = pilot_evaluator_config();
    let targets = pilot_targets();
    let request = EnqueueDecisionEvaluationsRequest::new(
        DecisionCoveragePolicy::OnDemand,
        config.clone(),
        a2a_lab_ot2::videos::dvr::DecisionEvaluationFrameSelection::FrameIds(vec![frame_id]),
        targets.clone().to_vec(),
        1,
    )
    .unwrap();
    store
        .enqueue_decision_evaluation_jobs(&request)
        .await
        .unwrap();
    let semantic = semantic_config();
    let evaluator = a2a_lab_dev_kit::FakeFrameDecisionEvaluator::new(
        config.clone(),
        FrameDecisionEvaluatorLimits::pilot(),
        FakeJevBehavior::OkPresent,
    )
    .with_partial_predicates(vec![LabPredicate::pipette_tip_attached_v1()]);
    let mut worker = a2a_lab_ot2::videos::dvr::DecisionEvaluationWorker::new(
        a2a_lab_dev_kit::FrameDecisionEvaluationRouter::new(evaluator),
        config,
        FrameDecisionEvaluatorLimits::pilot(),
        semantic.limits.decisions.clone(),
        "partial-worker",
        DecisionEvaluationWorkerFaults::new(),
    );
    worker.run_batch(&store, &semantic, &targets).await.unwrap();
    let unsupported: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM frame_decision_evaluation_job_runtime WHERE coverage = 'unsupported'",
    )
    .fetch_one(store.sqlite_pool())
    .await
    .unwrap();
    assert!(unsupported >= 2);
}

#[tokio::test]
async fn indexing_worker_and_decision_worker_stay_isolated() {
    let (_dir, store) = open_store("isolation").await;
    let frame_id = seed_frame(&store, 9).await;
    let semantic = semantic_config();
    let indexing_faults = IndexingWorkerFaults::new();
    let indexing_worker = a2a_lab_ot2::videos::dvr::DeterministicIndexingWorker::deterministic(
        &semantic,
        "luna-worker",
        indexing_faults,
    )
    .await
    .unwrap();
    let config = pilot_evaluator_config();
    let tip_target = DecisionEvaluationTarget::new(
        "isolate-tip",
        LabPredicate::pipette_tip_attached_v1(),
        pilot_predicate_targets()[0].1.clone(),
    )
    .unwrap();
    store
        .enqueue_decision_evaluation_jobs(
            &EnqueueDecisionEvaluationsRequest::new(
                DecisionCoveragePolicy::OnDemand,
                config.clone(),
                DecisionEvaluationFrameSelection::FrameIds(vec![frame_id.clone()]),
                vec![tip_target.clone()],
                1,
            )
            .unwrap(),
        )
        .await
        .unwrap();
    let mut decision_worker = worker_ok(&semantic);
    decision_worker
        .run_batch(&store, &semantic, &[tip_target])
        .await
        .unwrap();
    indexing_worker.run_batch(&store, &semantic).await.unwrap();
    let decision = store
        .get_decision_evaluation_job(&decision_evaluation_job_id(
            "jev-req-isolate-tip",
            &LabPredicate::pipette_tip_attached_v1(),
            &subject_digest(&pilot_predicate_targets()[0].1),
            config.config_digest().as_str(),
            config.producer().model_revision(),
        ))
        .await
        .unwrap();
    assert_eq!(
        decision.coverage(),
        Some(DecisionEvaluationCoverage::Evaluated)
    );
    let manifest = store.get_manifest_frame(&frame_id).await.unwrap();
    assert!(manifest.description().is_some());
}
