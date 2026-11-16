//! Epoch-aware H.264 provider selection over disk DVR.

use std::time::Duration;

use a2a_lab_dev_kit::{
    A2aLabError, GetVideoClipRequest, HLS_PLAYLIST_RELATIVE_PATH, MemoryVideos, ResolvedMediaAsset,
    TimeRange, UtcTimestamp, VideoCodec, VideoProvider, VideoSource, VideoSourceId,
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
        "a2a-lab-ot2-h264-epoch-provider-{name}-{}",
        std::process::id()
    ))
}

fn ts(text: &str) -> UtcTimestamp {
    UtcTimestamp::parse(text).unwrap()
}

fn sample(tag: u8) -> Vec<u8> {
    vec![0x00, 0x00, 0x00, 0x01, 0x65, tag]
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

#[tokio::test]
async fn historical_clip_maps_each_epoch_to_its_init() {
    let (catalog, source) = catalog("clip-boundary").await;
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
    assert_eq!(clip.codec(), VideoCodec::H264);
    let asset = catalog
        .resolve_clip_playback_asset(clip.id(), Some(HLS_PLAYLIST_RELATIVE_PATH))
        .await
        .unwrap();
    let ResolvedMediaAsset::Bytes(body) = asset else {
        panic!("expected packaged playlist bytes");
    };
    let playlist = String::from_utf8(body.bytes().to_vec()).unwrap();
    assert!(playlist.contains("#EXT-X-DISCONTINUITY"));
    assert!(playlist.contains("init-0.mp4"));
    assert!(playlist.contains("init-1.mp4"));
    let init_a = catalog
        .resolve_clip_playback_asset(clip.id(), Some("init-0.mp4"))
        .await
        .unwrap();
    let init_b = catalog
        .resolve_clip_playback_asset(clip.id(), Some("init-1.mp4"))
        .await
        .unwrap();
    let ResolvedMediaAsset::Bytes(init_a) = init_a else {
        panic!("init-a");
    };
    let ResolvedMediaAsset::Bytes(init_b) = init_b else {
        panic!("init-b");
    };
    assert_ne!(init_a.bytes(), init_b.bytes());
}

fn seed_live_tail(catalog: &VideoDvrCatalog, source: &VideoSourceId) {
    use a2a_lab_dev_kit::{DvrPictureProperties, DvrTrackId};
    let picture = DvrPictureProperties::new(640, 480).unwrap();
    let track = DvrTrackId::new("epoch-live-tail").unwrap();
    let live = catalog
        .live_h264()
        .register(source, 60, 5, 1_000_000)
        .unwrap();
    live.inner()
        .open_epoch(track.clone(), INIT_B.to_vec(), picture)
        .unwrap();
    live.inner()
        .append_sample(&track, ts("2024-06-01T00:00:02Z"), sample(2), true)
        .unwrap();
    live.inner()
        .append_sample(&track, ts("2024-06-01T00:00:03Z"), sample(3), true)
        .unwrap();
}

#[tokio::test]
async fn live_follow_anchors_the_latest_epoch() {
    let (catalog, source) = catalog("live-tail").await;
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
    seed_live_tail(&catalog, &source);
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
    let init = catalog
        .resolve_live_playback_asset(follow.id(), Some("init-0.mp4"))
        .await
        .unwrap();
    let ResolvedMediaAsset::Bytes(init) = init else {
        panic!("init segment");
    };
    let epoch_b = catalog
        .get_video_clip(
            GetVideoClipRequest::new(
                source.clone(),
                TimeRange::new(ts("2024-06-01T00:00:03Z"), ts("2024-06-01T00:00:04Z")).unwrap(),
            )
            .unwrap(),
        )
        .await
        .unwrap();
    let clip_init = catalog
        .resolve_clip_playback_asset(epoch_b.id(), Some("init-0.mp4"))
        .await
        .unwrap();
    let ResolvedMediaAsset::Bytes(clip_init) = clip_init else {
        panic!("epoch-b init");
    };
    assert_eq!(init.bytes(), clip_init.bytes());
}

#[tokio::test]
async fn multi_epoch_finite_export_returns_structured_error() {
    let (catalog, source) = catalog("finite-export").await;
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
            ts("2024-06-01T00:00:05Z"),
            200_000,
            true,
            INIT_B,
            &sample(2),
        )
        .await
        .unwrap();
    let range = TimeRange::new(ts("2024-06-01T00:00:00Z"), ts("2024-06-01T00:00:07Z")).unwrap();
    let clip = catalog
        .get_video_clip(GetVideoClipRequest::new(source, range).unwrap())
        .await
        .unwrap();
    let error = catalog
        .export_finite_video_clip(clip.id())
        .await
        .expect_err("multi-epoch export");
    assert!(matches!(
        error,
        A2aLabError::Invalid {
            field: "export",
            ..
        }
    ));
}
