//! Indexing intent at the saved-frame metadata commit boundary.

use std::time::Duration;

use a2a_lab_dev_kit::{DvrStoreNamespace, DvrTrackId, MemoryVideos, VideoSourceId};
use a2a_lab_ot2::videos::dvr::{
    DEFAULT_RECONCILE_BATCH, DEFAULT_SEMANTIC_STORE_NAMESPACE, MediaPayloadStatus,
    RegisterManifestOutcome,
};
use a2a_lab_ot2::{IndexedVideoDvr, LiveVideoInjections, start_live_video_catalog};
fn temp_root(name: &str) -> std::path::PathBuf {
    std::env::temp_dir().join(format!(
        "a2a-lab-ot2-indexing-{name}-{}",
        std::process::id()
    ))
}

async fn open_store(name: &str) -> (std::path::PathBuf, IndexedVideoDvr) {
    let dir = temp_root(name);
    let _ = std::fs::remove_dir_all(&dir);
    let store = IndexedVideoDvr::open(&dir, 86_400).await.unwrap();
    (dir, store)
}

fn store_ns() -> DvrStoreNamespace {
    DvrStoreNamespace::new(DEFAULT_SEMANTIC_STORE_NAMESPACE).unwrap()
}

fn jpeg(tag: u8) -> Vec<u8> {
    vec![0xFF, 0xD8, tag, 0xD9]
}

fn fresh_capture(offset_us: i64) -> a2a_lab_dev_kit::UtcTimestamp {
    a2a_lab_ot2::time::now()
        .unwrap()
        .saturating_add_micros(offset_us)
}

async fn current_frame_id(
    store: &IndexedVideoDvr,
    source: &VideoSourceId,
    track: &DvrTrackId,
    capture: a2a_lab_dev_kit::UtcTimestamp,
) -> String {
    let (frame, _, _) = store
        .ensure_indexing_at_dvr_key(&store_ns(), source, track, capture)
        .await
        .unwrap();
    frame.frame_id().as_str().to_owned()
}

#[tokio::test]
async fn mjpeg_commit_records_manifest_and_unique_job() {
    let (_dir, store) = open_store("mjpeg-job").await;
    let source = VideoSourceId::new("camera").unwrap();
    let track = MemoryVideos::mjpeg_track_id(&source).unwrap();
    let capture = fresh_capture(0);
    store
        .append_mjpeg_frame(&source, capture, 1_000_000, &jpeg(1))
        .await
        .unwrap();
    assert_eq!(store.count_indexing_jobs().await.unwrap(), 1);
    let frame_id = current_frame_id(&store, &source, &track, capture).await;
    assert!(store.indexing_job_exists(&frame_id).await.unwrap());
}

#[tokio::test]
async fn h264_commit_records_manifest_and_unique_job() {
    let (_dir, store) = open_store("h264-job").await;
    let source = VideoSourceId::new("camera").unwrap();
    let capture = fresh_capture(0);
    let codec_init = vec![0, 0, 0, 1, 9];
    let sample = vec![0, 0, 0, 1, 5];
    store
        .append_h264_sample(&source, capture, 1_000_000, true, &codec_init, &sample)
        .await
        .unwrap();
    assert_eq!(store.count_indexing_jobs().await.unwrap(), 1);
    let track = DvrTrackId::new("h264").unwrap();
    let frame_id = current_frame_id(&store, &source, &track, capture).await;
    assert!(store.indexing_job_exists(&frame_id).await.unwrap());
}

#[tokio::test]
async fn metadata_commit_fault_exposes_no_row_or_job() {
    let (_dir, store) = open_store("meta-fault").await;
    let source = VideoSourceId::new("camera").unwrap();
    let capture = fresh_capture(0);
    store.save_faults().set_fail_metadata_commit(true);
    let result = store
        .append_mjpeg_frame(&source, capture, 1_000_000, &jpeg(2))
        .await;
    assert!(result.is_err());
    assert_eq!(store.count_indexing_jobs().await.unwrap(), 0);
    let count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM dvr_frames WHERE source_id = ?1 AND start_us = ?2",
    )
    .bind(source.as_str())
    .bind(capture.as_micros())
    .fetch_one(store.sqlite_pool())
    .await
    .unwrap();
    assert_eq!(count, 0);
}

#[tokio::test]
async fn payload_append_fault_skips_indexing_intent() {
    let (_dir, store) = open_store("append-fault").await;
    let source = VideoSourceId::new("camera").unwrap();
    let capture = fresh_capture(0);
    store.save_faults().set_fail_payload_append(true);
    let result = store
        .append_mjpeg_frame(&source, capture, 1_000_000, &jpeg(3))
        .await;
    assert!(result.is_err());
    assert_eq!(store.count_indexing_jobs().await.unwrap(), 0);
}

#[tokio::test]
async fn restart_recovers_jobs_without_in_memory_notification() {
    let dir = temp_root("restart-job");
    let _ = std::fs::remove_dir_all(&dir);
    let source = VideoSourceId::new("camera").unwrap();
    let capture = fresh_capture(0);
    {
        let store = IndexedVideoDvr::open(&dir, 86_400).await.unwrap();
        store
            .append_mjpeg_frame(&source, capture, 1_000_000, &jpeg(4))
            .await
            .unwrap();
        assert_eq!(store.count_indexing_jobs().await.unwrap(), 1);
    }
    let store = IndexedVideoDvr::open(&dir, 86_400).await.unwrap();
    assert_eq!(store.count_indexing_jobs().await.unwrap(), 1);
    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn reconcile_backfills_missing_manifest_and_job() {
    let (_dir, store) = open_store("reconcile").await;
    let source = VideoSourceId::new("camera").unwrap();
    let track = MemoryVideos::mjpeg_track_id(&source).unwrap();
    let capture = fresh_capture(0);
    store
        .append_mjpeg_frame(&source, capture, 1_000_000, &jpeg(5))
        .await
        .unwrap();
    sqlx::query("DELETE FROM semantic_indexing_job_runtime")
        .execute(store.sqlite_pool())
        .await
        .unwrap();
    sqlx::query("DELETE FROM semantic_indexing_jobs")
        .execute(store.sqlite_pool())
        .await
        .unwrap();
    sqlx::query("DELETE FROM semantic_frame_revisions")
        .execute(store.sqlite_pool())
        .await
        .unwrap();
    assert_eq!(store.count_indexing_jobs().await.unwrap(), 0);
    let report = store
        .reconcile_indexing_jobs(&store_ns(), DEFAULT_RECONCILE_BATCH)
        .await
        .unwrap();
    assert_eq!(report.examined(), 1);
    assert_eq!(store.count_indexing_jobs().await.unwrap(), 1);
    let frame_id = current_frame_id(&store, &source, &track, capture).await;
    assert!(store.indexing_job_exists(&frame_id).await.unwrap());
}

#[tokio::test]
async fn duplicate_replay_creates_no_extra_jobs_or_revisions() {
    let (_dir, store) = open_store("dup-replay").await;
    let source = VideoSourceId::new("camera").unwrap();
    let track = MemoryVideos::mjpeg_track_id(&source).unwrap();
    let capture = fresh_capture(0);
    let payload = jpeg(6);
    store
        .append_mjpeg_frame(&source, capture, 1_000_000, &payload)
        .await
        .unwrap();
    let first = current_frame_id(&store, &source, &track, capture).await;
    store
        .append_mjpeg_frame(&source, capture, 1_000_000, &payload)
        .await
        .unwrap();
    assert_eq!(store.count_indexing_jobs().await.unwrap(), 1);
    let (_, outcome, _) = store
        .ensure_indexing_at_dvr_key(&store_ns(), &source, &track, capture)
        .await
        .unwrap();
    assert_eq!(outcome, RegisterManifestOutcome::Idempotent);
    assert!(store.indexing_job_exists(&first).await.unwrap());
}

#[tokio::test]
async fn reconcile_marks_corrupt_payload_explicitly() {
    let (_dir, store) = open_store("corrupt").await;
    let source = VideoSourceId::new("camera").unwrap();
    let capture = fresh_capture(0);
    store
        .append_mjpeg_frame(&source, capture, 1_000_000, &jpeg(7))
        .await
        .unwrap();
    let track = MemoryVideos::mjpeg_track_id(&source).unwrap();
    let frame_id = current_frame_id(&store, &source, &track, capture).await;
    sqlx::query("UPDATE dvr_frames SET payload_crc32 = 0 WHERE source_id = ?1 AND start_us = ?2")
        .bind(source.as_str())
        .bind(capture.as_micros())
        .execute(store.sqlite_pool())
        .await
        .unwrap();
    let record = store.get_manifest_frame(&frame_id).await.unwrap();
    assert_eq!(record.payload_status(), MediaPayloadStatus::Corrupt);
}

async fn seed_retained_frames_missing_jobs(frame_count: usize) -> std::path::PathBuf {
    let dir = temp_root("annotation-queue");
    let _ = std::fs::remove_dir_all(&dir);
    let store = IndexedVideoDvr::open(&dir, 86_400).await.unwrap();
    let source = VideoSourceId::new("camera").unwrap();
    for index in 0..frame_count {
        store
            .append_mjpeg_frame(
                &source,
                fresh_capture(i64::try_from(index).unwrap_or(i64::MAX) * 1_000_000),
                1_000_000,
                &jpeg(u8::try_from(index).unwrap_or(u8::MAX)),
            )
            .await
            .unwrap();
    }
    sqlx::query("DELETE FROM semantic_indexing_job_runtime")
        .execute(store.sqlite_pool())
        .await
        .unwrap();
    sqlx::query("DELETE FROM semantic_indexing_jobs")
        .execute(store.sqlite_pool())
        .await
        .unwrap();
    sqlx::query("DELETE FROM semantic_frame_revisions")
        .execute(store.sqlite_pool())
        .await
        .unwrap();
    assert_eq!(store.count_indexing_jobs().await.unwrap(), 0);
    drop(store);
    dir
}

#[tokio::test]
async fn open_does_not_synchronously_backfill_indexing_jobs() {
    const FRAMES: usize = 12;
    let dir = seed_retained_frames_missing_jobs(FRAMES).await;
    let store = IndexedVideoDvr::open(&dir, 86_400).await.unwrap();
    assert_eq!(store.count_indexing_jobs().await.unwrap(), 0);
    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn live_catalog_annotation_queue_backfills_and_survives_restart() {
    const FRAMES: usize = 12;
    let dir = seed_retained_frames_missing_jobs(FRAMES).await;
    let catalog = start_live_video_catalog(
        &[],
        5,
        60,
        86_400,
        &dir,
        2_000_000,
        LiveVideoInjections::none(),
    )
    .await
    .unwrap();
    let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
    loop {
        let jobs = catalog.dvr().count_indexing_jobs().await.unwrap();
        if jobs == u64::try_from(FRAMES).unwrap() {
            break;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "expected {FRAMES} indexing jobs, found {jobs}"
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    let persisted = catalog.dvr().count_indexing_jobs().await.unwrap();
    catalog.shutdown();
    drop(catalog);
    let reopened = start_live_video_catalog(
        &[],
        5,
        60,
        86_400,
        &dir,
        2_000_000,
        LiveVideoInjections::none(),
    )
    .await
    .unwrap();
    assert_eq!(
        reopened.dvr().count_indexing_jobs().await.unwrap(),
        persisted
    );
    reopened.shutdown();
    let _ = std::fs::remove_dir_all(&dir);
}
