//! SQLite persistence for immutable Jev frame decisions.

use std::sync::Arc;

use a2a_lab_dev_kit::{
    DecisionEvaluationCoverage, DecisionEvidenceItem, DecisionHistoryPolicy,
    DecisionIdempotencyKey, DecisionOutcome, DecisionSubject, DvrStoreNamespace, DvrTrackId,
    FrameDecisionAppendError, FrameDecisionEvaluatorLimits, FrameDecisionEvaluatorProviderConfig,
    FrameDecisionId, FrameDecisionLedgerProvider, FrameDecisionReadLimits, FrameDecisionReadScope,
    FrameDecisionRecord, FrameRoiTransform, JevEvaluationId, JevEvaluationProvenance,
    JevProducerIdentity, JevStructuredOutput, LabPredicate, MemoryVideos, PageRequest,
    PipetteChannelSubject, ReadFrameDecisionHistoryRequest, SavedFrameRef, Sha256Digest, TimeRange,
    UtcTimestamp, VideoSourceId,
};
use a2a_lab_ot2::IndexedVideoDvr;
use a2a_lab_ot2::videos::dvr::{DEFAULT_SEMANTIC_STORE_NAMESPACE, DvrFrameDecisionLedger};

fn temp_root(name: &str) -> std::path::PathBuf {
    std::env::temp_dir().join(format!(
        "a2a-lab-ot2-decisions-{name}-{}",
        std::process::id()
    ))
}

async fn open_store(name: &str) -> (std::path::PathBuf, Arc<IndexedVideoDvr>) {
    let dir = temp_root(name);
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

fn evaluation_id(suffix: &str) -> JevEvaluationId {
    JevEvaluationId::new(format!("jev-req-{suffix}")).unwrap()
}

fn provenance(captured: &str, evaluated: &str, eval_suffix: &str) -> JevEvaluationProvenance {
    JevEvaluationProvenance::new(
        UtcTimestamp::parse(captured).unwrap(),
        UtcTimestamp::parse(evaluated).unwrap(),
        JevProducerIdentity::default_production(),
        evaluation_id(eval_suffix),
        Sha256Digest::hash_bytes(b"config-v1"),
        Sha256Digest::hash_bytes(b"prompt-v1"),
        Sha256Digest::hash_bytes(b"criteria-v1"),
        Sha256Digest::hash_bytes(b"threshold-v1"),
        FrameRoiTransform::new([0.0, 0.0, 1.0, 1.0]).unwrap(),
        Sha256Digest::hash_bytes(b"jev-response-bytes"),
        DecisionEvaluationCoverage::Evaluated,
        Option::<&str>::None,
    )
    .unwrap()
}

fn tip_subject() -> DecisionSubject {
    DecisionSubject::PipetteChannel(
        PipetteChannelSubject::new(
            "left-pipetter",
            "channel-1",
            Some(FrameRoiTransform::new([0.2, 0.3, 0.5, 0.7]).unwrap()),
        )
        .unwrap(),
    )
}

fn tip_evidence(saved: &SavedFrameRef) -> Vec<DecisionEvidenceItem> {
    vec![
        DecisionEvidenceItem::saved_frame_revision(
            saved.frame_id().clone(),
            FrameRoiTransform::new([0.2, 0.3, 0.5, 0.7]).unwrap(),
        )
        .unwrap(),
    ]
}

fn tip_record(
    saved: &SavedFrameRef,
    outcome: DecisionOutcome,
    key: &str,
    supersedes: Option<FrameDecisionId>,
    eval_suffix: &str,
    captured: &str,
    evaluated: &str,
) -> FrameDecisionRecord {
    let output = JevStructuredOutput::empty();
    FrameDecisionRecord::new(
        DecisionIdempotencyKey::new(key).unwrap(),
        saved.clone(),
        LabPredicate::pipette_tip_attached_v1(),
        tip_subject(),
        tip_evidence(saved),
        outcome,
        output,
        provenance(captured, evaluated, eval_suffix),
        supersedes,
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

fn evaluator_config() -> FrameDecisionEvaluatorProviderConfig {
    FrameDecisionEvaluatorProviderConfig::pilot(
        Sha256Digest::hash_bytes(b"eval-config"),
        Sha256Digest::hash_bytes(b"threshold"),
    )
    .unwrap()
}

#[tokio::test]
async fn append_idempotent_and_conflict_without_mutation() {
    let (_dir, store) = open_store("idem").await;
    let payload = b"v1";
    let saved = frame(payload, "2024-06-01T12:00:00Z");
    register_saved(&store, &saved, payload).await;
    let record = tip_record(
        &saved,
        DecisionOutcome::Present,
        "jev-req-a:lab.pipette_tip_attached:left-1",
        None,
        "a",
        "2024-06-01T12:00:00Z",
        "2024-06-01T12:00:02Z",
    );
    store.append_frame_decision(record.clone()).await.unwrap();
    let replay = store.append_frame_decision(record).await.unwrap();
    assert!(replay.replayed());

    let conflict = tip_record(
        &saved,
        DecisionOutcome::Absent,
        "jev-req-a:lab.pipette_tip_attached:left-1",
        None,
        "a",
        "2024-06-01T12:00:00Z",
        "2024-06-01T12:00:03Z",
    );
    let err = store.append_frame_decision(conflict).await.unwrap_err();
    assert!(matches!(
        err,
        FrameDecisionAppendError::IdempotencyConflict { .. }
    ));

    let history = store
        .read_frame_decision_history(
            &ReadFrameDecisionHistoryRequest::new(
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
        )
        .await
        .unwrap();
    assert_eq!(history.entries().items().len(), 1);
    assert_eq!(
        history.entries().items()[0].record().outcome(),
        DecisionOutcome::Present
    );
}

#[tokio::test]
async fn supersession_validation_fails_without_mutation() {
    let (_dir, store) = open_store("super").await;
    let saved_a = frame(b"a", "2024-06-01T12:00:00Z");
    let saved_b = frame(b"b", "2024-06-01T12:00:01Z");
    register_saved(&store, &saved_a, b"a").await;
    register_saved(&store, &saved_b, b"b").await;

    let first = tip_record(
        &saved_a,
        DecisionOutcome::Unknown,
        "jev-req-s:lab.pipette_tip_attached:left-1:v1",
        None,
        "s",
        "2024-06-01T12:00:00Z",
        "2024-06-01T12:00:02Z",
    );
    let first_id = store
        .append_frame_decision(first)
        .await
        .unwrap()
        .record()
        .decision_id()
        .clone();

    let cross = tip_record(
        &saved_b,
        DecisionOutcome::Present,
        "jev-req-s2:lab.pipette_tip_attached:left-1:v2",
        Some(first_id.clone()),
        "s2",
        "2024-06-01T12:00:01Z",
        "2024-06-01T12:00:03Z",
    );
    assert!(matches!(
        store.append_frame_decision(cross).await,
        Err(FrameDecisionAppendError::InvalidSupersession { .. })
    ));

    let missing = tip_record(
        &saved_a,
        DecisionOutcome::Present,
        "jev-req-s3:lab.pipette_tip_attached:left-1:v2",
        Some(
            FrameDecisionId::new(
                "0000000000000000000000000000000000000000000000000000000000000000",
            )
            .unwrap(),
        ),
        "s3",
        "2024-06-01T12:00:00Z",
        "2024-06-01T12:00:04Z",
    );
    assert!(matches!(
        store.append_frame_decision(missing).await,
        Err(FrameDecisionAppendError::InvalidSupersession { .. })
    ));

    let successor = tip_record(
        &saved_a,
        DecisionOutcome::Present,
        "jev-req-s4:lab.pipette_tip_attached:left-1:v2",
        Some(first_id.clone()),
        "s4",
        "2024-06-01T12:00:00Z",
        "2024-06-01T12:00:05Z",
    );
    let second_id = store
        .append_frame_decision(successor)
        .await
        .expect("same-stream supersession")
        .record()
        .decision_id()
        .clone();
    assert_eq!(
        store
            .read_frame_decision_history(
                &ReadFrameDecisionHistoryRequest::new(
                    scope(&saved_a),
                    Some(LabPredicate::pipette_tip_attached_v1()),
                    Some(tip_subject()),
                    None,
                    DecisionHistoryPolicy::FullHistory,
                    FrameDecisionReadLimits::pilot(),
                    PageRequest::new(None, 10).unwrap(),
                    Vec::new(),
                )
                .unwrap()
            )
            .await
            .unwrap()
            .entries()
            .items()
            .len(),
        2
    );
    assert_ne!(first_id, second_id);
}

#[tokio::test]
#[allow(clippy::too_many_lines)]
async fn history_policies_preserve_disagreement_and_exclude_human_labels() {
    let (_dir, store) = open_store("hist").await;
    let saved = frame(b"h", "2024-06-01T12:00:00Z");
    register_saved(&store, &saved, b"h").await;

    let rev_a = JevProducerIdentity::new("jev-1.14.0").unwrap();
    let rev_b = JevProducerIdentity::new("jev-1.14.1").unwrap();
    let prov_b = JevEvaluationProvenance::new(
        saved.captured_at(),
        UtcTimestamp::parse("2024-06-01T12:00:04Z").unwrap(),
        rev_b.clone(),
        evaluation_id("hb"),
        Sha256Digest::hash_bytes(b"config-v2"),
        Sha256Digest::hash_bytes(b"prompt-v1"),
        Sha256Digest::hash_bytes(b"criteria-v1"),
        Sha256Digest::hash_bytes(b"threshold-v1"),
        FrameRoiTransform::new([0.0, 0.0, 1.0, 1.0]).unwrap(),
        Sha256Digest::hash_bytes(b"jev-response-bytes-b"),
        DecisionEvaluationCoverage::Evaluated,
        Option::<&str>::None,
    )
    .unwrap();
    let record_a = FrameDecisionRecord::new(
        DecisionIdempotencyKey::new("jev-req-ha:lab.pipette_tip_attached:left-1:rev-a").unwrap(),
        saved.clone(),
        LabPredicate::pipette_tip_attached_v1(),
        tip_subject(),
        tip_evidence(&saved),
        DecisionOutcome::Present,
        JevStructuredOutput::empty(),
        provenance("2024-06-01T12:00:00Z", "2024-06-01T12:00:01Z", "ha"),
        None,
    )
    .unwrap();
    let record_b = FrameDecisionRecord::new(
        DecisionIdempotencyKey::new("jev-req-hb:lab.pipette_tip_attached:left-1:rev-b").unwrap(),
        saved.clone(),
        LabPredicate::pipette_tip_attached_v1(),
        tip_subject(),
        tip_evidence(&saved),
        DecisionOutcome::Absent,
        JevStructuredOutput::empty(),
        prov_b,
        None,
    )
    .unwrap();
    store.append_frame_decision(record_a).await.unwrap();
    store.append_frame_decision(record_b).await.unwrap();
    store
        .insert_human_corpus_label(
            "human-label-1",
            saved.frame_id().as_str(),
            "lab.pipette_tip_attached",
            r#"{"outcome":"present","annotator":"pilot"}"#,
        )
        .await
        .unwrap();

    let full = store
        .read_frame_decision_history(
            &ReadFrameDecisionHistoryRequest::new(
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
        )
        .await
        .unwrap();
    assert_eq!(full.entries().items().len(), 2);
    assert_ne!(
        full.entries().items()[0].record().outcome(),
        full.entries().items()[1].record().outcome()
    );
    let capture_order: Vec<_> = full
        .entries()
        .items()
        .iter()
        .map(|entry| entry.record().frame().captured_at())
        .collect();
    assert_eq!(capture_order[0], capture_order[1]);
    let eval_order: Vec<_> = full
        .entries()
        .items()
        .iter()
        .map(|entry| entry.record().provenance().evaluated_at())
        .collect();
    assert!(eval_order[0] < eval_order[1]);

    let latest = store
        .read_frame_decision_history(
            &ReadFrameDecisionHistoryRequest::new(
                scope(&saved),
                None,
                None,
                Some(rev_a.clone()),
                DecisionHistoryPolicy::LatestAuthorizedProduction,
                FrameDecisionReadLimits::pilot(),
                PageRequest::new(None, 10).unwrap(),
                Vec::new(),
            )
            .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(latest.entries().items().len(), 1);
    assert_eq!(
        latest.entries().items()[0].record().provenance().producer(),
        &rev_a
    );
}

#[tokio::test]
async fn restart_concurrent_append_and_tombstones() {
    let (dir, store) = open_store("restart").await;
    let saved = frame(b"r", "2024-06-01T12:00:00Z");
    register_saved(&store, &saved, b"r").await;
    let record = tip_record(
        &saved,
        DecisionOutcome::Present,
        "jev-req-r:lab.pipette_tip_attached:left-1",
        None,
        "r",
        "2024-06-01T12:00:00Z",
        "2024-06-01T12:00:02Z",
    );
    let store2 = store.clone();
    let left = store.append_frame_decision(record.clone());
    let right = store2.append_frame_decision(record);
    let (a, b) = tokio::join!(left, right);
    assert!(a.is_ok() || b.is_ok());
    drop(store);
    let reopened = Arc::new(IndexedVideoDvr::open(&dir, 86_400).await.unwrap());
    let history = reopened
        .read_frame_decision_history(
            &ReadFrameDecisionHistoryRequest::new(
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
        )
        .await
        .unwrap();
    assert_eq!(history.entries().items().len(), 1);

    sqlx::query(
        "UPDATE semantic_frame_revisions SET retained_state = 'expired' WHERE frame_id = ?1",
    )
    .bind(saved.frame_id().as_str())
    .execute(reopened.semantic_pool())
    .await
    .unwrap();
    reopened
        .reconcile_frame_decision_tombstones()
        .await
        .unwrap();
    let tombstoned = reopened
        .read_frame_decision_history(
            &ReadFrameDecisionHistoryRequest::new(
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
        )
        .await
        .unwrap();
    assert_eq!(tombstoned.entries().items().len(), 1);
    assert!(matches!(
        tombstoned.entries().items()[0].evidence_availability()[0],
        a2a_lab_dev_kit::DecisionEvidenceAvailability::Unavailable { .. }
    ));
}

#[tokio::test]
async fn append_rejects_unregistered_manifest_and_superseded_revision_accepts() {
    let (_dir, store) = open_store("manifest").await;
    let saved = frame(b"alias", "2024-06-01T12:00:00Z");
    let ghost = frame(b"ghost", "2024-06-01T12:00:00Z");
    register_saved(&store, &saved, b"alias").await;

    let missing = tip_record(
        &ghost,
        DecisionOutcome::Unknown,
        "jev-req-ghost:lab.pipette_tip_attached:left-1",
        None,
        "ghost",
        "2024-06-01T12:00:00Z",
        "2024-06-01T12:00:01Z",
    );
    assert!(matches!(
        store.append_frame_decision(missing).await,
        Err(FrameDecisionAppendError::Validation(_))
    ));

    store
        .append_mjpeg_frame(saved.source_id(), saved.captured_at(), 200_000, b"replaced")
        .await
        .unwrap();
    let (superseded, _) = store
        .register_manifest_at_dvr_key(
            &store_ns(),
            saved.source_id(),
            saved.track_id(),
            saved.captured_at(),
        )
        .await
        .unwrap();
    assert_ne!(saved.frame_id(), superseded.frame_id());

    let tombstone = tip_record(
        &saved,
        DecisionOutcome::Present,
        "jev-req-super:lab.pipette_tip_attached:left-1",
        None,
        "super",
        "2024-06-01T12:00:00Z",
        "2024-06-01T12:00:05Z",
    );
    store.append_frame_decision(tombstone).await.unwrap();
}

#[tokio::test]
async fn decisions_persist_while_description_embedding_pending() {
    let (_dir, store) = open_store("pending").await;
    let saved = frame(b"p", "2024-06-01T12:00:00Z");
    register_saved(&store, &saved, b"p").await;
    let manifest = store
        .get_manifest_frame(saved.frame_id().as_str())
        .await
        .unwrap();
    assert_eq!(
        manifest.description_checkpoint(),
        a2a_lab_ot2::videos::dvr::IndexingCheckpoint::Pending
    );
    let record = tip_record(
        &saved,
        DecisionOutcome::Unknown,
        "jev-req-p:lab.pipette_tip_attached:left-1",
        None,
        "p",
        "2024-06-01T12:00:00Z",
        "2024-06-01T12:00:02Z",
    );
    store.append_frame_decision(record).await.unwrap();

    let ledger = DvrFrameDecisionLedger::authorized_producer(
        store,
        &evaluator_config(),
        FrameDecisionEvaluatorLimits::pilot(),
    )
    .unwrap();
    let caps =
        ledger
            .get_frame_decision_capabilities(
                a2a_lab_dev_kit::GetFrameDecisionCapabilitiesRequest::new(),
            )
            .await
            .unwrap();
    assert!(caps.is_registered());
}
