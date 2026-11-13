//! Devkit DVR store boundary for OT2-1 (TASK-85).

use a2a_lab_dev_kit::{
    DvrFrameAtRequest, DvrMediaPayload, DvrMediaStore, MemoryVideos, VideoSourceId,
};
use a2a_lab_ot2::IndexedVideoDvr;

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
    .fetch_one(store.sqlite_pool())
    .await
    .unwrap();
    assert_eq!(media_tables, 4);
    let semantic_tables: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name IN
         ('semantic_frame_revisions', 'semantic_indexing_jobs', 'frame_decisions')",
    )
    .fetch_one(store.sqlite_pool())
    .await
    .unwrap();
    assert_eq!(semantic_tables, 3);
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
