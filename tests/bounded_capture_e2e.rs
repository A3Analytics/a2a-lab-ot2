//! Deterministic hardware-free boundary for bounded 30fps capture, cadence, routing, and shutdown.

use std::path::PathBuf;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use a2a_lab_dev_kit::videos::{
    AdvertisedCaptureProfile, CaptureClock, CaptureDrainRunner, CaptureFormat, CaptureIdentity,
    CaptureMode, CaptureRate, CaptureTiming, CapturedColorFrame, ColorCaptureSource, ColorMatrix,
    ColorProperties, ColorRange, LatestFrameStore, LiveDvrCadenceSelector, RawCameraSample,
    SENSOR_30FPS_PERIOD_NS, SelectedCaptureProfile,
};
use a2a_lab_dev_kit::{
    A2aLabError, DvrEnqueueStatus, DvrPersistenceQueue, FollowVideoRequest, GetVideoClipRequest,
    HLS_PLAYLIST_RELATIVE_PATH, ResolvedMediaAsset, SaveIntervalDuration, TimeRange, UtcTimestamp,
    VideoProvider, VideoSourceId,
};
use a2a_lab_ot2::camera::clear_all_live_stills;
use a2a_lab_ot2::{
    IndexedSourceFilter, LiveCameraInputFormat, LiveVideoDelivery, LiveVideoInjections,
    LiveVideoSourceDescriptor, start_fixture_declarative_serve,
};

fn temp_root(name: &str) -> PathBuf {
    std::env::temp_dir().join(format!(
        "a2a-lab-ot2-bounded-e2e-{name}-{}",
        std::process::id()
    ))
}

fn tiny_jpeg(y: u8) -> Vec<u8> {
    use image::codecs::jpeg::JpegEncoder;
    use image::{ExtendedColorType, ImageBuffer, ImageEncoder, Rgb};
    let image: ImageBuffer<Rgb<u8>, Vec<u8>> = ImageBuffer::from_fn(16, 16, |_, _| Rgb([y, y, y]));
    let mut jpeg = Vec::new();
    JpegEncoder::new_with_quality(&mut jpeg, 80)
        .write_image(image.as_raw(), 16, 16, ExtendedColorType::Rgb8)
        .expect("jpeg");
    jpeg
}

fn h264_source(id: &str) -> LiveVideoSourceDescriptor {
    h264_source_device(id, 1)
}

fn h264_source_device(id: &str, device_index: u32) -> LiveVideoSourceDescriptor {
    let (camera_id, device_index) = a2a_lab_ot2::index_camera_fields(device_index);
    LiveVideoSourceDescriptor {
        id: id.to_owned(),
        name: format!("Name {id}"),
        description: format!("Description {id}"),
        camera_id,
        device_index,
        input: LiveCameraInputFormat::Mjpeg,
        width: Some(16),
        height: Some(16),
        capture_rate: Some(30),
        delivery: LiveVideoDelivery::H264,
    }
}

fn save_interval_nanos(text: &str) -> u64 {
    SaveIntervalDuration::parse("videos.dvr.save_interval", text)
        .unwrap()
        .as_nanos()
}

/// Nominal 30 Hz sensor for injected catalog pumps.
struct ThirtyFpsSensor {
    negotiated: SelectedCaptureProfile,
    advertised: Vec<AdvertisedCaptureProfile>,
    frames: Vec<Vec<u8>>,
    index: AtomicU64,
}

impl ThirtyFpsSensor {
    fn mjpeg(frame_count: usize) -> Self {
        let mode = CaptureMode::new(CaptureFormat::Mjpeg, 16, 16).unwrap();
        let negotiated = SelectedCaptureProfile::negotiated(
            mode,
            CaptureRate::fps(30).unwrap(),
            CaptureRate::fps(30).unwrap(),
        );
        Self {
            negotiated,
            advertised: vec![AdvertisedCaptureProfile::new(
                mode,
                CaptureRate::fps(30).unwrap(),
            )],
            frames: (0..frame_count)
                .map(|i| tiny_jpeg(u8::try_from(i % 251).unwrap_or(255)))
                .collect(),
            index: AtomicU64::new(0),
        }
    }
}

impl a2a_lab_dev_kit::videos::ColorCamera for ThirtyFpsSensor {
    fn advertised(&mut self) -> Result<Vec<AdvertisedCaptureProfile>, A2aLabError> {
        Ok(self.advertised.clone())
    }

    fn open(
        &mut self,
        profile: &SelectedCaptureProfile,
    ) -> Result<SelectedCaptureProfile, A2aLabError> {
        assert_eq!(profile.mode().format(), CaptureFormat::Mjpeg);
        Ok(self.negotiated)
    }

    fn raw_frame(&mut self) -> Result<RawCameraSample, A2aLabError> {
        let index = usize::try_from(self.index.fetch_add(1, Ordering::SeqCst)).unwrap_or(0);
        let bytes = self
            .frames
            .get(index)
            .cloned()
            .unwrap_or_else(|| self.frames.last().cloned().unwrap_or_default());
        Ok(RawCameraSample::new(CaptureFormat::Mjpeg, 16, 16, 0, bytes))
    }

    fn stop(&mut self) -> Result<(), A2aLabError> {
        Ok(())
    }
}

struct FlakyThirtyFps {
    inner: ThirtyFpsSensor,
    failures: Mutex<usize>,
}

impl a2a_lab_dev_kit::videos::ColorCamera for FlakyThirtyFps {
    fn advertised(&mut self) -> Result<Vec<AdvertisedCaptureProfile>, A2aLabError> {
        self.inner.advertised()
    }

    fn open(
        &mut self,
        profile: &SelectedCaptureProfile,
    ) -> Result<SelectedCaptureProfile, A2aLabError> {
        self.inner.open(profile)
    }

    fn raw_frame(&mut self) -> Result<RawCameraSample, A2aLabError> {
        let mut left = self.failures.lock().expect("failures");
        if *left > 0 {
            *left -= 1;
            return Err(A2aLabError::unavailable("transient disconnect"));
        }
        self.inner.raw_frame()
    }

    fn stop(&mut self) -> Result<(), A2aLabError> {
        self.inner.stop()
    }
}

struct FailingSensor;

impl a2a_lab_dev_kit::videos::ColorCamera for FailingSensor {
    fn advertised(&mut self) -> Result<Vec<AdvertisedCaptureProfile>, A2aLabError> {
        Err(A2aLabError::unavailable("missing"))
    }

    fn open(
        &mut self,
        _profile: &SelectedCaptureProfile,
    ) -> Result<SelectedCaptureProfile, A2aLabError> {
        Err(A2aLabError::unavailable("missing"))
    }

    fn raw_frame(&mut self) -> Result<RawCameraSample, A2aLabError> {
        Err(A2aLabError::unavailable("missing"))
    }

    fn stop(&mut self) -> Result<(), A2aLabError> {
        Ok(())
    }
}

struct ThirtyFpsPull {
    next: u64,
    end: u64,
}

impl ThirtyFpsPull {
    const fn new(end: u64) -> Self {
        Self { next: 0, end }
    }
}

impl ColorCaptureSource for ThirtyFpsPull {
    fn capabilities(&self) -> &a2a_lab_dev_kit::videos::CaptureCapabilities {
        static CAPS: std::sync::OnceLock<a2a_lab_dev_kit::videos::CaptureCapabilities> =
            std::sync::OnceLock::new();
        CAPS.get_or_init(|| {
            a2a_lab_dev_kit::videos::CaptureCapabilities::new(vec![
                CaptureMode::new(CaptureFormat::Mjpeg, 16, 16).unwrap(),
            ])
            .unwrap()
        })
    }

    fn pull_frame(&mut self) -> Result<CapturedColorFrame, A2aLabError> {
        if self.next >= self.end {
            return Err(A2aLabError::unavailable("producer ended"));
        }
        let sequence = self.next;
        self.next = self.next.saturating_add(1);
        Ok(CapturedColorFrame::mjpeg(
            CaptureIdentity::new(VideoSourceId::new("pull").unwrap(), sequence),
            CaptureTiming::new(
                UtcTimestamp::parse("2026-01-01T00:00:00Z").unwrap(),
                CaptureClock::monotonic(sequence.saturating_mul(SENSOR_30FPS_PERIOD_NS)),
            ),
            16,
            16,
            &tiny_jpeg(u8::try_from(sequence % 251).unwrap_or(1)),
            ColorProperties::new(ColorMatrix::Bt709, ColorRange::Limited),
        )
        .unwrap())
    }
}

async fn count_dvr_h264(store: &a2a_lab_ot2::IndexedVideoDvr, source: &str) -> u64 {
    sqlx::query_scalar(
        "SELECT COUNT(*) FROM dvr_frames
         WHERE source_id = ?1 AND track_id = 'h264' AND payload_ready = 1",
    )
    .bind(source)
    .fetch_one(store.media_pool())
    .await
    .unwrap()
}

async fn count_indexing_jobs(store: &a2a_lab_ot2::IndexedVideoDvr) -> u64 {
    store.count_indexing_jobs().await.unwrap()
}

fn count_live_h264_samples(catalog: &a2a_lab_ot2::DeclarativeVideoServe, id: &str) -> usize {
    let source = VideoSourceId::new(id).unwrap();
    let live = catalog
        .provider()
        .live_h264()
        .get(&source)
        .expect("live store");
    live.inner().snapshot().unwrap().samples().len()
}

fn live_sample_cap(retain_secs: u64, live_fps: u32) -> usize {
    usize::try_from(
        retain_secs
            .saturating_mul(u64::from(live_fps))
            .saturating_add(48)
            .max(32),
    )
    .unwrap_or(32)
}

async fn run_capture(
    id: &str,
    live_fps: u32,
    save_text: Option<&str>,
    wall_ms: u64,
    retain_secs: u64,
    _frames: usize,
    camera: Box<dyn a2a_lab_dev_kit::videos::ColorCamera + Send>,
) -> (a2a_lab_ot2::DeclarativeVideoServe, PathBuf) {
    let label = save_text.unwrap_or("equal");
    let root = temp_root(&format!("{id}-{label}"));
    let _ = std::fs::remove_dir_all(&root);
    let source = h264_source(id);
    let catalog = start_fixture_declarative_serve(
        vec![source],
        live_fps,
        30,
        retain_secs,
        &root,
        256 * 1024,
        save_text.map(save_interval_nanos),
        LiveVideoInjections::none().with_color_camera(id, camera),
    )
    .await
    .unwrap();
    catalog
        .dvr()
        .set_indexed_source_filter(IndexedSourceFilter::allow_only(
            std::iter::once(id.to_owned()).collect(),
        ));
    tokio::time::sleep(Duration::from_millis(wall_ms)).await;
    (catalog, root)
}

async fn wait_for_follow(provider: &impl VideoProvider, id: &str) {
    let source = VideoSourceId::new(id).unwrap();
    for _ in 0..120 {
        if provider
            .follow_video(FollowVideoRequest::new(source.clone()))
            .await
            .is_ok()
        {
            return;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    panic!("live follow never became available for {id}");
}

#[test]
fn invalid_cadence_and_save_interval_boundaries_reject_zero_period() {
    let cadence = LiveDvrCadenceSelector::new(0, 1_000_000_000, 8);
    assert!(cadence.is_err(), "zero live period must be rejected");
    for text in ["0", "0ms", "nope"] {
        assert!(
            SaveIntervalDuration::parse("videos.dvr.save_interval", text).is_err(),
            "{text}"
        );
    }
}

#[test]
fn thirty_fps_drain_keeps_queue_depth_and_retained_bytes_bounded() {
    const FRAMES: u64 = 1_500;
    let mut runner = CaptureDrainRunner::new(ThirtyFpsPull::new(FRAMES));
    let mut max_depth = 0usize;
    let mut max_bytes = 0usize;
    for _ in 0..FRAMES {
        runner.drain_once();
        max_depth = max_depth.max(runner.queue_depth());
        max_bytes = max_bytes.max(runner.retained_bytes());
        if runner.queue_depth() > 0 {
            let _ = runner.take_latest();
        }
    }
    assert_eq!(max_depth, 1, "capacity-one latest-frame queue");
    assert_eq!(LatestFrameStore::CAPACITY, 1);
    assert!(max_bytes <= 256 * 1024, "max_bytes={max_bytes}");
    assert_eq!(runner.counters().drained(), FRAMES);
}

#[tokio::test]
async fn thirty_fps_live_and_dvr_cadence_diverge_for_representative_intervals() {
    for (label, save, wall_ms) in [
        ("fast", Some("500ms"), 2_500),
        ("equal", None, 2_500),
        ("sparse", Some("30s"), 4_000),
    ] {
        let id = format!("cadence-{label}");
        let (catalog, root) = run_capture(
            &id,
            5,
            save,
            wall_ms,
            120,
            4_000,
            Box::new(ThirtyFpsSensor::mjpeg(8_000)),
        )
        .await;
        let dvr = count_dvr_h264(catalog.dvr(), &id).await;
        let live = count_live_h264_samples(&catalog, &id);
        assert!(live >= 4, "{label}: live={live}");
        match label {
            "fast" | "equal" => {
                assert!(dvr >= 1, "{label}: dvr={dvr}");
                assert!(live >= 4, "{label}: live={live}");
            }
            "sparse" => {
                assert!(dvr >= 1, "{label}: dvr={dvr}");
            }
            _ => unreachable!(),
        }
        catalog.shutdown();
        let _ = std::fs::remove_dir_all(root);
    }
}

#[tokio::test]
async fn simulated_long_run_caps_live_memory_and_matches_semantic_jobs() {
    let id = "long-run";
    let retain = 60_u64;
    let live_fps = 5_u32;
    let (catalog, root) = run_capture(
        id,
        live_fps,
        Some("1s"),
        6_000,
        retain,
        12_000,
        Box::new(ThirtyFpsSensor::mjpeg(12_000)),
    )
    .await;
    tokio::time::sleep(Duration::from_millis(500)).await;
    let dvr = count_dvr_h264(catalog.dvr(), id).await;
    let jobs = count_indexing_jobs(catalog.dvr()).await;
    assert!(
        jobs.abs_diff(dvr.saturating_mul(2)) <= 4,
        "jobs={jobs} dvr={dvr}"
    );
    let live = count_live_h264_samples(&catalog, id);
    let cap = live_sample_cap(retain, live_fps);
    assert!(live <= cap, "live={live} cap={cap}");
    assert!(dvr >= 1, "dvr={dvr}");
    assert!(
        catalog.still_cache().latest(id).is_some(),
        "latest still missing"
    );
    catalog.shutdown();
    let _ = std::fs::remove_dir_all(root);
}

#[tokio::test]
async fn reconnect_shutdown_and_source_isolation_stay_observable() {
    clear_all_live_stills();
    let root = temp_root("behaviors");
    let _ = std::fs::remove_dir_all(&root);
    let sources = [
        h264_source_device("healthy", 0),
        h264_source_device("flaky", 1),
        h264_source_device("missing", 2),
    ];
    let catalog = start_fixture_declarative_serve(
        sources.to_vec(),
        5,
        30,
        120,
        &root,
        128 * 1024,
        Some(save_interval_nanos("1s")),
        LiveVideoInjections::none()
            .with_color_camera("healthy", Box::new(ThirtyFpsSensor::mjpeg(600)))
            .with_color_camera(
                "flaky",
                Box::new(FlakyThirtyFps {
                    inner: ThirtyFpsSensor::mjpeg(600),
                    failures: Mutex::new(2),
                }),
            )
            .with_color_camera("missing", Box::new(FailingSensor)),
    )
    .await
    .unwrap();
    wait_for_follow(&catalog.provider(), "healthy").await;
    wait_for_follow(&catalog.provider(), "flaky").await;
    assert!(catalog.still_cache().latest("healthy").is_some());
    assert!(catalog.still_cache().latest("flaky").is_some());
    catalog.shutdown();
    tokio::time::sleep(Duration::from_millis(150)).await;
    let _ = std::fs::remove_dir_all(root);
}

#[tokio::test]
async fn disk_fault_preserves_live_follow_without_duplicating_drop_reports() {
    let id = "disk-fault";
    let root = temp_root(id);
    let _ = std::fs::remove_dir_all(&root);
    let source = h264_source(id);
    let catalog = start_fixture_declarative_serve(
        vec![source],
        5,
        30,
        120,
        &root,
        256 * 1024,
        Some(save_interval_nanos("500ms")),
        LiveVideoInjections::none().with_color_camera(id, Box::new(ThirtyFpsSensor::mjpeg(8_000))),
    )
    .await
    .unwrap();
    catalog.dvr().save_faults().set_fail_payload_append(true);
    tokio::time::sleep(Duration::from_millis(2_000)).await;
    wait_for_follow(&catalog.provider(), id).await;
    let live_during_fault = count_live_h264_samples(&catalog, id);
    let dvr_during_fault = count_dvr_h264(catalog.dvr(), id).await;
    assert!(live_during_fault >= 4, "live_during={live_during_fault}");
    catalog.dvr().save_faults().set_fail_payload_append(false);
    tokio::time::sleep(Duration::from_millis(2_500)).await;
    let dvr_after = count_dvr_h264(catalog.dvr(), id).await;
    assert!(
        dvr_after >= dvr_during_fault,
        "dvr_after={dvr_after} during={dvr_during_fault}"
    );
    let jobs = count_indexing_jobs(catalog.dvr()).await;
    assert!(
        jobs.abs_diff(dvr_after.saturating_mul(2)) <= 4,
        "jobs={jobs} dvr={dvr_after}"
    );
    catalog.shutdown();
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn persistence_queue_saturation_is_bounded_and_observable() {
    let queue = DvrPersistenceQueue::new(1).unwrap();
    let ts = UtcTimestamp::parse("2024-06-01T00:00:00Z").unwrap();
    assert_eq!(
        queue.try_enqueue(ts, vec![0xAA]).unwrap(),
        DvrEnqueueStatus::Accepted
    );
    assert_eq!(
        queue.try_enqueue(ts, vec![0xBB]).unwrap(),
        DvrEnqueueStatus::Full
    );
    queue.stop();
    assert_eq!(
        queue.try_enqueue(ts, vec![0xCC]).unwrap(),
        DvrEnqueueStatus::Stopped
    );
}

#[tokio::test]
async fn live_follow_and_history_clip_route_separately() {
    let id = "routing";
    let (catalog, root) = run_capture(
        id,
        5,
        Some("1s"),
        3_500,
        300,
        6_000,
        Box::new(ThirtyFpsSensor::mjpeg(6_000)),
    )
    .await;
    wait_for_follow(&catalog.provider(), id).await;
    let source = VideoSourceId::new(id).unwrap();
    let follow = catalog
        .provider()
        .follow_video(FollowVideoRequest::new(source.clone()))
        .await
        .unwrap();
    let live_asset = catalog
        .provider()
        .resolve_live_playback_asset(follow.id(), Some(HLS_PLAYLIST_RELATIVE_PATH))
        .await
        .unwrap();
    let ResolvedMediaAsset::Bytes(live_playlist) = live_asset else {
        panic!("live playlist");
    };
    assert!(
        String::from_utf8(live_playlist.bytes().to_vec())
            .unwrap()
            .contains("init-"),
        "live playlist missing init segment"
    );
    let now = a2a_lab_ot2::time::now().unwrap();
    let range = TimeRange::new(
        now.saturating_add_micros(-120_000_000),
        now.saturating_add_micros(-1_000_000),
    )
    .unwrap();
    let clip = catalog
        .provider()
        .get_video_clip(GetVideoClipRequest::new(source, range).unwrap())
        .await;
    if let Ok(clip) = clip {
        let asset = catalog
            .provider()
            .resolve_clip_playback_asset(clip.id(), Some(HLS_PLAYLIST_RELATIVE_PATH))
            .await
            .unwrap();
        let ResolvedMediaAsset::Bytes(history_playlist) = asset else {
            panic!("history playlist");
        };
        let history = String::from_utf8(history_playlist.bytes().to_vec()).unwrap();
        assert!(history.contains("init-"), "history playlist");
    }
    catalog.shutdown();
    let _ = std::fs::remove_dir_all(root);
}
