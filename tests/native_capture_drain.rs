//! Hardware-free native capture drain and latest-frame behavior.

use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use a2a_lab_dev_kit::videos::{
    AdvertisedCaptureProfile, CaptureCapabilities, CaptureClock, CaptureDrainRunner, CaptureFormat,
    CaptureIdentity, CaptureMode, CaptureRate, CaptureTiming, CapturedColorFrame, ColorCamera,
    ColorCaptureSource, ColorMatrix, ColorProperties, ColorRange, LatestFrameStore,
    RawCameraSample, SelectedCaptureProfile,
};
use a2a_lab_dev_kit::{
    A2aLabError, FollowVideoRequest, GetCurrentImageRequest, ImageProvider, ImageSource,
    ImageSourceId, ListVideoSourcesRequest, PageRequest, UtcTimestamp, VideoProvider,
    VideoSourceId,
};
use a2a_lab_ot2::camera::{clear_all_live_stills, latest_live_still};
use a2a_lab_ot2::{
    ImageCatalogConfig, LiveCameraInputFormat, LiveImageCatalog, LiveVideoDelivery,
    LiveVideoInjections, LiveVideoSourceDescriptor, start_live_video_catalog,
};

fn jpeg(marker: u8) -> Vec<u8> {
    vec![
        0xFF, 0xD8, 0xFF, 0xC0, 0x00, 0x0B, 0x08, 0x00, 0x04, 0x00, 0x04, 0x01, 0x01, 0x11, 0x00,
        marker, 0xFF, 0xD9,
    ]
}

fn temp_dvr(name: &str) -> PathBuf {
    std::env::temp_dir().join(format!("a2a-lab-ot2-drain-{name}-{}", std::process::id()))
}

fn page() -> PageRequest {
    PageRequest::new(None, 10).unwrap()
}

fn descriptor(id: &str, device_index: u32) -> LiveVideoSourceDescriptor {
    LiveVideoSourceDescriptor {
        id: id.to_owned(),
        name: format!("Name {id}"),
        description: format!("Description {id}"),
        device_index,
        input: LiveCameraInputFormat::Mjpeg,
        width: Some(4),
        height: Some(4),
        capture_rate: Some(30),
        delivery: LiveVideoDelivery::Mjpeg,
    }
}

fn profile(format: CaptureFormat, width: u32, height: u32, fps: u32) -> AdvertisedCaptureProfile {
    AdvertisedCaptureProfile::new(
        CaptureMode::new(format, width, height).expect("mode"),
        CaptureRate::fps(fps).expect("rate"),
    )
}

struct ScriptedFrames {
    capabilities: CaptureCapabilities,
    pending: Vec<Result<CapturedColorFrame, A2aLabError>>,
    index: usize,
}

impl ColorCaptureSource for ScriptedFrames {
    fn capabilities(&self) -> &CaptureCapabilities {
        &self.capabilities
    }

    fn pull_frame(&mut self) -> Result<CapturedColorFrame, A2aLabError> {
        let next = self
            .pending
            .get(self.index)
            .cloned()
            .unwrap_or_else(|| Err(A2aLabError::unavailable("script ended")));
        self.index = self.index.saturating_add(1);
        next
    }
}

fn mjpeg_frame(sequence: u64, marker: u8) -> CapturedColorFrame {
    CapturedColorFrame::mjpeg(
        CaptureIdentity::new(VideoSourceId::new("drain").unwrap(), sequence),
        CaptureTiming::new(
            UtcTimestamp::parse("2026-01-01T00:00:00Z").unwrap(),
            CaptureClock::monotonic(sequence),
        ),
        4,
        4,
        &jpeg(marker),
        ColorProperties::new(ColorMatrix::Unknown, ColorRange::Unknown),
    )
    .unwrap()
}

#[test]
fn slow_consumer_keeps_capacity_one_queue() {
    let caps =
        CaptureCapabilities::new(vec![CaptureMode::new(CaptureFormat::Mjpeg, 4, 4).unwrap()])
            .unwrap();
    let pending: Vec<_> = (0..12)
        .map(|sequence| Ok(mjpeg_frame(sequence, u8::try_from(sequence % 250).unwrap())))
        .collect();
    let mut runner = CaptureDrainRunner::new(ScriptedFrames {
        capabilities: caps,
        pending,
        index: 0,
    });
    runner.drain_steps(8);
    assert_eq!(runner.queue_depth(), 1);
    assert_eq!(LatestFrameStore::CAPACITY, 1);
    assert_eq!(runner.peek_latest().unwrap().identity().sequence(), 7);
    runner.drain_steps(4);
    let latest = runner.take_latest().unwrap();
    assert_eq!(latest.identity().sequence(), 11);
    assert_eq!(runner.counters().replaced(), 11);
}

struct FailingCamera;

impl Default for FailingCamera {
    fn default() -> Self {
        Self
    }
}

impl ColorCamera for FailingCamera {
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

struct SequenceCamera {
    formats: Vec<AdvertisedCaptureProfile>,
    negotiated: SelectedCaptureProfile,
    frames: Vec<Vec<u8>>,
    index: AtomicU64,
    pulls: Arc<AtomicU64>,
}

impl SequenceCamera {
    fn new(frames: Vec<Vec<u8>>) -> (Self, Arc<AtomicU64>) {
        let pulls = Arc::new(AtomicU64::new(0));
        let negotiated = SelectedCaptureProfile::negotiated(
            CaptureMode::new(CaptureFormat::Mjpeg, 4, 4).unwrap(),
            CaptureRate::fps(30).unwrap(),
            CaptureRate::fps(5).unwrap(),
        );
        (
            Self {
                formats: vec![profile(CaptureFormat::Mjpeg, 4, 4, 30)],
                negotiated,
                frames,
                index: AtomicU64::new(0),
                pulls: Arc::clone(&pulls),
            },
            pulls,
        )
    }
}

impl ColorCamera for SequenceCamera {
    fn advertised(&mut self) -> Result<Vec<AdvertisedCaptureProfile>, A2aLabError> {
        Ok(self.formats.clone())
    }

    fn open(
        &mut self,
        profile: &SelectedCaptureProfile,
    ) -> Result<SelectedCaptureProfile, A2aLabError> {
        assert_eq!(profile.mode().format(), CaptureFormat::Mjpeg);
        Ok(self.negotiated)
    }

    fn raw_frame(&mut self) -> Result<RawCameraSample, A2aLabError> {
        self.pulls.fetch_add(1, Ordering::SeqCst);
        let index = usize::try_from(self.index.fetch_add(1, Ordering::SeqCst)).unwrap_or(0);
        let bytes = self
            .frames
            .get(index)
            .cloned()
            .unwrap_or_else(|| self.frames.last().cloned().unwrap_or_else(|| jpeg(0)));
        Ok(RawCameraSample::new(CaptureFormat::Mjpeg, 4, 4, 0, bytes))
    }

    fn stop(&mut self) -> Result<(), A2aLabError> {
        Ok(())
    }
}

struct FlakyCamera {
    inner: SequenceCamera,
    failures: Mutex<usize>,
}

struct WrongMode {
    negotiated: SelectedCaptureProfile,
}

impl ColorCamera for WrongMode {
    fn advertised(&mut self) -> Result<Vec<AdvertisedCaptureProfile>, A2aLabError> {
        Ok(vec![profile(CaptureFormat::Mjpeg, 4, 4, 5)])
    }

    fn open(
        &mut self,
        _profile: &SelectedCaptureProfile,
    ) -> Result<SelectedCaptureProfile, A2aLabError> {
        Ok(self.negotiated)
    }

    fn raw_frame(&mut self) -> Result<RawCameraSample, A2aLabError> {
        Ok(RawCameraSample::new(
            CaptureFormat::Yuy2,
            4,
            4,
            8,
            vec![1; 32],
        ))
    }

    fn stop(&mut self) -> Result<(), A2aLabError> {
        Ok(())
    }
}

impl ColorCamera for FlakyCamera {
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
            return Err(A2aLabError::unavailable("transient"));
        }
        self.inner.raw_frame()
    }

    fn stop(&mut self) -> Result<(), A2aLabError> {
        self.inner.stop()
    }
}

async fn wait_for_follow(provider: &impl VideoProvider, id: &str) {
    let source = VideoSourceId::new(id).unwrap();
    for _ in 0..80 {
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

#[tokio::test]
async fn latest_still_tracks_pump_after_delayed_consumer() {
    clear_all_live_stills();
    let frames = vec![jpeg(1), jpeg(2), jpeg(3)];
    let (camera, pulls) = SequenceCamera::new(frames.clone());
    let sources = [descriptor("delayed", 0)];
    let catalog = start_live_video_catalog(
        &sources,
        5,
        5,
        86_400,
        &temp_dvr("delayed"),
        1024,
        None,
        LiveVideoInjections::none().with_color_camera("delayed", Box::new(camera)),
    )
    .await
    .unwrap();
    tokio::time::sleep(Duration::from_millis(350)).await;
    assert!(pulls.load(Ordering::SeqCst) > 1);
    let remembered = latest_live_still("delayed").expect("remembered");
    assert_eq!(remembered, frames.last().unwrap().as_slice());
    let images = LiveImageCatalog::new(
        vec![(
            ImageSource {
                id: ImageSourceId::new("delayed").unwrap(),
                name: "delayed".to_owned(),
                description: "x".to_owned(),
                asset_id: None,
                semantic_id: None,
            },
            Arc::new(
                a2a_lab_ot2::camera::PumpLinkedStill::new("delayed", "delayed", 1024).unwrap(),
            ),
        )],
        ImageCatalogConfig::new(4, 1024).unwrap(),
    )
    .unwrap();
    let still = images
        .get_current_image(GetCurrentImageRequest::new(
            ImageSourceId::new("delayed").unwrap(),
        ))
        .await
        .unwrap();
    assert_eq!(still.data(), frames.last().unwrap().as_slice());
    catalog.shutdown();
}

#[tokio::test]
async fn warmup_reports_unavailable_until_first_frame() {
    clear_all_live_stills();
    let (camera, _) = SequenceCamera::new(vec![jpeg(9)]);
    let sources = [descriptor("warmup", 1)];
    let catalog = start_live_video_catalog(
        &sources,
        5,
        5,
        86_400,
        &temp_dvr("warmup"),
        1024,
        None,
        LiveVideoInjections::none().with_color_camera("warmup", Box::new(camera)),
    )
    .await
    .unwrap();
    wait_for_follow(&catalog.provider(), "warmup").await;
    assert!(latest_live_still("warmup").is_some());
    catalog.shutdown();
}

#[tokio::test]
async fn reconnect_after_transient_fault_restores_live_and_still() {
    clear_all_live_stills();
    let (inner, pulls) = SequenceCamera::new(vec![jpeg(4), jpeg(5), jpeg(6)]);
    let camera = FlakyCamera {
        inner,
        failures: Mutex::new(2),
    };
    let sources = [descriptor("flaky", 2)];
    let catalog = start_live_video_catalog(
        &sources,
        5,
        5,
        86_400,
        &temp_dvr("flaky"),
        1024,
        None,
        LiveVideoInjections::none().with_color_camera("flaky", Box::new(camera)),
    )
    .await
    .unwrap();
    wait_for_follow(&catalog.provider(), "flaky").await;
    assert!(pulls.load(Ordering::SeqCst) >= 2);
    assert!(latest_live_still("flaky").is_some());
    catalog.shutdown();
}

#[tokio::test]
async fn independent_sources_keep_stable_ids_when_one_is_unavailable() {
    clear_all_live_stills();
    let (good, _) = SequenceCamera::new(vec![jpeg(8)]);
    let sources = [descriptor("healthy", 0), descriptor("missing", 1)];
    let catalog = start_live_video_catalog(
        &sources,
        5,
        5,
        86_400,
        &temp_dvr("independent"),
        1024,
        None,
        LiveVideoInjections::none()
            .with_color_camera("healthy", Box::new(good))
            .with_color_camera("missing", Box::new(FailingCamera)),
    )
    .await
    .unwrap();
    wait_for_follow(&catalog.provider(), "healthy").await;
    let page = catalog
        .provider()
        .list_video_sources(ListVideoSourcesRequest::new(page()).unwrap())
        .await
        .unwrap();
    assert_eq!(page.items().len(), 2);
    let ids: Vec<_> = page.items().iter().map(|item| item.id().as_str()).collect();
    assert!(ids.contains(&"healthy"));
    assert!(ids.contains(&"missing"));
    catalog.shutdown();
}

#[tokio::test]
async fn shutdown_leaves_runner_closed_without_leaking_threads() {
    clear_all_live_stills();
    let (camera, _) = SequenceCamera::new(vec![jpeg(2)]);
    let sources = [descriptor("stop", 3)];
    let catalog = start_live_video_catalog(
        &sources,
        5,
        5,
        86_400,
        &temp_dvr("stop"),
        1024,
        None,
        LiveVideoInjections::none().with_color_camera("stop", Box::new(camera)),
    )
    .await
    .unwrap();
    wait_for_follow(&catalog.provider(), "stop").await;
    catalog.shutdown();
    tokio::time::sleep(Duration::from_millis(120)).await;
}

#[tokio::test]
async fn mode_mismatch_surfaces_without_silent_fallback() {
    let negotiated = SelectedCaptureProfile::negotiated(
        CaptureMode::new(CaptureFormat::Yuy2, 4, 4).unwrap(),
        CaptureRate::fps(5).unwrap(),
        CaptureRate::fps(5).unwrap(),
    );
    let sources = [descriptor("mode", 0)];
    let catalog = start_live_video_catalog(
        &sources,
        5,
        5,
        86_400,
        &temp_dvr("mode"),
        4096,
        None,
        LiveVideoInjections::none().with_color_camera("mode", Box::new(WrongMode { negotiated })),
    )
    .await
    .unwrap();
    tokio::time::sleep(Duration::from_millis(300)).await;
    let follow = catalog
        .provider()
        .follow_video(FollowVideoRequest::new(VideoSourceId::new("mode").unwrap()))
        .await;
    assert!(follow.is_err() || latest_live_still("mode").is_none());
    catalog.shutdown();
}
