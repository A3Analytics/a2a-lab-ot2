//! Jev decision worker factory gates for serve wiring.

use a2a_lab_dev_kit::{A2aLabConfig, SemanticIndexingConfig};

use a2a_lab_ot2::videos::semantic_indexing::{
    build_deterministic_decision_worker, jev_live_worker_enabled,
};

const SEMANTIC_LIMITS: &str = r#""limits":{"worker":{"max_batch_items":4,"max_concurrency":2,"max_queue_backlog":64,"max_retry_attempts":3,"job_deadline":"30s","retry_backoff":"1s"},"search":{"top_k":10,"candidate_limit":1000,"max_neighbor_frames":5,"query_window":"2h"},"roi":{"min_edge_pixels":280,"max_edge_pixels":1568,"max_rois_per_frame":8},"storage":{"vector_index_path":"./state/semantic-index","max_disk_bytes":1073741824},"decisions":{"max_batch_items":8,"max_concurrency":2,"job_deadline":"45s","coverage_policy":"on_demand"}}"#;

fn semantic_config() -> SemanticIndexingConfig {
    let document = format!(
        r#"{{"schema_version":2,"semantic_indexing":{{"description":{{"provider":"bedrock"}},"embeddings":{{"provider":"bedrock","model":"text-embed-v1","space":{{"dimensions":4,"normalization":"l2_unit","metric":"cosine"}}}},{SEMANTIC_LIMITS},"decisions":{{"provider":"jev","model":"jev-1.14.0","credentials":{{"env":"TYPESAFE_API_KEY"}}}}}}}}"#
    );
    A2aLabConfig::parse(&document)
        .unwrap()
        .semantic_indexing
        .unwrap()
}

#[test]
fn live_worker_disabled_without_env_opt_in() {
    let semantic = semantic_config();
    assert!(!jev_live_worker_enabled(&semantic));
    let _worker = build_deterministic_decision_worker(&semantic);
}
