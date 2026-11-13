//! Devkit DVR store boundary for OT2-1 (TASK-85) and shared-pool semantic persistence (TASK-93).

use a2a_lab_dev_kit::{
    DvrFrameAtRequest, DvrMediaPayload, DvrMediaStore, EmbeddingMetric, FrameDescriptionProvenance,
    FrameEmbeddingVector, MemoryVideos, ModelSpaceIdentity, ProvenanceGeneration, Sha256Digest,
    TextEmbeddingProvenance, VectorNormalization, VideoSourceId,
};
use a2a_lab_ot2::IndexedVideoDvr;
use a2a_lab_ot2::videos::dvr::{
    IndexingCheckpoint, ListManifestFramesRequest, ManifestPageRequest,
};

fn temp_root(name: &str) -> std::path::PathBuf {
    std::env::temp_dir().join(format!(
        "a2a-lab-ot2-dvr-boundary-{name}-{}",
        std::process::id()
    ))
}

fn jpeg() -> Vec<u8> {
    vec![0xFF, 0xD8, 0xFF, 0xD9]
}

#[tokio::test]
async fn devkit_schema_without_duplicate_ot2_media_migrations() {
    let dir = temp_root("schema-boundary");
    let _ = std::fs::remove_dir_all(&dir);
    let store = IndexedVideoDvr::open(&dir, 86_400).await.unwrap();
    let media_tables: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name IN
         ('dvr_frames', 'dvr_chunks', 'dvr_tracks', 'dvr_h264_epochs')",
    )
    .fetch_one(store.media_pool())
    .await
    .unwrap();
    assert_eq!(media_tables, 4);
    let semantic_on_media: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name IN
         ('semantic_frame_revisions', 'semantic_indexing_jobs', 'frame_decisions')",
    )
    .fetch_one(store.media_pool())
    .await
    .unwrap();
    assert_eq!(semantic_on_media, 0);
    let semantic_tables: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name IN
         ('semantic_frame_revisions', 'semantic_indexing_jobs', 'frame_decisions')",
    )
    .fetch_one(store.semantic_pool())
    .await
    .unwrap();
    assert_eq!(semantic_tables, 3);
    let mut held = store.semantic_pool().begin().await.unwrap();
    sqlx::query("SELECT 1").execute(&mut *held).await.unwrap();
    let media_count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM dvr_frames")
        .fetch_one(store.media_pool())
        .await
        .unwrap();
    assert_eq!(media_count, 0);
    held.commit().await.unwrap();
    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn chunk_media_survives_reopen_and_semantic_jobs_follow() {
    let dir = temp_root("reopen");
    let _ = std::fs::remove_dir_all(&dir);
    let source = VideoSourceId::new("camera").unwrap();
    let capture = a2a_lab_ot2::time::now().unwrap();
    {
        let store = IndexedVideoDvr::open(&dir, 86_400).await.unwrap();
        store
            .append_mjpeg_frame(&source, capture, 1_000_000, &jpeg())
            .await
            .unwrap();
        assert_eq!(store.count_indexing_jobs().await.unwrap(), 1);
    }
    let store = IndexedVideoDvr::open(&dir, 86_400).await.unwrap();
    let track = MemoryVideos::mjpeg_track_id(&source).unwrap();
    let frame = store
        .frame_at(DvrFrameAtRequest::new(source.clone(), track, capture, None))
        .await
        .unwrap();
    assert!(matches!(
        frame.payload(),
        DvrMediaPayload::MjpegFrame(bytes) if bytes == &jpeg()
    ));
    assert_eq!(store.count_indexing_jobs().await.unwrap(), 1);
    let _ = std::fs::remove_dir_all(&dir);
}

fn sample_description(text: &str) -> FrameDescriptionProvenance {
    FrameDescriptionProvenance::new(
        "bedrock",
        "global.openai.gpt-5.6-luna",
        Sha256Digest::hash_bytes(b"prompt-v1"),
        Sha256Digest::hash_bytes(b"preprocess-v1"),
        None,
        Sha256Digest::hash_bytes(text.as_bytes()),
        ProvenanceGeneration::new(1).unwrap(),
    )
    .unwrap()
}

fn sample_embedding(text: &str) -> TextEmbeddingProvenance {
    TextEmbeddingProvenance::new(
        "bedrock",
        "text-embed-v1",
        4,
        VectorNormalization::L2Unit,
        EmbeddingMetric::Cosine,
        ProvenanceGeneration::new(1).unwrap(),
        Sha256Digest::hash_bytes(text.as_bytes()),
    )
    .unwrap()
}

fn sample_model_space() -> ModelSpaceIdentity {
    ModelSpaceIdentity::new(
        "bedrock",
        "text-embed-v1",
        4,
        VectorNormalization::L2Unit,
        EmbeddingMetric::Cosine,
        ProvenanceGeneration::new(1).unwrap(),
    )
    .unwrap()
}

fn sample_vector() -> FrameEmbeddingVector {
    FrameEmbeddingVector::new(vec![0.6, 0.0, 0.8, 0.0], 4).unwrap()
}

#[tokio::test]
async fn semantic_migrations_idempotent_through_repeated_reopen() {
    let dir = temp_root("migrate-idempotent");
    let _ = std::fs::remove_dir_all(&dir);
    let source = VideoSourceId::new("camera").unwrap();
    let capture = a2a_lab_ot2::time::now().unwrap();
    let track = MemoryVideos::mjpeg_track_id(&source).unwrap();
    let text = "plate centered";
    let vector = sample_vector();
    let frame_id = {
        let store = IndexedVideoDvr::open(&dir, 86_400).await.unwrap();
        store
            .append_mjpeg_frame(&source, capture, 1_000_000, &jpeg())
            .await
            .unwrap();
        let request = ListManifestFramesRequest::new(
            source.clone(),
            track.clone(),
            None,
            true,
            ManifestPageRequest::new(None, 4).unwrap(),
        )
        .unwrap();
        let page = store.list_manifest_frames(&request).await.unwrap();
        let frame = page.items()[0].frame().clone();
        let frame_id = frame.frame_id().as_str().to_owned();
        store
            .upsert_frame_description(&frame, text, &sample_description(text))
            .await
            .unwrap();
        store
            .upsert_frame_embedding(
                &frame,
                &sample_model_space(),
                &sample_embedding(text),
                &vector,
                IndexingCheckpoint::Ready,
            )
            .await
            .unwrap();
        frame_id
    };
    for _ in 0..2 {
        let store = IndexedVideoDvr::open(&dir, 86_400).await.unwrap();
        let priority_exists: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM pragma_table_info('semantic_indexing_jobs')
             WHERE name = 'enqueue_priority'",
        )
        .fetch_one(store.semantic_pool())
        .await
        .unwrap();
        assert_eq!(priority_exists, 1);
        let generation_exists: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM pragma_table_info('semantic_embedding_config_identity')
             WHERE name = 'model_space_generation'",
        )
        .fetch_one(store.semantic_pool())
        .await
        .unwrap();
        assert_eq!(generation_exists, 1);
        let record = store.get_manifest_frame(&frame_id).await.unwrap();
        assert_eq!(record.embedding().and_then(|e| e.vector()), Some(&vector));
        assert_eq!(store.count_indexing_jobs().await.unwrap(), 1);
    }
    let _ = std::fs::remove_dir_all(&dir);
}
