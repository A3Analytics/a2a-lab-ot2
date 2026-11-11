//! Serve wiring fixtures for v2 semantic configuration and Jev-only decisions.

use std::fs;
use std::path::PathBuf;
use std::sync::Arc;

use a2a_lab_dev_kit::{
    A2aLabCommand, A2aLabResult, A2aLabService, AppendFrameDecisionRequest, DecisionOutcome,
    DecisionSubject, FrameDecisionEvaluationOwnership, FrameDecisionEvaluatorLimits,
    FrameDecisionEvaluatorProviderConfig, FrameDescriptionBackend,
    GetFrameDecisionCapabilitiesRequest, GetSemanticSearchCapabilitiesRequest, JevEvaluationId,
    JevEvaluationProvenance, JevProducerIdentity, JevStructuredOutput, LabPredicate, MemoryLogs,
    MemoryMetrics, MemoryTasks, MemoryVideos, PipetteChannelSubject, Sha256Digest, VideoSourceId,
};
use a2a_lab_ot2::videos::dvr::{
    AnnotationQueueWorker, AnnotationQueueWorkerBundle, AnyIndexingAnnotationWorker,
    DEFAULT_SEMANTIC_STORE_NAMESPACE, DeterministicIndexingWorker, DvrFrameDecisionLedger,
    IndexingWorkerFaults, IndexingWorkerLane,
};
use a2a_lab_ot2::videos::semantic_indexing::{
    SemanticServeInjections, build_deterministic_providers, description_provider_config,
    embedding_provider_config,
};
use a2a_lab_ot2::{
    ConversationStore, ExplicitSetting, LabConfigInputs, LiveVideoInjections, ModelProvider,
    RecordingServeAgentFactory, ServeChatLegacyInputs, build_serve_lab_agent,
    open_resolved_video_catalog_with_semantic, resolve_lab_config, resolve_semantic_indexing,
};

const SEMANTIC_LIMITS: &str = r#""limits":{"worker":{"max_batch_items":4,"max_concurrency":2,"max_queue_backlog":64,"max_retry_attempts":3,"job_deadline":"30s","retry_backoff":"1s"},"search":{"top_k":10,"candidate_limit":1000,"max_neighbor_frames":5,"query_window":"2h"},"roi":{"min_edge_pixels":280,"max_edge_pixels":1568,"max_rois_per_frame":8},"storage":{"vector_index_path":"./state/semantic-index","max_disk_bytes":1073741824},"decisions":{"max_batch_items":8,"max_concurrency":2,"job_deadline":"45s","coverage_policy":"on_demand"}}"#;

fn temp_root(name: &str) -> PathBuf {
    std::env::temp_dir().join(format!(
        "a2a-lab-ot2-semantic-serve-{name}-{}",
        std::process::id()
    ))
}

fn write_config(dir: &PathBuf, body: &str) -> PathBuf {
    fs::create_dir_all(dir).unwrap();
    let path = dir.join("a2a-lab.json");
    fs::write(&path, body).unwrap();
    path
}

fn semantic_document(description_model: Option<&str>) -> String {
    let model = description_model
        .map(|value| format!(r#","model":"{value}""#))
        .unwrap_or_default();
    format!(
        r#"{{"schema_version":2,"chat":{{"provider":"openai","model":"chat-only"}},"videos":{{"sources":[{{"id":"camera","device_index":0,"format":"mjpeg"}}]}},"semantic_indexing":{{"description":{{"provider":"bedrock"{model}}},"embeddings":{{"provider":"bedrock","model":"text-embed-v1","space":{{"dimensions":4,"normalization":"l2_unit","metric":"cosine"}}}},{SEMANTIC_LIMITS},"decisions":{{"provider":"jev","model":"jev-1.14.0"}}}}}}"#
    )
}

fn inputs_with_config(path: PathBuf) -> LabConfigInputs {
    LabConfigInputs {
        config_path: ExplicitSetting::cli(Some(path)),
        ..LabConfigInputs::default()
    }
}

#[test]
fn luna_and_embedding_configs_are_independent_from_chat() {
    let dir = temp_root("models");
    let path = write_config(&dir, &semantic_document(Some("custom-luna")));
    let inputs = inputs_with_config(path);
    let resolved = resolve_lab_config(&inputs).unwrap();
    let semantic = resolve_semantic_indexing(&inputs, &resolved.video)
        .unwrap()
        .expect("semantic");
    let description = description_provider_config(&semantic.config.description).unwrap();
    let embedding =
        embedding_provider_config(semantic.config.embeddings.as_ref().unwrap()).unwrap();
    assert_eq!(description.backend(), FrameDescriptionBackend::Bedrock);
    assert_eq!(description.model(), "custom-luna");
    assert_ne!(description.model(), "chat-only");
    assert_eq!(embedding.model(), "text-embed-v1");
}

#[tokio::test]
async fn enabled_semantic_registers_capabilities_for_configured_source() {
    let dir = temp_root("serve");
    let path = write_config(&dir, &semantic_document(None));
    let inputs = inputs_with_config(path);
    let resolved = resolve_lab_config(&inputs).unwrap();
    let semantic = resolve_semantic_indexing(&inputs, &resolved.video)
        .unwrap()
        .expect("semantic");
    let providers = build_deterministic_providers(&semantic.config)
        .await
        .unwrap();
    let live = DeterministicIndexingWorker::new(
        providers,
        semantic.config.limits.worker.clone(),
        "fixture-worker-live",
        IndexingWorkerFaults::new(),
    )
    .with_lane(IndexingWorkerLane::Live);
    let backfill_providers = build_deterministic_providers(&semantic.config)
        .await
        .unwrap();
    let backfill = DeterministicIndexingWorker::new(
        backfill_providers,
        semantic.config.limits.worker.clone(),
        "fixture-worker-backfill",
        IndexingWorkerFaults::new(),
    )
    .with_lane(IndexingWorkerLane::Backfill);
    let injections = SemanticServeInjections {
        workers: Some(AnnotationQueueWorkerBundle::new(
            AnnotationQueueWorker::new(
                AnyIndexingAnnotationWorker::Deterministic(Arc::new(live)),
                semantic.config.clone(),
            ),
            AnnotationQueueWorker::new(
                AnyIndexingAnnotationWorker::Deterministic(Arc::new(backfill)),
                semantic.config.clone(),
            ),
        )),
        force_deterministic_worker: true,
        ..SemanticServeInjections::default()
    };
    let catalog = open_resolved_video_catalog_with_semantic(
        &resolved.video,
        LiveVideoInjections::none(),
        Some(&inputs),
        Some(injections),
    )
    .await
    .unwrap()
    .expect("catalog");
    let ledger = catalog
        .frame_decision_ledger()
        .unwrap()
        .expect("jev ledger");
    assert!(
        catalog.semantic_bundle().decision_worker.is_none(),
        "serve must not fabricate Jev decisions when live Jev is disabled"
    );
    assert_eq!(
        catalog.semantic_bundle().decision_unavailable,
        Some((
            a2a_lab_dev_kit::DecisionEvaluationCoverage::NotEvaluated,
            "jev_live_disabled",
        ))
    );
    let search = catalog.semantic_bundle().search.clone().expect("search");
    let lab = A2aLabService::new(
        MemoryLogs::default(),
        MemoryMetrics::default(),
        MemoryTasks::default(),
    )
    .with_videos(catalog.provider())
    .with_frame_decisions(ledger)
    .with_semantic_search(search);
    let caps = lab
        .execute(A2aLabCommand::GetSemanticSearchCapabilities(
            GetSemanticSearchCapabilitiesRequest::new(),
        ))
        .await
        .unwrap()
        .task
        .result;
    let A2aLabResult::GetSemanticSearchCapabilities(caps) = caps else {
        panic!("capabilities");
    };
    assert!(caps.is_registered());
    let decision_caps = lab
        .execute(A2aLabCommand::GetFrameDecisionCapabilities(
            GetFrameDecisionCapabilitiesRequest::new(),
        ))
        .await
        .unwrap()
        .task
        .result;
    let A2aLabResult::GetFrameDecisionCapabilities(decision_caps) = decision_caps else {
        panic!("frame decision capabilities");
    };
    assert!(decision_caps.is_registered());
}

#[tokio::test]
async fn absent_semantic_leaves_search_unregistered() {
    let inputs = LabConfigInputs {
        camera_index: ExplicitSetting::cli(Some(0)),
        ..LabConfigInputs::default()
    };
    let resolved = resolve_lab_config(&inputs).unwrap();
    assert!(
        resolve_semantic_indexing(&inputs, &resolved.video)
            .unwrap()
            .is_none()
    );
}

#[tokio::test]
#[allow(clippy::too_many_lines)]
async fn decision_append_keeps_chat_factory_single_invocation() {
    let recorder = RecordingServeAgentFactory::new();
    let legacy = ServeChatLegacyInputs {
        model_provider: ModelProvider::OpenAi,
        model: Some("chat-only".into()),
        bedrock_model: None,
    };
    let inputs = LabConfigInputs::default();
    let store = ConversationStore::open(
        &std::env::temp_dir().join(format!("semantic-serve-chat-{}", std::process::id())),
        4,
    )
    .await
    .unwrap();
    let hook = recorder.clone();
    let _ = build_serve_lab_agent(
        &inputs,
        &legacy,
        "http://127.0.0.1:9/mcp",
        store,
        move |provider, model, _url, store| {
            let hook = hook.clone();
            async move {
                hook.record(provider, &model)?;
                a2a_lab_ot2::LabAgent::from_provider(
                    provider,
                    &model,
                    "http://127.0.0.1:9/mcp",
                    store,
                )
                .await
            }
        },
        None,
    )
    .await;
    let (dir, store) = {
        let dir = temp_root("jev-decision");
        let store = Arc::new(
            a2a_lab_ot2::IndexedVideoDvr::open(dir.join("dvr"), 86_400)
                .await
                .unwrap(),
        );
        (dir, store)
    };
    let source = VideoSourceId::new("camera").unwrap();
    let capture = a2a_lab_ot2::time::now().unwrap();
    store
        .append_mjpeg_frame(&source, capture, 1_000_000, &[0xFF, 0xD8, 1, 0xD9])
        .await
        .unwrap();
    let (frame, _) = store
        .register_manifest_at_dvr_key(
            &a2a_lab_dev_kit::DvrStoreNamespace::new(DEFAULT_SEMANTIC_STORE_NAMESPACE).unwrap(),
            &source,
            &MemoryVideos::mjpeg_track_id(&source).unwrap(),
            capture,
        )
        .await
        .unwrap();
    let record = a2a_lab_dev_kit::FrameDecisionRecord::new(
        a2a_lab_dev_kit::DecisionIdempotencyKey::new("jev-fixture:tip").unwrap(),
        frame.clone(),
        LabPredicate::pipette_tip_attached_v1(),
        DecisionSubject::PipetteChannel(PipetteChannelSubject::new("left", "1", None).unwrap()),
        vec![
            a2a_lab_dev_kit::DecisionEvidenceItem::saved_frame_revision(
                frame.frame_id().clone(),
                a2a_lab_dev_kit::FrameRoiTransform::new([0.0, 0.0, 1.0, 1.0]).unwrap(),
            )
            .unwrap(),
        ],
        DecisionOutcome::Present,
        JevStructuredOutput::empty(),
        JevEvaluationProvenance::new(
            frame.captured_at(),
            frame.captured_at().saturating_add_micros(1_000_000),
            JevProducerIdentity::default_production(),
            JevEvaluationId::new("jev-fixture").unwrap(),
            Sha256Digest::hash_bytes(b"c"),
            Sha256Digest::hash_bytes(b"p"),
            Sha256Digest::hash_bytes(b"crit"),
            Sha256Digest::hash_bytes(b"t"),
            a2a_lab_dev_kit::FrameRoiTransform::new([0.0, 0.0, 1.0, 1.0]).unwrap(),
            Sha256Digest::hash_bytes(b"resp"),
            a2a_lab_dev_kit::DecisionEvaluationCoverage::Evaluated,
            Option::<&str>::None,
        )
        .unwrap(),
        None,
    )
    .unwrap();
    let ledger = Arc::new(
        DvrFrameDecisionLedger::authorized_producer(
            store,
            &FrameDecisionEvaluatorProviderConfig::pilot(
                Sha256Digest::hash_bytes(b"cfg"),
                Sha256Digest::hash_bytes(b"thr"),
            )
            .unwrap(),
            FrameDecisionEvaluatorLimits::pilot(),
        )
        .unwrap(),
    );
    let lab = A2aLabService::new(
        MemoryLogs::default(),
        MemoryMetrics::default(),
        MemoryTasks::default(),
    )
    .with_videos(MemoryVideos::default())
    .with_frame_decisions(ledger as Arc<dyn a2a_lab_dev_kit::FrameDecisionLedgerProvider>);
    lab.execute(A2aLabCommand::AppendFrameDecision(
        AppendFrameDecisionRequest::new(
            record,
            FrameDecisionEvaluationOwnership::new(JevEvaluationId::new("jev-fixture").unwrap()),
        )
        .unwrap(),
    ))
    .await
    .unwrap();
    assert!(recorder.take_recorded().unwrap().is_some());
    let _ = dir;
}
