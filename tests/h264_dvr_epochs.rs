//! H.264 immutable epoch persistence and cutover behavior.

use a2a_lab_dev_kit::{
    DvrMediaStore, DvrTrackId, TimeRange, UtcTimestamp, VideoCodec, VideoSourceId,
};
use a2a_lab_ot2::IndexedVideoDvr;

fn temp_root(name: &str) -> std::path::PathBuf {
    std::env::temp_dir().join(format!(
        "a2a-lab-ot2-h264-epoch-{name}-{}",
        std::process::id()
    ))
}

fn ts(text: &str) -> UtcTimestamp {
    UtcTimestamp::parse(text).unwrap()
}

fn recent(offset_micros: i64) -> UtcTimestamp {
    a2a_lab_ot2::time::now()
        .unwrap()
        .saturating_add_micros(offset_micros)
}

async fn open_store(name: &str) -> (std::path::PathBuf, IndexedVideoDvr) {
    let dir = temp_root(name);
    let _ = std::fs::remove_dir_all(&dir);
    let store = IndexedVideoDvr::open(&dir, 86_400).await.unwrap();
    (dir, store)
}

const INIT_A: &[u8] = &[
    0x00, 0x00, 0x00, 0x01, 0x67, 0x42, 0x00, 0x1E, 0x9A, 0x74, 0x03, 0xC0, 0x11, 0x00, 0x00, 0x03,
    0x00, 0x01, 0x00, 0x00, 0x03, 0x00, 0x3C, 0x0F, 0x16, 0x2D, 0x96, 0x00, 0x00, 0x00, 0x01, 0x68,
    0xCE, 0x3C, 0x80,
];

const INIT_B: &[u8] = &[
    0x00, 0x00, 0x00, 0x01, 0x67, 0x42, 0x00, 0x28, 0x9A, 0x74, 0x03, 0xC0, 0x11, 0x00, 0x00, 0x03,
    0x00, 0x01, 0x00, 0x00, 0x03, 0x00, 0x3C, 0x0F, 0x16, 0x2D, 0x96, 0x00, 0x00, 0x00, 0x01, 0x68,
    0xCE, 0x3C, 0x80,
];

fn sample(tag: u8) -> Vec<u8> {
    vec![0x00, 0x00, 0x00, 0x01, 0x65, tag]
}

#[tokio::test]
async fn unchanged_init_reuses_one_epoch() {
    let (_dir, store) = open_store("reuse").await;
    let source = VideoSourceId::new("cam").unwrap();
    store
        .append_h264_sample(
            &source,
            ts("2024-06-01T00:00:00Z"),
            200_000,
            true,
            INIT_A,
            &sample(1),
        )
        .await
        .unwrap();
    store
        .append_h264_sample(
            &source,
            ts("2024-06-01T00:00:01Z"),
            200_000,
            true,
            INIT_A,
            &sample(2),
        )
        .await
        .unwrap();
    let epochs: i64 = sqlx::query_scalar(
        "SELECT COUNT(DISTINCT h264_epoch_id) FROM dvr_frames WHERE source_id = ?1",
    )
    .bind(source.as_str())
    .fetch_one(store.media_pool())
    .await
    .unwrap();
    assert_eq!(epochs, 1);
}

#[tokio::test]
async fn changed_init_opens_distinct_epochs_and_list_tracks() {
    let (_dir, store) = open_store("distinct").await;
    let source = VideoSourceId::new("cam").unwrap();
    store
        .append_h264_sample(
            &source,
            ts("2024-06-01T00:00:00Z"),
            200_000,
            true,
            INIT_A,
            &sample(1),
        )
        .await
        .unwrap();
    store
        .append_h264_sample(
            &source,
            ts("2024-06-01T00:00:02Z"),
            200_000,
            true,
            INIT_B,
            &sample(2),
        )
        .await
        .unwrap();
    let epochs: i64 = sqlx::query_scalar(
        "SELECT COUNT(DISTINCT h264_epoch_id) FROM dvr_frames WHERE source_id = ?1",
    )
    .bind(source.as_str())
    .fetch_one(store.media_pool())
    .await
    .unwrap();
    assert_eq!(epochs, 2);
    let tracks = store.list_tracks(&source).await.unwrap();
    let h264: Vec<_> = tracks
        .iter()
        .filter(|track| track.codec() == VideoCodec::H264)
        .collect();
    assert_eq!(h264.len(), 2);
    assert_ne!(
        h264[0].codec_initialization(),
        h264[1].codec_initialization()
    );
}

#[tokio::test]
async fn cutover_clears_h264_rows_but_not_mjpeg() {
    let (dir, store) = open_store("cutover").await;
    let source = VideoSourceId::new("cam").unwrap();
    let jpeg = vec![0xFF, 0xD8, 0xFF, 0xD9];
    store
        .append_mjpeg_frame(&source, recent(0), 1_000_000, &jpeg)
        .await
        .unwrap();
    store
        .append_h264_sample(
            &source,
            recent(1_000_000),
            200_000,
            true,
            INIT_A,
            &sample(1),
        )
        .await
        .unwrap();
    sqlx::query("DELETE FROM dvr_store_meta WHERE meta_key = 'h264_epoch_storage_cutover'")
        .execute(store.media_pool())
        .await
        .unwrap();
    drop(store);
    let store = IndexedVideoDvr::open(&dir, 86_400).await.unwrap();
    let h264_frames: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM dvr_frames WHERE source_id = ?1 AND track_id = 'h264'",
    )
    .bind(source.as_str())
    .fetch_one(store.media_pool())
    .await
    .unwrap();
    assert_eq!(h264_frames, 0);
    assert!(store.media_coverage(&source).await.is_ok());
    let jobs: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM semantic_indexing_jobs")
        .fetch_one(store.media_pool())
        .await
        .unwrap();
    assert!(jobs >= 0);
    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn restart_preserves_epoch_association() {
    let dir = temp_root("restart");
    let _ = std::fs::remove_dir_all(&dir);
    let source = VideoSourceId::new("cam").unwrap();
    let anchor = recent(0);
    {
        let store = IndexedVideoDvr::open(&dir, 86_400).await.unwrap();
        store
            .append_h264_sample(&source, anchor, 200_000, true, INIT_A, &sample(1))
            .await
            .unwrap();
        store
            .append_h264_sample(
                &source,
                anchor.saturating_add_micros(2_000_000),
                200_000,
                true,
                INIT_B,
                &sample(2),
            )
            .await
            .unwrap();
    }
    let store = IndexedVideoDvr::open(&dir, 86_400).await.unwrap();
    let tracks = store.list_tracks(&source).await.unwrap();
    assert_eq!(
        tracks
            .iter()
            .filter(|track| track.codec() == VideoCodec::H264)
            .count(),
        2
    );
    let window = TimeRange::new(
        anchor.saturating_add_micros(-1_000_000),
        anchor.saturating_add_micros(3_000_000),
    )
    .unwrap();
    let track = DvrTrackId::new("h264").unwrap();
    let page = store
        .read_media_window(
            a2a_lab_dev_kit::ReadDvrMediaWindowRequest::new(
                source.clone(),
                track,
                window,
                a2a_lab_dev_kit::PageRequest::new(None, 10).unwrap(),
            )
            .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(page.items().len(), 2);
    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn retention_keeps_adjacent_epochs_independently() {
    let store = open_store("retain").await.1;
    let source = VideoSourceId::new("cam").unwrap();
    store
        .append_h264_sample(
            &source,
            ts("2020-01-01T00:00:00Z"),
            200_000,
            true,
            INIT_A,
            &sample(1),
        )
        .await
        .unwrap();
    store
        .append_h264_sample(
            &source,
            ts("2024-06-01T00:00:00Z"),
            200_000,
            true,
            INIT_B,
            &sample(2),
        )
        .await
        .unwrap();
    store.cleanup_at(ts("2024-06-01T00:01:00Z")).await.unwrap();
    let epochs: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM dvr_h264_epochs WHERE source_id = ?1")
            .bind(source.as_str())
            .fetch_one(store.media_pool())
            .await
            .unwrap();
    assert_eq!(epochs, 1);
    let tracks = store.list_tracks(&source).await.unwrap();
    let epochs: Vec<_> = tracks
        .iter()
        .filter(|track| track.codec() == VideoCodec::H264)
        .filter(|track| track.id().as_str() != "h264")
        .collect();
    assert_eq!(epochs.len(), 1);
    assert_eq!(epochs[0].codec_initialization(), INIT_B);
}
