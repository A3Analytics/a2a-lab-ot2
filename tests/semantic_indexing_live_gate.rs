//! Live Bedrock opt-in for semantic indexing and search (credential-free by default).

use std::collections::HashSet;

use a2a_lab_dev_kit::{A2aLabConfig, SemanticIndexingConfig};
use a2a_lab_ot2::IndexedVideoDvr;
use a2a_lab_ot2::lab_config::ResolvedSemanticIndexing;
use a2a_lab_ot2::videos::semantic_indexing::{
    SemanticServeInjections, build_deterministic_search_embedder, ensure_live_bedrock_ready,
    load_bedrock_sdk_config, prepare_semantic_serve_bundle,
};

const SEMANTIC_LIMITS: &str = r#""limits":{"worker":{"max_batch_items":4,"max_concurrency":2,"max_queue_backlog":64,"max_retry_attempts":3,"job_deadline":"30s","retry_backoff":"1s"},"search":{"top_k":10,"candidate_limit":1000,"max_neighbor_frames":5,"query_window":"2h"},"roi":{"min_edge_pixels":280,"max_edge_pixels":1568,"max_rois_per_frame":8},"storage":{"vector_index_path":"./state/semantic-index","max_disk_bytes":1073741824},"decisions":{"max_batch_items":8,"max_concurrency":2,"job_deadline":"45s","coverage_policy":"on_demand"}}"#;

fn semantic_config() -> SemanticIndexingConfig {
    let body = format!(
        r#"{{"schema_version":2,"semantic_indexing":{{"description":{{"provider":"bedrock","model":"custom-luna"}},"embeddings":{{"provider":"bedrock","model":"text-embed-v1","space":{{"dimensions":4,"normalization":"l2_unit","metric":"cosine"}}}},{SEMANTIC_LIMITS},"decisions":{{"provider":"jev","model":"jev-1.14.0"}}}}}}"#
    );
    A2aLabConfig::parse(&body)
        .unwrap()
        .semantic_indexing
        .unwrap()
}

fn resolved_semantic(config: SemanticIndexingConfig) -> ResolvedSemanticIndexing {
    ResolvedSemanticIndexing {
        config,
        vector_index_path: std::path::PathBuf::from("./state/semantic-index"),
        sqlite_path: std::path::PathBuf::from("./state/semantic.sqlite"),
        indexed_source_ids: HashSet::from(["camera".to_owned()]),
    }
}

async fn open_dvr(name: &str) -> (std::path::PathBuf, IndexedVideoDvr) {
    let dir = std::env::temp_dir().join(format!(
        "a2a-lab-ot2-semantic-live-{name}-{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    let store = IndexedVideoDvr::open(&dir, 86_400).await.unwrap();
    (dir, store)
}

fn isolate_aws_env_for_missing_prerequisites() {
    unsafe {
        for key in [
            "AWS_PROFILE",
            "AWS_REGION",
            "AWS_DEFAULT_REGION",
            "AWS_ACCESS_KEY_ID",
            "AWS_SECRET_ACCESS_KEY",
            "AWS_SESSION_TOKEN",
            "AWS_CONTAINER_CREDENTIALS_RELATIVE_URI",
            "AWS_WEB_IDENTITY_TOKEN_FILE",
        ] {
            std::env::remove_var(key);
        }
        std::env::set_var("AWS_CONFIG_FILE", "/nonexistent/a2a-lab-test-aws-config");
        std::env::set_var(
            "AWS_SHARED_CREDENTIALS_FILE",
            "/nonexistent/a2a-lab-test-aws-credentials",
        );
        std::env::set_var("AWS_SDK_LOAD_CONFIG", "0");
        std::env::set_var("AWS_EC2_METADATA_DISABLED", "true");
    }
}

fn with_static_aws_credentials() {
    isolate_aws_env_for_missing_prerequisites();
    unsafe {
        std::env::set_var("AWS_ACCESS_KEY_ID", "dummy-key-id");
        std::env::set_var("AWS_SECRET_ACCESS_KEY", "dummy-secret");
        std::env::set_var("AWS_REGION", "us-east-1");
        for candidate in ["/etc/ssl/cert.pem", "/etc/ssl/certs/ca-certificates.crt"] {
            if std::path::Path::new(candidate).is_file() {
                std::env::set_var("SSL_CERT_FILE", candidate);
                break;
            }
        }
    }
}

#[tokio::test]
async fn live_bedrock_semantic_indexing_gate_contract() {
    {
        unsafe { std::env::remove_var("A2ALAB_SEMANTIC_INDEXING_LIVE") };
        isolate_aws_env_for_missing_prerequisites();
        let semantic = resolved_semantic(semantic_config());
        let (_dir, dvr) = open_dvr("deterministic").await;
        let bundle =
            prepare_semantic_serve_bundle(&semantic, &dvr, &SemanticServeInjections::default())
                .await
                .expect("deterministic bundle");
        let workers = bundle.workers.expect("workers");
        assert!(!workers.description.live.uses_live_adapters());
        assert!(!workers.description.backfill.uses_live_adapters());
        let embedder = build_deterministic_search_embedder(&semantic.config)
            .await
            .expect("deterministic embedder");
        assert!(!embedder.uses_live_adapters());
    }

    {
        unsafe {
            std::env::set_var("A2ALAB_SEMANTIC_INDEXING_LIVE", "true");
        }
        with_static_aws_credentials();
        let semantic = resolved_semantic(semantic_config());
        let (_dir, dvr) = open_dvr("live").await;
        let bundle =
            prepare_semantic_serve_bundle(&semantic, &dvr, &SemanticServeInjections::default())
                .await
                .expect("live bundle");
        let workers = bundle.workers.expect("live workers");
        assert!(workers.description.live.uses_live_adapters());
        assert!(
            workers
                .embedding
                .as_ref()
                .expect("embedding workers")
                .live
                .uses_live_adapters()
        );
        assert!(bundle.search.is_some());
        unsafe {
            std::env::remove_var("A2ALAB_SEMANTIC_INDEXING_LIVE");
        }
    }

    {
        unsafe {
            std::env::set_var("A2ALAB_SEMANTIC_INDEXING_LIVE", "true");
        }
        isolate_aws_env_for_missing_prerequisites();
        let semantic = semantic_config();
        let description = a2a_lab_ot2::videos::semantic_indexing::description_provider_config(
            &semantic.description,
        )
        .unwrap();
        let embedding = a2a_lab_ot2::videos::semantic_indexing::embedding_provider_config(
            semantic.embeddings.as_ref().unwrap(),
        )
        .unwrap();
        let Err(region_error) = load_bedrock_sdk_config().await else {
            panic!("expected missing region/credentials");
        };
        let region_text = region_error.to_string();
        assert!(
            region_text.contains("AWS_REGION") || region_text.contains("credentials"),
            "{region_text}"
        );
        assert!(!region_text.contains("dummy-secret"));

        let Err(ready_error) = ensure_live_bedrock_ready(&description, &embedding).await else {
            panic!("expected startup validation failure");
        };
        let ready_text = ready_error.to_string();
        assert!(ready_text.contains("AWS") || ready_text.contains("credentials"));
        unsafe {
            std::env::remove_var("A2ALAB_SEMANTIC_INDEXING_LIVE");
        }
    }
}
