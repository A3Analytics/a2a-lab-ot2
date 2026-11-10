//! End-to-end decision APIs over SQLite DVR storage and dev-kit service commands.

use std::sync::Arc;

use a2a_lab_dev_kit::{
    A2aLabCommand, A2aLabResult, A2aLabService, AppendFrameDecisionRequest,
    DecisionEvaluationCoverage, DecisionEvidenceItem, DecisionHistoryPolicy,
    DecisionIdempotencyKey, DecisionOutcome, DecisionSubject, DvrStoreNamespace, DvrTrackId,
    EmptyImages, FrameDecisionEvaluationOwnership, FrameDecisionEvaluatorLimits,
    FrameDecisionEvaluatorProviderConfig, FrameDecisionLedgerProvider, FrameDecisionReadLimits,
    FrameDecisionReadScope, FrameDecisionRecord, FrameRoiTransform,
    GetFrameDecisionCapabilitiesRequest, JevEvaluationId, JevEvaluationProvenance,
    JevProducerIdentity, JevStructuredOutput, LabPredicate, MemoryLogs, MemoryMetrics, MemoryTasks,
    MemoryVideos, PageRequest, PipetteChannelSubject, ReadFrameDecisionHistoryRequest,
    RejectedDecisionProvenance, SavedFrameRef, Sha256Digest, TimeRange, UtcTimestamp,
    VideoSourceId, reject_decision_provenance,
};
use a2a_lab_ot2::IndexedVideoDvr;
use a2a_lab_ot2::videos::dvr::{DEFAULT_SEMANTIC_STORE_NAMESPACE, DvrFrameDecisionLedger};

async fn open_store(name: &str) -> (std::path::PathBuf, Arc<IndexedVideoDvr>) {
    let dir = std::env::temp_dir().join(format!(
        "a2a-lab-ot2-decision-api-{name}-{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    let store = Arc::new(IndexedVideoDvr::open(&dir, 86_400).await.unwrap());
    (dir, store)
}

fn store_ns() -> DvrStoreNamespace {
    DvrStoreNamespace::new(DEFAULT_SEMANTIC_STORE_NAMESPACE).unwrap()
}

fn deck_source() -> VideoSourceId {
    VideoSourceId::new("deck-left").unwrap()
}

fn deck_track() -> DvrTrackId {
    MemoryVideos::mjpeg_track_id(&deck_source()).unwrap()
}

fn frame(payload: &[u8], captured: &str) -> SavedFrameRef {
    SavedFrameRef::from_payload_bytes(
        store_ns(),
        deck_source(),
        deck_track(),
        UtcTimestamp::parse(captured).unwrap(),
        payload,
        None,
    )
    .unwrap()
}

async fn register_saved(store: &IndexedVideoDvr, saved: &SavedFrameRef, payload: &[u8]) {
    store
        .append_mjpeg_frame(saved.source_id(), saved.captured_at(), 200_000, payload)
        .await
        .unwrap();
    store
        .register_manifest_at_dvr_key(
            &store_ns(),
            saved.source_id(),
            saved.track_id(),
            saved.captured_at(),
        )
        .await
        .unwrap();
}

fn tip_record(saved: &SavedFrameRef, outcome: DecisionOutcome, key: &str) -> FrameDecisionRecord {
    let subject = DecisionSubject::PipetteChannel(
        PipetteChannelSubject::new(
            "left-pipetter",
            "channel-1",
            Some(FrameRoiTransform::new([0.2, 0.3, 0.5, 0.7]).unwrap()),
        )
        .unwrap(),
    );
    let evidence = vec![
        DecisionEvidenceItem::saved_frame_revision(
            saved.frame_id().clone(),
            FrameRoiTransform::new([0.2, 0.3, 0.5, 0.7]).unwrap(),
        )
        .unwrap(),
    ];
    let provenance = JevEvaluationProvenance::new(
        UtcTimestamp::parse("2024-06-01T12:00:00Z").unwrap(),
        UtcTimestamp::parse("2024-06-01T12:00:02Z").unwrap(),
        JevProducerIdentity::default_production(),
        JevEvaluationId::new("jev-req-e2e-api").unwrap(),
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

fn scope(saved: &SavedFrameRef) -> FrameDecisionReadScope {
    FrameDecisionReadScope::new(
        saved.source_id().clone(),
        TimeRange::new(
            UtcTimestamp::parse("2024-06-01T12:00:00Z").unwrap(),
            UtcTimestamp::parse("2024-06-01T12:10:00Z").unwrap(),
        )
        .unwrap(),
        Some(saved.clone()),
    )
    .unwrap()
}

fn evaluation_id() -> JevEvaluationId {
    JevEvaluationId::new("jev-req-e2e-api").unwrap()
}

fn ledger_service(
    ledger: Arc<dyn FrameDecisionLedgerProvider>,
) -> A2aLabService<MemoryLogs, MemoryMetrics, MemoryTasks, EmptyImages, MemoryVideos> {
    A2aLabService::new(
        MemoryLogs::default(),
        MemoryMetrics::default(),
        MemoryTasks::default(),
    )
    .with_videos(MemoryVideos::default())
    .with_frame_decisions(ledger)
}

#[tokio::test]
async fn service_wires_capabilities_append_and_bounded_history() {
    let (_dir, store) = open_store("api").await;
    let saved = frame(b"api", "2024-06-01T12:00:00Z");
    register_saved(&store, &saved, b"api").await;
    let config = FrameDecisionEvaluatorProviderConfig::pilot(
        Sha256Digest::hash_bytes(b"eval-config"),
        Sha256Digest::hash_bytes(b"threshold"),
    )
    .unwrap();
    let ledger = DvrFrameDecisionLedger::authorized_producer(
        store,
        &config,
        FrameDecisionEvaluatorLimits::pilot(),
    )
    .unwrap();
    let lab = ledger_service(Arc::new(ledger) as Arc<dyn FrameDecisionLedgerProvider>);

    let caps = lab
        .execute(A2aLabCommand::GetFrameDecisionCapabilities(
            GetFrameDecisionCapabilitiesRequest::new(),
        ))
        .await
        .unwrap()
        .task
        .result;
    let A2aLabResult::GetFrameDecisionCapabilities(caps) = caps else {
        panic!("capabilities");
    };
    assert!(caps.is_registered());

    let record = tip_record(
        &saved,
        DecisionOutcome::Present,
        "jev-req-e2e-api:lab.pipette_tip_attached:left-1",
    );
    let append = AppendFrameDecisionRequest::new(
        record,
        FrameDecisionEvaluationOwnership::new(evaluation_id()),
    )
    .unwrap();
    let first = lab
        .execute(A2aLabCommand::AppendFrameDecision(append.clone()))
        .await
        .unwrap()
        .task
        .result;
    let A2aLabResult::AppendFrameDecision(first) = first else {
        panic!("append");
    };
    assert!(!first.replayed());
    let replay = lab
        .execute(A2aLabCommand::AppendFrameDecision(append))
        .await
        .unwrap()
        .task
        .result;
    let A2aLabResult::AppendFrameDecision(replay) = replay else {
        panic!("replay");
    };
    assert!(replay.replayed());

    let history = lab
        .execute(A2aLabCommand::ReadFrameDecisionHistory(
            ReadFrameDecisionHistoryRequest::new(
                scope(&saved),
                None,
                None,
                None,
                DecisionHistoryPolicy::FullHistory,
                FrameDecisionReadLimits::pilot(),
                PageRequest::new(None, 10).unwrap(),
                Vec::new(),
            )
            .unwrap(),
        ))
        .await
        .unwrap()
        .task
        .result;
    let A2aLabResult::ReadFrameDecisionHistory(history) = history else {
        panic!("history");
    };
    assert_eq!(history.entries().items().len(), 1);
    assert!(
        history.entries().items()[0]
            .record()
            .provenance()
            .evaluated_at()
            >= history.entries().items()[0].record().frame().captured_at()
    );
}

#[test]
fn non_jev_provenance_rejected_without_fallback() {
    let err =
        reject_decision_provenance(RejectedDecisionProvenance::LlmAssessment).expect_err("llm");
    assert!(err.to_string().contains("LLM"));
}
