//! Hardware-free tests for the indexed video DVR.

use std::time::Duration;

use a2a_lab_dev_kit::{
    DvrFrameAtRequest, DvrMediaPayload, DvrMediaStore, GetVideoClipRequest, MemoryVideos,
    PageRequest, ReadDvrMediaWindowRequest, TimeRange, UtcTimestamp, VideoProvider, VideoSourceId,
};
use a2a_lab_ot2::videos::dvr::VideoDvrCatalog;
use a2a_lab_ot2::videos::dvr::chunk::{chunk_relpath, crc32, minute_start_us};
use a2a_lab_ot2::{DEFAULT_DISK_VIDEO_RETENTION_SECS, IndexedVideoDvr};

fn temp_root(name: &str) -> std::path::PathBuf {
    std::env::temp_dir().join(format!("a2a-lab-ot2-dvr-{name}-{}", std::process::id()))
}

fn ts(text: &str) -> UtcTimestamp {
    UtcTimestamp::parse(text).unwrap()
}

fn jpeg() -> Vec<u8> {
    vec![0xFF, 0xD8, 0xFF, 0xD9]
}

async fn open_store(name: &str, retain_secs: u64) -> IndexedVideoDvr {
    let dir = temp_root(name);
    let _ = std::fs::remove_dir_all(&dir);
    IndexedVideoDvr::open(&dir, retain_secs).await.unwrap()
}

#[tokio::test]
async fn restart_recovery_preserves_gapped_coverage() {
    let dir = temp_root("restart");
    let _ = std::fs::remove_dir_all(&dir);
    let source = VideoSourceId::new("cam-a").unwrap();
    let anchor = a2a_lab_ot2::time::now().unwrap();
    let stamps = [
        anchor,
        anchor.saturating_add_micros(1_000_000),
        anchor.saturating_add_micros(4_000_000),
        anchor.saturating_add_micros(5_000_000),
    ];
    {
        let store = IndexedVideoDvr::open(&dir, 86_400).await.unwrap();
        for stamp in stamps {
            store
                .append_mjpeg_frame(&source, stamp, 1_000_000, &jpeg())
                .await
                .unwrap();
        }
    }
    let store = IndexedVideoDvr::open(&dir, 86_400).await.unwrap();
    let coverage = store.media_coverage(&source).await.unwrap();
    assert_eq!(coverage.intervals().len(), 2);
    assert_eq!(coverage.discontinuities().len(), 1);
    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn cleanup_never_removes_other_sources() {
    let store = open_store("isolation", 60).await;
    let a = VideoSourceId::new("cam-a").unwrap();
    let b = VideoSourceId::new("cam-b").unwrap();
    let old = ts("2020-01-01T00:00:00Z");
    let fresh = ts("2024-06-01T00:00:00Z");
    store
        .append_mjpeg_frame(&a, old, 1_000_000, &jpeg())
        .await
        .unwrap();
    store
        .append_mjpeg_frame(&b, fresh, 1_000_000, &jpeg())
        .await
        .unwrap();
    store
        .cleanup_source(&a, ts("2024-06-01T00:01:00Z"))
        .await
        .unwrap();
    assert!(store.media_coverage(&a).await.is_err());
    assert!(store.media_coverage(&b).await.is_ok());
}

#[tokio::test]
async fn frame_at_distinguishes_gap_from_outside_retention() {
    let store = open_store("gaps", 86_400).await;
    let source = VideoSourceId::new("cam-a").unwrap();
    let track = MemoryVideos::mjpeg_track_id(&source).unwrap();
    store
        .append_mjpeg_frame(&source, ts("2024-06-01T00:00:00Z"), 1_000_000, &jpeg())
        .await
        .unwrap();
    store
        .append_mjpeg_frame(&source, ts("2024-06-01T00:00:04Z"), 1_000_000, &jpeg())
        .await
        .unwrap();
    let gap = store
        .frame_at(DvrFrameAtRequest::new(
            source.clone(),
            track.clone(),
            ts("2024-06-01T00:00:02Z"),
            None,
        ))
        .await
        .unwrap_err();
    assert_eq!(gap.code(), "recording_gap");
    let outside = store
        .frame_at(DvrFrameAtRequest::new(
            source,
            track,
            ts("2019-01-01T00:00:00Z"),
            None,
        ))
        .await
        .unwrap_err();
    assert_eq!(outside.code(), "no_recording");
}

#[tokio::test]
async fn window_reads_paginate_without_scanning_everything() {
    let store = open_store("long", 86_400 * 7).await;
    let source = VideoSourceId::new("cam-a").unwrap();
    let track = MemoryVideos::mjpeg_track_id(&source).unwrap();
    let start = ts("2024-06-01T00:00:00Z");
    for offset in 0..5_000_i64 {
        let at = start.saturating_add_micros(offset * 1_000_000);
        store
            .append_mjpeg_frame(&source, at, 1_000_000, &jpeg())
            .await
            .unwrap();
    }
    let window = TimeRange::new(ts("2024-06-01T01:00:00Z"), ts("2024-06-01T01:00:05Z")).unwrap();
    let page = store
        .read_media_window(
            ReadDvrMediaWindowRequest::new(
                source.clone(),
                track.clone(),
                window,
                PageRequest::new(None, 3).unwrap(),
            )
            .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(page.items().len(), 3);
    assert!(page.next_cursor().is_some());
}

#[tokio::test]
async fn clip_sessions_expire_with_stable_errors() {
    let store = open_store("clips", 86_400).await;
    let live = MemoryVideos::new();
    let catalog = VideoDvrCatalog::open(live, store, Duration::from_millis(1)).unwrap();
    let source = VideoSourceId::new("cam-a").unwrap();
    catalog
        .dvr()
        .append_mjpeg_frame(&source, ts("2024-06-01T00:00:00Z"), 1_000_000, &jpeg())
        .await
        .unwrap();
    let range = TimeRange::new(ts("2024-06-01T00:00:00Z"), ts("2024-06-01T00:00:02Z")).unwrap();
    let clip = catalog
        .get_video_clip(GetVideoClipRequest::new(source, range).unwrap())
        .await
        .unwrap();
    tokio::time::sleep(Duration::from_millis(5)).await;
    let Err(missing) = catalog.open_video_clip(clip.id()).await else {
        panic!("expected expired clip");
    };
    assert_eq!(missing.code(), "clip_expired");
}

#[tokio::test]
async fn partial_payloads_are_removed_on_recovery() {
    let dir = temp_root("partial");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("cam-a/external-camera-mjpeg")).unwrap();
    std::fs::write(
        dir.join("cam-a/external-camera-mjpeg/123.mjpeg.tmp"),
        jpeg(),
    )
    .unwrap();
    let store = IndexedVideoDvr::open(&dir, 86_400).await.unwrap();
    assert!(
        !dir.join("cam-a/external-camera-mjpeg/123.mjpeg.tmp")
            .exists()
    );
    let _ = store;
    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn filesystem_writes_do_not_block_concurrent_reads() {
    let store = open_store("async", 86_400).await;
    let source = VideoSourceId::new("cam-a").unwrap();
    store
        .append_mjpeg_frame(&source, ts("2024-06-01T00:00:00Z"), 1_000_000, &jpeg())
        .await
        .unwrap();
    let store = std::sync::Arc::new(store);
    let writer = {
        let store = std::sync::Arc::clone(&store);
        let source = source.clone();
        tokio::spawn(async move {
            for offset in 1..200_i64 {
                let at = ts("2024-06-01T00:00:00Z").saturating_add_micros(offset * 1_000_000);
                store
                    .append_mjpeg_frame(&source, at, 1_000_000, &jpeg())
                    .await
                    .unwrap();
            }
        })
    };
    for _ in 0..20 {
        let result =
            tokio::time::timeout(Duration::from_secs(1), store.media_coverage(&source)).await;
        assert!(result.is_ok(), "coverage read blocked the runtime");
    }
    writer.await.unwrap();
}

#[tokio::test]
async fn invalid_storage_is_rejected_before_opening() {
    let Err(error) = IndexedVideoDvr::open("", 86_400).await else {
        panic!("expected invalid root");
    };
    assert_eq!(error.code(), "invalid");
}

#[test]
fn default_disk_retention_is_three_days() {
    assert_eq!(DEFAULT_DISK_VIDEO_RETENTION_SECS, 3 * 24 * 60 * 60);
}

#[tokio::test]
async fn startup_cleanup_drops_expired_rows() {
    let store = open_store("startup-clean", 10).await;
    let source = VideoSourceId::new("cam-a").unwrap();
    store
        .append_mjpeg_frame(&source, ts("2020-01-01T00:00:00Z"), 1_000_000, &jpeg())
        .await
        .unwrap();
    store.cleanup_at(ts("2024-06-01T00:00:00Z")).await.unwrap();
    assert!(store.media_coverage(&source).await.is_err());
}

#[tokio::test]
async fn legacy_per_frame_segments_remain_readable() {
    let store = open_store("legacy-seg", 86_400).await;
    let source = VideoSourceId::new("cam-a").unwrap();
    let track = MemoryVideos::mjpeg_track_id(&source).unwrap();
    store
        .write_legacy_segment(
            &source,
            &track,
            ts("2024-06-01T00:00:00Z"),
            1_000_000,
            true,
            &jpeg(),
        )
        .await
        .unwrap();
    store
        .append_mjpeg_frame(&source, ts("2024-06-01T00:00:01Z"), 1_000_000, &jpeg())
        .await
        .unwrap();
    let frame = store
        .frame_at(DvrFrameAtRequest::new(
            source.clone(),
            track.clone(),
            ts("2024-06-01T00:00:00Z"),
            None,
        ))
        .await
        .unwrap();
    assert!(matches!(
        frame.payload(),
        DvrMediaPayload::MjpegFrame(bytes) if bytes == &jpeg()
    ));
}

fn recent(offset_micros: i64) -> UtcTimestamp {
    a2a_lab_ot2::time::now()
        .unwrap()
        .saturating_add_micros(offset_micros)
}

#[tokio::test]
async fn chunk_checksum_mismatch_is_rejected() {
    let dir = temp_root("checksum");
    let _ = std::fs::remove_dir_all(&dir);
    let source = VideoSourceId::new("cam-a").unwrap();
    let at = recent(0);
    {
        let store = IndexedVideoDvr::open(&dir, 86_400).await.unwrap();
        store
            .append_mjpeg_frame(&source, at, 1_000_000, &jpeg())
            .await
            .unwrap();
    }
    let minute = minute_start_us(at);
    let track = MemoryVideos::mjpeg_track_id(&source).unwrap();
    let relpath = chunk_relpath(source.as_str(), track.as_str(), minute);
    let path = dir.join(format!("{relpath}.tmp"));
    let mut bytes = std::fs::read(&path).unwrap();
    if let Some(byte) = bytes.first_mut() {
        *byte ^= 0xFF;
    }
    std::fs::write(&path, bytes).unwrap();
    let store = IndexedVideoDvr::open(&dir, 86_400).await.unwrap();
    let Err(error) = store
        .frame_at(DvrFrameAtRequest::new(source, track, at, None))
        .await
    else {
        panic!("expected checksum failure");
    };
    assert!(format!("{error}").contains("checksum"));
    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn crash_recovery_truncates_unindexed_chunk_tail() {
    let dir = temp_root("crash");
    let _ = std::fs::remove_dir_all(&dir);
    let source = VideoSourceId::new("cam-a").unwrap();
    let at = recent(0);
    let retain_secs = 7 * 86_400;
    let track = MemoryVideos::mjpeg_track_id(&source).unwrap();
    let minute = minute_start_us(at);
    let relpath = chunk_relpath(source.as_str(), track.as_str(), minute);
    let tmp_path = dir.join(format!("{relpath}.tmp"));
    let final_path = dir.join(&relpath);
    {
        let store = IndexedVideoDvr::open(&dir, retain_secs).await.unwrap();
        store
            .append_mjpeg_frame(&source, at, 1_000_000, &jpeg())
            .await
            .unwrap();
    }
    let mut bytes = std::fs::read(&tmp_path).unwrap();
    bytes.extend_from_slice(&[0xDE, 0xAD, 0xBE, 0xEF]);
    std::fs::write(&tmp_path, bytes).unwrap();
    assert_eq!(
        std::fs::metadata(&tmp_path).unwrap().len(),
        jpeg().len() as u64 + 4
    );
    let store = IndexedVideoDvr::open(&dir, retain_secs).await.unwrap();
    let frame = store
        .frame_at(DvrFrameAtRequest::new(source, track, at, None))
        .await
        .unwrap();
    assert!(matches!(
        frame.payload(),
        DvrMediaPayload::MjpegFrame(bytes) if bytes == &jpeg()
    ));
    let payload_path = if tmp_path.exists() {
        tmp_path
    } else {
        final_path
    };
    assert_eq!(
        std::fs::metadata(payload_path).unwrap().len(),
        jpeg().len() as u64
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn chunk_reads_use_frame_byte_ranges() {
    let store = open_store("range", 86_400).await;
    let source = VideoSourceId::new("cam-a").unwrap();
    let track = MemoryVideos::mjpeg_track_id(&source).unwrap();
    let frame_a = vec![0xAA; 64];
    let frame_b = vec![0xBB; 64];
    store
        .append_mjpeg_frame(&source, ts("2024-06-01T00:00:00Z"), 1_000_000, &frame_a)
        .await
        .unwrap();
    store
        .append_mjpeg_frame(&source, ts("2024-06-01T00:00:01Z"), 1_000_000, &frame_b)
        .await
        .unwrap();
    let second = store
        .frame_at(DvrFrameAtRequest::new(
            source.clone(),
            track.clone(),
            ts("2024-06-01T00:00:01Z"),
            None,
        ))
        .await
        .unwrap();
    assert!(matches!(
        second.payload(),
        DvrMediaPayload::MjpegFrame(bytes) if bytes == &frame_b
    ));
    let minute = minute_start_us(ts("2024-06-01T00:00:00Z"));
    let relpath = chunk_relpath(source.as_str(), track.as_str(), minute);
    let on_disk = std::fs::read(store.root().join(format!("{relpath}.tmp"))).unwrap();
    assert_eq!(on_disk.len(), frame_a.len() + frame_b.len());
    assert_eq!(crc32(&frame_a), crc32(&on_disk[..frame_a.len()]));
}

#[tokio::test]
async fn minute_rotation_finalizes_previous_chunk() {
    let store = open_store("rotate", 86_400).await;
    let source = VideoSourceId::new("cam-a").unwrap();
    let track = MemoryVideos::mjpeg_track_id(&source).unwrap();
    store
        .append_mjpeg_frame(&source, ts("2024-06-01T00:00:59Z"), 1_000_000, &jpeg())
        .await
        .unwrap();
    store
        .append_mjpeg_frame(&source, ts("2024-06-01T00:01:01Z"), 1_000_000, &jpeg())
        .await
        .unwrap();
    let first_minute = minute_start_us(ts("2024-06-01T00:00:00Z"));
    let second_minute = minute_start_us(ts("2024-06-01T00:01:00Z"));
    let first = chunk_relpath(source.as_str(), track.as_str(), first_minute);
    let second = chunk_relpath(source.as_str(), track.as_str(), second_minute);
    assert!(store.root().join(&first).is_file());
    assert!(store.root().join(format!("{second}.tmp")).is_file());
}

#[tokio::test]
async fn chunk_retention_removes_expired_payloads() {
    let store = open_store("chunk-retain", 60).await;
    let source = VideoSourceId::new("cam-a").unwrap();
    store
        .append_mjpeg_frame(&source, ts("2020-01-01T00:00:00Z"), 1_000_000, &jpeg())
        .await
        .unwrap();
    let track = MemoryVideos::mjpeg_track_id(&source).unwrap();
    let minute = minute_start_us(ts("2020-01-01T00:00:00Z"));
    let relpath = chunk_relpath(source.as_str(), track.as_str(), minute);
    assert!(store.root().join(format!("{relpath}.tmp")).exists());
    store.cleanup_at(ts("2024-06-01T00:00:00Z")).await.unwrap();
    assert!(store.media_coverage(&source).await.is_err());
    assert!(!store.root().join(format!("{relpath}.tmp")).exists());
}
