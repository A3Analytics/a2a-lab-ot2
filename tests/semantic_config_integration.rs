//! End-to-end deterministic path from v2 semantic configuration to DVR search.

use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use a2a_lab_dev_kit::{
    A2aLabConfig, A2aLabError, ChatProvider, DvrStoreNamespace, EmbeddingMetric, ImageProvider,
    ListImageSourcesRequest, MemoryVideos, PageRequest, SearchSemanticFramesRequest,
    SemanticSchedule, SemanticSearchLimits, SemanticSearchQuery, SemanticSearchScope,
    SemanticVideoSearchProvider, TimeRange, VectorNormalization, VideoSourceId,
};
use a2a_lab_ot2::IndexedVideoDvr;
use a2a_lab_ot2::lab_config::ResolvedSemanticIndexing;
use a2a_lab_ot2::videos::dvr::{
    DEFAULT_SEMANTIC_STORE_NAMESPACE, DeterministicIndexingWorker, DvrSemanticSearchProvider,
    IndexingWorkerFaults,
};
use a2a_lab_ot2::videos::semantic_indexing::{
    SemanticServeInjections, build_deterministic_search_embedder, embedding_provider_config,
    prepare_semantic_serve_bundle,
};
use a2a_lab_ot2::{
    ExplicitSetting, ImageServeConfig, LabConfigInputs, image_catalog, resolve_lab_config,
    resolve_semantic_indexing,
};

const OPENTRONS_CAMERA_SOURCE_ID: &str = "opentrons-camera";

const SEMANTIC_LIMITS: &str = r#""limits":{"worker":{"max_batch_items":4,"max_concurrency":2,"max_queue_backlog":64,"max_retry_attempts":3,"job_deadline":"30s","retry_backoff":"1s"},"search":{"top_k":10,"candidate_limit":1000,"max_neighbor_frames":5,"query_window":"2h"},"roi":{"min_edge_pixels":280,"max_edge_pixels":1568,"max_rois_per_frame":8},"storage":{"vector_index_path":"./state/semantic-index","max_disk_bytes":1073741824},"decisions":{"max_batch_items":8,"max_concurrency":2,"job_deadline":"45s","coverage_policy":"on_demand"}}"#;

fn temp_root(name: &str) -> PathBuf {
    std::env::temp_dir().join(format!(
        "a2a-lab-ot2-semantic-integration-{name}-{}",
        std::process::id()
    ))
}

fn write_config(dir: &Path, body: &str) -> PathBuf {
    fs::create_dir_all(dir).unwrap();
    let path = dir.join("a2a-lab.json");
    fs::write(&path, body).unwrap();
    path
}

fn integration_config_document() -> String {
    format!(
        r#"{{"schema_version":2,"chat":{{"provider":"openai","model":"chat-only"}},"videos":{{"sources":[{{"id":"bench-a","device_index":0,"format":"mjpeg","semantic_indexing":{{"description":"5s","description_model":"luna-a","embeddings":"1m","embedding_model":"embed-a"}}}},{{"id":"bench-b","device_index":1,"format":"mjpeg","semantic_indexing":{{"description":"1.5s","description_model":"luna-b","embeddings":"5s","embedding_model":"embed-b"}}}}],"dvr":{{"storage_path":"./dvr","retention":"1h"}}}},"semantic_indexing":{{"description":{{"provider":"bedrock","model":"global.openai.gpt-5.6-luna"}},"embeddings":{{"provider":"bedrock","model":"text-embed-v1","space":{{"dimensions":4,"normalization":"l2_unit","metric":"cosine","generation":1}}}},{SEMANTIC_LIMITS},"decisions":{{"provider":"jev","model":"jev-1.14.0"}}}}}}"#
    )
}

fn inputs_for(path: PathBuf) -> LabConfigInputs {
    LabConfigInputs {
        config_path: ExplicitSetting::cli(Some(path)),
        ..LabConfigInputs::default()
    }
}

fn resolve_semantic_from_document(
    body: &str,
    dir: &Path,
) -> (LabConfigInputs, ResolvedSemanticIndexing) {
    let path = write_config(dir, body);
    let inputs = inputs_for(path);
    let resolved_video = resolve_lab_config(&inputs).unwrap().video;
    let semantic = resolve_semantic_indexing(&inputs, &resolved_video)
        .unwrap()
        .expect("semantic indexing");
    (inputs, semantic)
}

fn store_ns() -> DvrStoreNamespace {
    DvrStoreNamespace::new(DEFAULT_SEMANTIC_STORE_NAMESPACE).unwrap()
}

fn jpeg(tag: u8) -> Vec<u8> {
    vec![0xFF, 0xD8, tag, 0xD9]
}

fn isolate_aws_env() {
    unsafe {
        for key in [
            "AWS_PROFILE",
            "AWS_REGION",
            "AWS_DEFAULT_REGION",
            "AWS_ACCESS_KEY_ID",
            "AWS_SECRET_ACCESS_KEY",
            "AWS_SESSION_TOKEN",
            "AWS_EC2_METADATA_DISABLED",
            "AWS_SDK_LOAD_CONFIG",
            "A2ALAB_SEMANTIC_INDEXING_LIVE",
        ] {
            std::env::remove_var(key);
        }
        std::env::set_var("AWS_EC2_METADATA_DISABLED", "true");
        std::env::set_var("AWS_SDK_LOAD_CONFIG", "0");
    }
}

#[test]
#[allow(clippy::cognitive_complexity)]
fn complete_v2_configuration_resolves_distinct_source_schedules() {
    isolate_aws_env();
    let dir = temp_root("resolve");
    let (_, semantic) = resolve_semantic_from_document(&integration_config_document(), &dir);
    assert_eq!(
        semantic.indexed_source_ids,
        HashSet::from(["bench-a".to_owned(), "bench-b".to_owned()])
    );
    let schedule_a = semantic.config.schedule_for("bench-a");
    let schedule_b = semantic.config.schedule_for("bench-b");
    assert_eq!(
        schedule_a.description(),
        SemanticSchedule::Interval { micros: 5_000_000 }
    );
    assert_eq!(
        schedule_b.description(),
        SemanticSchedule::Interval { micros: 1_500_000 }
    );
    assert_eq!(schedule_a.description_model(), Some("luna-a"));
    assert_eq!(schedule_b.description_model(), Some("luna-b"));
    assert_eq!(schedule_a.embedding_model(), Some("embed-a"));
    assert_eq!(schedule_b.embedding_model(), Some("embed-b"));

    let embedding = semantic.config.embeddings.as_ref().unwrap();
    assert_eq!(embedding.provider(), "bedrock");
    assert_eq!(embedding.model(), "text-embed-v1");
    assert_eq!(embedding.space().dimensions, 4);
    assert_eq!(embedding.space().normalization, VectorNormalization::L2Unit);
    assert_eq!(embedding.space().metric, EmbeddingMetric::Cosine);

    let loaded = A2aLabConfig::parse(&integration_config_document()).unwrap();
    assert_eq!(
        loaded.chat.as_ref().unwrap().provider,
        Some(ChatProvider::Openai)
    );
    assert_eq!(
        loaded.chat.as_ref().unwrap().model.as_deref(),
        Some("chat-only")
    );

    let path = dir.join("a2a-lab.json");
    let output = Command::new(env!("CARGO_BIN_EXE_a2a-lab-ot2"))
        .env_remove("OPENAI_API_KEY")
        .env_remove("ANTHROPIC_API_KEY")
        .env_remove("AWS_ACCESS_KEY_ID")
        .env_remove("AWS_SECRET_ACCESS_KEY")
        .env_remove("AWS_PROFILE")
        .env("A2ALAB_OPENTRONS_URL", "http://127.0.0.1:9")
        .args(["config", "validate", "--config"])
        .arg(&path)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let _ = fs::remove_dir_all(dir);
}

#[test]
fn bedrock_embedding_validation_accepts_compatible_space_and_rejects_invalid() {
    isolate_aws_env();
    let base = integration_config_document();
    let accepted = base
        .replace(
            "\"model\":\"text-embed-v1\"",
            "\"model\":\"amazon.titan-embed-text-v1\"",
        )
        .replace("\"dimensions\":4", "\"dimensions\":1536");
    let parsed = A2aLabConfig::parse(&accepted).unwrap();
    assert_eq!(
        parsed.chat.as_ref().unwrap().model.as_deref(),
        Some("chat-only")
    );
    let embeddings = parsed
        .semantic_indexing
        .as_ref()
        .unwrap()
        .embeddings
        .as_ref()
        .unwrap();
    let provider = embedding_provider_config(embeddings).unwrap();
    assert_eq!(provider.model(), "amazon.titan-embed-text-v1");
    assert_eq!(provider.model_space().dimensions(), 1536);
    let rejected = format!(
        r#"{{"schema_version":2,"semantic_indexing":{{"description":{{"provider":"bedrock","model":"global.openai.gpt-5.6-luna"}},"embeddings":{{"provider":"bedrock","model":"text-embed-v1","space":{{"dimensions":0,"normalization":"l2_unit","metric":"cosine","generation":1}}}},{SEMANTIC_LIMITS},"decisions":{{"provider":"jev","model":"jev-1.14.0"}}}}}}"#
    );
    let err = A2aLabConfig::parse(&rejected).unwrap_err().to_string();
    assert!(err.contains("dimensions"), "{err}");

    let local_bge_doc = integration_config_document().replace(
        "\"provider\":\"bedrock\",\"model\":\"text-embed-v1\"",
        "\"provider\":\"local_bge\",\"model\":\"wrong-model\"",
    );
    let local_bge = A2aLabConfig::parse(&local_bge_doc)
        .unwrap()
        .semantic_indexing
        .unwrap()
        .embeddings
        .unwrap();
    let field_err = embedding_provider_config(&local_bge).unwrap_err();
    assert!(
        matches!(field_err, A2aLabError::Invalid { field, .. } if field == "semantic_indexing.embeddings.space")
    );

    let empty_model =
        integration_config_document().replace("\"model\":\"text-embed-v1\"", "\"model\":\"\"");
    let model_err = A2aLabConfig::parse(&empty_model).unwrap_err();
    assert!(matches!(
        model_err,
        A2aLabError::Invalid { field, .. } if field == "semantic_indexing.embeddings.model"
    ));
}

#[test]
fn per_source_schedule_changes_do_not_mutate_other_sources() {
    let dir = temp_root("schedule-mutate");
    let (_, semantic) = resolve_semantic_from_document(&integration_config_document(), &dir);
    let mut edited =
        integration_config_document().replace("\"description\":\"5s\"", "\"description\":\"10s\"");
    edited = edited.replace(
        "\"description_model\":\"luna-a\"",
        "\"description_model\":\"luna-a-2\"",
    );
    let (_, updated) = resolve_semantic_from_document(&edited, &dir);
    let before_b = semantic.config.schedule_for("bench-b");
    let after_b = updated.config.schedule_for("bench-b");
    assert_eq!(before_b, after_b);
    assert_eq!(
        updated.config.schedule_for("bench-a").description(),
        SemanticSchedule::Interval { micros: 10_000_000 }
    );
    assert_eq!(
        updated.config.schedule_for("bench-a").description_model(),
        Some("luna-a-2")
    );
    assert_eq!(
        semantic.config.description.model(),
        updated.config.description.model()
    );
    let _ = fs::remove_dir_all(dir);
}

#[tokio::test]
async fn source_schedules_gate_eligibility_independently() {
    let dir = temp_root("schedule-gate");
    let store = IndexedVideoDvr::open(&dir, 86_400).await.unwrap();
    let track = MemoryVideos::mjpeg_track_id(&VideoSourceId::new("bench-a").unwrap()).unwrap();
    let five_sec = SemanticSchedule::Interval { micros: 5_000_000 };
    let one_point_five = SemanticSchedule::Interval { micros: 1_500_000 };
    assert!(
        store
            .select_semantic_stage_frame("bench-a", track.as_str(), "description", 0, five_sec)
            .await
            .unwrap()
    );
    assert!(
        !store
            .select_semantic_stage_frame(
                "bench-a",
                track.as_str(),
                "description",
                1_000_000,
                five_sec
            )
            .await
            .unwrap()
    );
    assert!(
        store
            .select_semantic_stage_frame(
                "bench-a",
                track.as_str(),
                "description",
                5_000_000,
                five_sec
            )
            .await
            .unwrap()
    );
    let track_b = MemoryVideos::mjpeg_track_id(&VideoSourceId::new("bench-b").unwrap()).unwrap();
    assert!(
        store
            .select_semantic_stage_frame(
                "bench-b",
                track_b.as_str(),
                "description",
                0,
                one_point_five
            )
            .await
            .unwrap()
    );
    assert!(
        !store
            .select_semantic_stage_frame(
                "bench-b",
                track_b.as_str(),
                "description",
                1_000_000,
                one_point_five
            )
            .await
            .unwrap()
    );
    assert!(
        store
            .select_semantic_stage_frame(
                "bench-b",
                track_b.as_str(),
                "description",
                1_500_000,
                one_point_five
            )
            .await
            .unwrap()
    );
    let _ = fs::remove_dir_all(dir);
}

#[tokio::test]
async fn deterministic_semantic_path_indexes_persisted_vectors_and_searches_after_reopen() {
    isolate_aws_env();
    let dir = temp_root("persist-search");
    let dvr_dir = dir.join("dvr-store");
    let mut body = integration_config_document();
    body = body.replace(
        "\"storage_path\":\"./dvr\"",
        &format!("\"storage_path\":\"{}\"", dvr_dir.to_string_lossy()),
    );
    let (_, semantic) = resolve_semantic_from_document(&body, &dir);
    let source = VideoSourceId::new("bench-a").unwrap();
    let capture = a2a_lab_ot2::time::now().unwrap();
    let frame_id = {
        let store = IndexedVideoDvr::open(&dvr_dir, 86_400).await.unwrap();
        store
            .append_mjpeg_frame(&source, capture, 1_000_000, &jpeg(31))
            .await
            .unwrap();
        let worker = DeterministicIndexingWorker::deterministic(
            &semantic.config,
            "integration-worker",
            IndexingWorkerFaults::new(),
        )
        .await
        .unwrap();
        let report = worker.run_batch(&store, &semantic.config).await.unwrap();
        assert_eq!(report.embeddings_committed(), 1);
        let track = MemoryVideos::mjpeg_track_id(&source).unwrap();
        let (frame, _, _) = store
            .ensure_indexing_at_dvr_key(&store_ns(), &source, &track, capture)
            .await
            .unwrap();
        let manifest = store
            .get_manifest_frame(frame.frame_id().as_str())
            .await
            .unwrap();
        assert!(manifest.embedding().and_then(|row| row.vector()).is_some());
        frame.frame_id().as_str().to_owned()
    };

    let store = IndexedVideoDvr::open(&dvr_dir, 86_400).await.unwrap();
    let manifest = store.get_manifest_frame(&frame_id).await.unwrap();
    let description = manifest
        .description()
        .expect("persisted description")
        .text()
        .to_owned();
    let embedder = build_deterministic_search_embedder(&semantic.config)
        .await
        .unwrap();
    assert!(!embedder.uses_live_adapters());
    let provider = DvrSemanticSearchProvider::new(store, &semantic, embedder).unwrap();
    let end = capture.saturating_add_micros(10_000_000);
    let scope =
        SemanticSearchScope::new(source.clone(), Some(TimeRange::new(capture, end).unwrap()))
            .unwrap();
    let selector = semantic.config.active_search_provenance().unwrap();
    let request = SearchSemanticFramesRequest::new(
        scope,
        SemanticSearchQuery::Text {
            text: description.clone(),
        },
        selector,
        SemanticSearchLimits::new(5, 100, 3).unwrap(),
        PageRequest::new(None, 5).unwrap(),
        None,
    )
    .unwrap();
    let result = provider.search_semantic_frames(request).await.unwrap();
    assert_eq!(result.hits().items().len(), 1);
    assert_eq!(
        result.hits().items()[0].generated_description(),
        description
    );
    assert!(matches!(
        result.hits().items()[0].media_availability(),
        a2a_lab_dev_kit::SemanticFrameMediaAvailability::Ready
    ));
    let _ = fs::remove_dir_all(dir);
}

#[tokio::test]
async fn semantic_search_scopes_results_to_configured_sources() {
    isolate_aws_env();
    let dir = temp_root("scope");
    let dvr_dir = dir.join("dvr-store");
    let mut body = integration_config_document();
    body = body.replace(
        "\"storage_path\":\"./dvr\"",
        &format!("\"storage_path\":\"{}\"", dvr_dir.to_string_lossy()),
    );
    let (_, semantic) = resolve_semantic_from_document(&body, &dir);
    let source_a = VideoSourceId::new("bench-a").unwrap();
    let source_b = VideoSourceId::new("bench-b").unwrap();
    let capture = a2a_lab_ot2::time::now().unwrap();
    {
        let store = IndexedVideoDvr::open(&dvr_dir, 86_400).await.unwrap();
        for (source, tag) in [(&source_a, 41_u8), (&source_b, 42_u8)] {
            store
                .append_mjpeg_frame(
                    source,
                    capture.saturating_add_micros(i64::from(tag) * 1_000_000),
                    1_000_000,
                    &jpeg(tag),
                )
                .await
                .unwrap();
        }
        let worker = DeterministicIndexingWorker::deterministic(
            &semantic.config,
            "scope-worker",
            IndexingWorkerFaults::new(),
        )
        .await
        .unwrap();
        worker.run_batch(&store, &semantic.config).await.unwrap();
    }
    let store = IndexedVideoDvr::open(&dvr_dir, 86_400).await.unwrap();
    let embedder = build_deterministic_search_embedder(&semantic.config)
        .await
        .unwrap();
    let provider = DvrSemanticSearchProvider::new(store, &semantic, embedder).unwrap();
    let end = capture.saturating_add_micros(120_000_000);
    let scope_a = SemanticSearchScope::new(
        source_a.clone(),
        Some(TimeRange::new(capture, end).unwrap()),
    )
    .unwrap();
    let selector = semantic.config.active_search_provenance().unwrap();
    let request = SearchSemanticFramesRequest::new(
        scope_a,
        SemanticSearchQuery::Text {
            text: "desc".to_owned(),
        },
        selector.clone(),
        SemanticSearchLimits::new(10, 100, 3).unwrap(),
        PageRequest::new(None, 10).unwrap(),
        None,
    )
    .unwrap();
    let hits = provider.search_semantic_frames(request).await.unwrap();
    assert!(!hits.hits().items().is_empty());
    assert!(
        hits.hits()
            .items()
            .iter()
            .all(|hit| hit.frame().source_id() == &source_a)
    );

    let foreign = VideoSourceId::new("unconfigured-source").unwrap();
    let err = provider
        .search_semantic_frames(
            SearchSemanticFramesRequest::new(
                SemanticSearchScope::new(foreign, Some(TimeRange::new(capture, end).unwrap()))
                    .unwrap(),
                SemanticSearchQuery::Text {
                    text: "desc".to_owned(),
                },
                selector,
                SemanticSearchLimits::pilot(),
                PageRequest::new(None, 10).unwrap(),
                None,
            )
            .unwrap(),
        )
        .await
        .unwrap_err();
    assert!(err.to_string().contains("semantic indexing scope"));
    let _ = fs::remove_dir_all(dir);
}

#[tokio::test]
async fn configured_video_sources_expose_matching_image_catalog_entries() {
    isolate_aws_env();
    let dir = temp_root("image-catalog");
    let (_, semantic) = resolve_semantic_from_document(&integration_config_document(), &dir);
    let resolved = resolve_lab_config(&inputs_for(dir.join("a2a-lab.json"))).unwrap();
    let image_config = ImageServeConfig::new(None, None, 64 * 1024, 10_000, 30)
        .unwrap()
        .with_native_video_sources(&resolved.video.sources);
    let catalog = image_catalog("http://127.0.0.1:31950", &image_config, None, None, None).unwrap();
    let page = catalog
        .list_image_sources(
            ListImageSourcesRequest::new(PageRequest::new(None, 20).unwrap()).unwrap(),
        )
        .await
        .unwrap();
    let ids: HashSet<_> = page.items().iter().map(|item| item.id.as_str()).collect();
    assert!(ids.contains("bench-a"));
    assert!(ids.contains("bench-b"));
    assert!(ids.contains(OPENTRONS_CAMERA_SOURCE_ID));
    let opentrons = page
        .items()
        .iter()
        .find(|item| item.id.as_str() == OPENTRONS_CAMERA_SOURCE_ID)
        .unwrap();
    assert!(opentrons.semantic_id.is_none());
    assert!(
        resolved
            .video
            .sources
            .iter()
            .all(|source| source.id != OPENTRONS_CAMERA_SOURCE_ID)
    );
    assert!(semantic.indexed_source_ids.contains("bench-a"));
    assert!(semantic.indexed_source_ids.contains("bench-b"));
    assert!(
        !semantic
            .indexed_source_ids
            .contains(OPENTRONS_CAMERA_SOURCE_ID)
    );
    let _ = fs::remove_dir_all(dir);
}

#[tokio::test]
async fn live_semantic_gate_stays_deterministic_without_credentials() {
    isolate_aws_env();
    let semantic =
        resolve_semantic_from_document(&integration_config_document(), &temp_root("live")).1;
    let dvr_dir = temp_root("live-dvr");
    let store = IndexedVideoDvr::open(&dvr_dir, 86_400).await.unwrap();
    let bundle =
        prepare_semantic_serve_bundle(&semantic, &store, &SemanticServeInjections::default())
            .await
            .expect("deterministic bundle");
    let workers = bundle.workers.expect("workers");
    assert!(!workers.description.live.uses_live_adapters());
    assert!(!workers.description.backfill.uses_live_adapters());
    let embedder = build_deterministic_search_embedder(&semantic.config)
        .await
        .expect("deterministic embedder");
    assert!(!embedder.uses_live_adapters());
    assert!(bundle.search.is_some());
    let _ = fs::remove_dir_all(dvr_dir);
}
