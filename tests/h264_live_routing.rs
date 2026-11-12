//! Live H.264 follow routes through the in-memory store, not disk DVR.

use std::sync::Arc;
use std::time::Duration;

use a2a_lab_dev_kit::{
    DvrEnqueueStatus, DvrPersistenceQueue, DvrPictureProperties, DvrTrackId, GetVideoClipRequest,
    HLS_PLAYLIST_RELATIVE_PATH, MemoryVideos, ResolvedMediaAsset, TimeRange, UtcTimestamp,
    VideoCodec, VideoProvider, VideoSource, VideoSourceId,
};

use a2a_lab_ot2::IndexedVideoDvr;
use a2a_lab_ot2::videos::dvr::VideoDvrCatalog;

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

fn temp_root(name: &str) -> std::path::PathBuf {
    std::env::temp_dir().join(format!(
        "a2a-lab-ot2-h264-live-routing-{name}-{}",
        std::process::id()
    ))
}

fn ts(text: &str) -> UtcTimestamp {
    UtcTimestamp::parse(text).unwrap()
}

fn sample(tag: u8) -> Vec<u8> {
    vec![0x00, 0x00, 0x00, 0x01, 0x65, tag]
}

fn track(label: &str) -> DvrTrackId {
    DvrTrackId::new(label).unwrap()
}

async fn catalog(name: &str) -> (VideoDvrCatalog, VideoSourceId) {
    let dir = temp_root(name);
    let _ = std::fs::remove_dir_all(&dir);
    let store = IndexedVideoDvr::open(&dir, 86_400).await.unwrap();
    let live = MemoryVideos::new();
    let source = VideoSourceId::new("cam").unwrap();
    live.insert_source(
        VideoSource::with_capabilities(
            source.clone(),
            "H264",
            "encoded",
            None,
            None,
            640,
            480,
            5,
            "video/mp4",
            vec![VideoCodec::H264],
        )
        .unwrap(),
    )
    .await
    .unwrap();
    live.set_live_available(&source, true).await.unwrap();
    let catalog = VideoDvrCatalog::open(live, store, Duration::from_secs(60)).unwrap();
    (catalog, source)
}

fn seed_live(
    catalog: &VideoDvrCatalog,
    source: &VideoSourceId,
    track_id: &DvrTrackId,
    init: &[u8],
    at: UtcTimestamp,
    bytes: Vec<u8>,
) {
    let picture = DvrPictureProperties::new(640, 480).unwrap();
    let entry = catalog.live_h264().get(source).unwrap_or_else(|| {
        catalog
            .live_h264()
            .register(source, 60, 5, 1_000_000)
            .unwrap()
    });
    let store = entry.inner();
    if store
        .snapshot()
        .unwrap()
        .epochs()
        .iter()
        .all(|epoch| epoch.track_id() != track_id)
    {
        store
            .open_epoch(track_id.clone(), init.to_vec(), picture)
            .unwrap();
    }
    store.append_sample(track_id, at, bytes, true).unwrap();
}

#[tokio::test]
async fn live_follow_uses_memory_store_without_disk_epochs() {
    let (catalog, source) = catalog("live-no-disk").await;
    seed_live(
        &catalog,
        &source,
        &track("epoch-live"),
        INIT_B,
        ts("2024-06-01T00:00:03Z"),
        sample(9),
    );
    let follow = catalog
        .follow_video(a2a_lab_dev_kit::FollowVideoRequest::new(source.clone()))
        .await
        .unwrap();
    let asset = catalog
        .resolve_live_playback_asset(follow.id(), Some(HLS_PLAYLIST_RELATIVE_PATH))
        .await
        .unwrap();
    let ResolvedMediaAsset::Bytes(body) = asset else {
        panic!("playlist");
    };
    let playlist = String::from_utf8(body.bytes().to_vec()).unwrap();
    assert!(playlist.contains("init-0.mp4"));
    assert!(!playlist.contains("init-1.mp4"));
}

#[tokio::test]
async fn historical_clip_stays_on_disk_even_when_live_tail_differs() {
    let (catalog, source) = catalog("history-vs-live").await;
    seed_live(
        &catalog,
        &source,
        &track("epoch-live"),
        INIT_B,
        ts("2024-06-01T00:00:03Z"),
        sample(2),
    );
    catalog
        .dvr()
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
    catalog
        .dvr()
        .append_h264_sample(
            &source,
            ts("2024-06-01T00:00:03Z"),
            200_000,
            true,
            INIT_B,
            &sample(2),
        )
        .await
        .unwrap();
    let range = TimeRange::new(ts("2024-06-01T00:00:00Z"), ts("2024-06-01T00:00:05Z")).unwrap();
    let clip = catalog
        .get_video_clip(GetVideoClipRequest::new(source.clone(), range).unwrap())
        .await
        .unwrap();
    let asset = catalog
        .resolve_clip_playback_asset(clip.id(), Some(HLS_PLAYLIST_RELATIVE_PATH))
        .await
        .unwrap();
    let ResolvedMediaAsset::Bytes(body) = asset else {
        panic!("clip playlist");
    };
    let playlist = String::from_utf8(body.bytes().to_vec()).unwrap();
    assert!(playlist.contains("#EXT-X-DISCONTINUITY"));
    assert!(playlist.contains("init-1.mp4"));

    let follow = catalog
        .follow_video(a2a_lab_dev_kit::FollowVideoRequest::new(source))
        .await
        .unwrap();
    let live_init = catalog
        .resolve_live_playback_asset(follow.id(), Some("init-0.mp4"))
        .await
        .unwrap();
    let clip_init_b = catalog
        .resolve_clip_playback_asset(clip.id(), Some("init-1.mp4"))
        .await
        .unwrap();
    let ResolvedMediaAsset::Bytes(live_init) = live_init else {
        panic!("live init");
    };
    let ResolvedMediaAsset::Bytes(clip_init_b) = clip_init_b else {
        panic!("disk init");
    };
    assert_eq!(live_init.bytes(), clip_init_b.bytes());
}

#[tokio::test]
async fn saturated_dvr_queue_does_not_block_live_follow() {
    let queue = DvrPersistenceQueue::new(1).unwrap();
    assert_eq!(
        queue
            .try_enqueue(ts("2024-06-01T00:00:01Z"), vec![0xAA])
            .unwrap(),
        DvrEnqueueStatus::Accepted
    );
    assert_eq!(
        queue
            .try_enqueue(ts("2024-06-01T00:00:02Z"), vec![0xBB])
            .unwrap(),
        DvrEnqueueStatus::Full
    );
    queue.stop();
    assert_eq!(
        queue
            .try_enqueue(ts("2024-06-01T00:00:03Z"), vec![0xCC])
            .unwrap(),
        DvrEnqueueStatus::Stopped
    );
    let (catalog, source) = catalog("queue-pressure").await;
    seed_live(
        &catalog,
        &source,
        &track("epoch-live"),
        INIT_A,
        ts("2024-06-01T00:00:04Z"),
        sample(4),
    );
    let follow = catalog
        .follow_video(a2a_lab_dev_kit::FollowVideoRequest::new(source))
        .await
        .unwrap();
    catalog
        .resolve_live_playback_asset(follow.id(), Some(HLS_PLAYLIST_RELATIVE_PATH))
        .await
        .unwrap();
}

#[tokio::test]
async fn sparse_disk_does_not_block_live_follow() {
    let (catalog, source) = catalog("sparse-disk").await;
    seed_live(
        &catalog,
        &source,
        &track("epoch-live"),
        INIT_A,
        ts("2024-06-01T00:00:01Z"),
        sample(1),
    );
    let follow = catalog
        .follow_video(a2a_lab_dev_kit::FollowVideoRequest::new(source.clone()))
        .await
        .unwrap();
    catalog
        .resolve_live_playback_asset(follow.id(), Some(HLS_PLAYLIST_RELATIVE_PATH))
        .await
        .unwrap();
}

#[tokio::test]
async fn reconnect_clears_live_samples_until_new_frames_arrive() {
    let (catalog, source) = catalog("reconnect").await;
    let entry = catalog
        .live_h264()
        .register(&source, 60, 5, 1_000_000)
        .unwrap();
    let store: Arc<_> = Arc::clone(entry.inner());
    let picture = DvrPictureProperties::new(640, 480).unwrap();
    store
        .open_epoch(track("epoch-live"), INIT_A.to_vec(), picture)
        .unwrap();
    store
        .append_sample(
            &track("epoch-live"),
            ts("2024-06-01T00:00:01Z"),
            sample(1),
            true,
        )
        .unwrap();
    let follow = catalog
        .follow_video(a2a_lab_dev_kit::FollowVideoRequest::new(source.clone()))
        .await
        .unwrap();
    store.reconnect().unwrap();
    let Err(error) = catalog
        .resolve_live_playback_asset(follow.id(), Some(HLS_PLAYLIST_RELATIVE_PATH))
        .await
    else {
        panic!("empty live store after reconnect");
    };
    assert!(
        matches!(error.code(), "no_recording" | "live_unavailable"),
        "{error}"
    );
    store
        .append_sample(
            &track("epoch-live"),
            ts("2024-06-01T00:00:02Z"),
            sample(2),
            true,
        )
        .unwrap();
    catalog
        .resolve_live_playback_asset(follow.id(), Some(HLS_PLAYLIST_RELATIVE_PATH))
        .await
        .unwrap();
}

#[tokio::test]
async fn historical_gap_returns_structured_no_recording() {
    let (catalog, source) = catalog("historical-gap").await;
    catalog
        .dvr()
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
    let range = TimeRange::new(ts("2024-06-01T00:01:00Z"), ts("2024-06-01T00:01:05Z")).unwrap();
    let error = catalog
        .get_video_clip(GetVideoClipRequest::new(source, range).unwrap())
        .await
        .expect_err("empty historical range");
    assert_eq!(error.code(), "no_recording");
}

#[tokio::test]
async fn fixed_anchor_outside_live_window_is_unavailable() {
    let (catalog, source) = catalog("live-to-history").await;
    seed_live(
        &catalog,
        &source,
        &track("epoch-live"),
        INIT_A,
        ts("2024-06-01T00:00:10Z"),
        sample(1),
    );
    let follow = catalog
        .follow_video(a2a_lab_dev_kit::FollowVideoRequest::at(
            source,
            ts("2024-06-01T00:02:00Z"),
        ))
        .await
        .unwrap();
    let Err(error) = catalog
        .resolve_live_playback_asset(follow.id(), Some(HLS_PLAYLIST_RELATIVE_PATH))
        .await
    else {
        panic!("anchor outside live retention");
    };
    assert!(
        matches!(error.code(), "no_recording" | "live_unavailable"),
        "{error}"
    );
}
