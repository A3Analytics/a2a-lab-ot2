//! Warm capture sessions, still/live reuse, and independent camera failure.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use a2a_lab_dev_kit::{
    A2aLabError, FollowVideoRequest, GetCurrentImageRequest, ImageProvider,
    ListVideoSourcesRequest, PageRequest, VideoProvider, VideoSourceId,
};
use a2a_lab_ot2::camera::{
    CameraCapture, CameraCaptureError, EXTERNAL_CAMERA_SOURCE_ID, ExternalCamera,
    ExternalCameraFormat, remember_external_frame,
};
use a2a_lab_ot2::orbbec::{
    ORBBEC_CAMERA_SOURCE_ID, ORBBEC_FIRST_FRAME_WAIT, OrbbecCamera, OrbbecCapture,
    OrbbecCaptureError, remember_color_frame,
};
use a2a_lab_ot2::{
    ExternalVideoConfig, ImageCatalogConfig, LiveImageCatalog, LiveJpegSource, LiveVideoInjections,
    OrbbecVideoConfig, VideoServeConfig, image_catalog, start_live_videos,
};

fn jpeg(width: u16, height: u16, marker: u8) -> Vec<u8> {
    let mut data = vec![0xFF, 0xD8, 0xFF, 0xC0, 0x00, 0x0B, 0x08];
    data.extend_from_slice(&height.to_be_bytes());
    data.extend_from_slice(&width.to_be_bytes());
    data.extend_from_slice(&[0x01, 0x01, 0x11, 0x00, marker, 0xFF, 0xD9]);
    data
}

fn page() -> PageRequest {
    PageRequest::new(None, 10).unwrap()
}

struct RepeatJpeg {
    bytes: Vec<u8>,
    reads: Arc<AtomicUsize>,
}

impl LiveJpegSource for RepeatJpeg {
    fn next_jpeg(&mut self) -> Result<Vec<u8>, A2aLabError> {
        self.reads.fetch_add(1, Ordering::SeqCst);
        Ok(self.bytes.clone())
    }
}

struct RememberingExternal {
    bytes: Vec<u8>,
}

impl LiveJpegSource for RememberingExternal {
    fn next_jpeg(&mut self) -> Result<Vec<u8>, A2aLabError> {
        remember_external_frame(self.bytes.clone());
        Ok(self.bytes.clone())
    }
}

struct FlakyExternal {
    bytes: Vec<u8>,
    failures: Mutex<usize>,
    reads: AtomicUsize,
}

impl LiveJpegSource for FlakyExternal {
    fn next_jpeg(&mut self) -> Result<Vec<u8>, A2aLabError> {
        self.reads.fetch_add(1, Ordering::SeqCst);
        let mut left = self.failures.lock().expect("failures");
        if *left > 0 {
            *left -= 1;
            return Err(A2aLabError::unavailable("external camera disconnected"));
        }
        remember_external_frame(self.bytes.clone());
        Ok(self.bytes.clone())
    }
}

struct FailExternal;

impl LiveJpegSource for FailExternal {
    fn next_jpeg(&mut self) -> Result<Vec<u8>, A2aLabError> {
        Err(A2aLabError::unavailable("external camera unavailable"))
    }
}

struct RememberingOrbbec {
    bytes: Vec<u8>,
}

impl LiveJpegSource for RememberingOrbbec {
    fn next_jpeg(&mut self) -> Result<Vec<u8>, A2aLabError> {
        remember_color_frame(self.bytes.clone());
        Ok(self.bytes.clone())
    }
}

#[derive(Clone)]
struct CaptureScript {
    calls: Arc<AtomicUsize>,
    result: Arc<Mutex<Result<Vec<u8>, CameraCaptureError>>>,
}

impl CaptureScript {
    fn ok(bytes: Vec<u8>) -> Self {
        Self {
            calls: Arc::new(AtomicUsize::new(0)),
            result: Arc::new(Mutex::new(Ok(bytes))),
        }
    }

    fn calls(&self) -> usize {
        self.calls.load(Ordering::SeqCst)
    }
}

impl CameraCapture for CaptureScript {
    fn capture(
        &self,
        _index: u32,
        _format: ExternalCameraFormat,
        _frames: u32,
    ) -> Result<Vec<u8>, CameraCaptureError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        self.result.lock().expect("result").clone()
    }
}

struct SlowOrbbec {
    delay: Duration,
    bytes: Vec<u8>,
}

impl OrbbecCapture for SlowOrbbec {
    fn capture_color(
        &self,
        _serial: Option<&str>,
        _timeout: Duration,
        _max_bytes: u64,
    ) -> Result<Vec<u8>, OrbbecCaptureError> {
        std::thread::sleep(self.delay);
        Ok(self.bytes.clone())
    }
}

struct CountingOrbbec {
    calls: Arc<AtomicUsize>,
    bytes: Vec<u8>,
}

impl OrbbecCapture for CountingOrbbec {
    fn capture_color(
        &self,
        _serial: Option<&str>,
        _timeout: Duration,
        _max_bytes: u64,
    ) -> Result<Vec<u8>, OrbbecCaptureError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        Ok(self.bytes.clone())
    }
}

async fn wait_for_follow(provider: &impl VideoProvider, source: &str) {
    let id = VideoSourceId::new(source).unwrap();
    for _ in 0..100 {
        if provider
            .follow_video(FollowVideoRequest::new(id.clone()))
            .await
            .is_ok()
        {
            return;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    panic!("live follow never became available for {source}");
}

#[tokio::test]
async fn external_live_session_pumps_multiple_frames_before_shutdown() {
    let frame = jpeg(4, 4, 1);
    let reads = Arc::new(AtomicUsize::new(0));
    let catalog = start_live_videos(
        Some(ExternalVideoConfig {
            index: 4,
            description: "test",
        }),
        None,
        30,
        1024,
        LiveVideoInjections {
            external: Some(Box::new(RepeatJpeg {
                bytes: frame,
                reads: Arc::clone(&reads),
            })),
            orbbec: None,
        },
    )
    .await
    .unwrap();
    let provider = catalog.provider();
    wait_for_follow(&provider, EXTERNAL_CAMERA_SOURCE_ID).await;
    tokio::time::sleep(Duration::from_millis(120)).await;
    assert!(reads.load(Ordering::SeqCst) >= 2);
    catalog.shutdown();
    tokio::time::sleep(Duration::from_millis(50)).await;
}

#[tokio::test]
async fn external_still_reuses_live_session_without_second_open() {
    let live_bytes = jpeg(6, 4, 42);
    let catalog = start_live_videos(
        Some(ExternalVideoConfig {
            index: 4,
            description: "test",
        }),
        None,
        30,
        1024,
        LiveVideoInjections {
            external: Some(Box::new(RememberingExternal {
                bytes: live_bytes.clone(),
            })),
            orbbec: None,
        },
    )
    .await
    .unwrap();
    wait_for_follow(&catalog.provider(), EXTERNAL_CAMERA_SOURCE_ID).await;
    tokio::time::sleep(Duration::from_millis(40)).await;

    let script = CaptureScript::ok(jpeg(1, 1, 99));
    let external =
        ExternalCamera::with_capture(4, Duration::from_millis(500), 1024, script.clone()).unwrap();
    let images = LiveImageCatalog::new(
        vec![(
            a2a_lab_ot2::camera::external_camera_source().unwrap(),
            Arc::new(external),
        )],
        ImageCatalogConfig::new(4, 1024).unwrap(),
    )
    .unwrap();
    let still = images
        .get_current_image(GetCurrentImageRequest::new(
            a2a_lab_dev_kit::ImageSourceId::new(EXTERNAL_CAMERA_SOURCE_ID).unwrap(),
        ))
        .await
        .unwrap();
    assert_eq!(still.data(), live_bytes.as_slice());
    assert_eq!(script.calls(), 0);
    catalog.shutdown();
}

#[tokio::test]
async fn orbbec_still_reuses_remembered_live_frame_without_second_capture() {
    let live_bytes = jpeg(8, 6, 7);
    remember_color_frame(live_bytes.clone());
    let calls = Arc::new(AtomicUsize::new(0));
    let capture = CountingOrbbec {
        calls: Arc::clone(&calls),
        bytes: jpeg(1, 1, 1),
    };
    let camera = OrbbecCamera::with_capture(None, ORBBEC_FIRST_FRAME_WAIT, 1024, capture).unwrap();
    let images = LiveImageCatalog::new(
        vec![(
            a2a_lab_ot2::orbbec::orbbec_camera_source().unwrap(),
            Arc::new(camera),
        )],
        ImageCatalogConfig::new(4, 1024).unwrap(),
    )
    .unwrap();
    let still = images
        .get_current_image(GetCurrentImageRequest::new(
            a2a_lab_dev_kit::ImageSourceId::new(ORBBEC_CAMERA_SOURCE_ID).unwrap(),
        ))
        .await
        .unwrap();
    assert_eq!(still.data(), live_bytes.as_slice());
    assert_eq!(calls.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn orbbec_cold_start_allows_twenty_second_first_frame_budget() {
    let bytes = jpeg(2, 2, 11);
    let camera = OrbbecCamera::with_capture(
        None,
        ORBBEC_FIRST_FRAME_WAIT,
        1024,
        SlowOrbbec {
            delay: Duration::from_millis(150),
            bytes: bytes.clone(),
        },
    )
    .unwrap();
    let images = LiveImageCatalog::new(
        vec![(
            a2a_lab_ot2::orbbec::orbbec_camera_source().unwrap(),
            Arc::new(camera),
        )],
        ImageCatalogConfig::new(4, 1024).unwrap(),
    )
    .unwrap();
    let started = Instant::now();
    let still = images
        .get_current_image(GetCurrentImageRequest::new(
            a2a_lab_dev_kit::ImageSourceId::new(ORBBEC_CAMERA_SOURCE_ID).unwrap(),
        ))
        .await
        .unwrap();
    assert!(started.elapsed() >= Duration::from_millis(140));
    assert!(started.elapsed() < ORBBEC_FIRST_FRAME_WAIT);
    assert_eq!(still.data(), bytes.as_slice());
}

#[tokio::test]
async fn external_live_reconnects_after_transient_disconnect() {
    let bytes = jpeg(3, 3, 5);
    let catalog = start_live_videos(
        Some(ExternalVideoConfig {
            index: 2,
            description: "test",
        }),
        None,
        30,
        1024,
        LiveVideoInjections {
            external: Some(Box::new(FlakyExternal {
                bytes: bytes.clone(),
                failures: Mutex::new(2),
                reads: AtomicUsize::new(0),
            })),
            orbbec: None,
        },
    )
    .await
    .unwrap();
    wait_for_follow(&catalog.provider(), EXTERNAL_CAMERA_SOURCE_ID).await;
    catalog.shutdown();
}

#[tokio::test]
async fn shutdown_stops_live_capture_without_leaking_threads() {
    let catalog = start_live_videos(
        Some(ExternalVideoConfig {
            index: 1,
            description: "test",
        }),
        None,
        30,
        1024,
        LiveVideoInjections {
            external: Some(Box::new(RememberingExternal {
                bytes: jpeg(2, 2, 1),
            })),
            orbbec: None,
        },
    )
    .await
    .unwrap();
    wait_for_follow(&catalog.provider(), EXTERNAL_CAMERA_SOURCE_ID).await;
    catalog.shutdown();
    tokio::time::sleep(Duration::from_millis(100)).await;
}

#[tokio::test]
async fn orbbec_live_survives_external_failure() {
    let orbbec_bytes = jpeg(5, 5, 9);
    let catalog = start_live_videos(
        Some(ExternalVideoConfig {
            index: 3,
            description: "external",
        }),
        Some(OrbbecVideoConfig { serial: None }),
        30,
        1024,
        LiveVideoInjections {
            external: Some(Box::new(FailExternal)),
            orbbec: Some(Box::new(RememberingOrbbec {
                bytes: orbbec_bytes.clone(),
            })),
        },
    )
    .await
    .unwrap();
    let provider = catalog.provider();
    wait_for_follow(&provider, ORBBEC_CAMERA_SOURCE_ID).await;
    let page = provider
        .list_video_sources(ListVideoSourcesRequest::new(page()).unwrap())
        .await
        .unwrap();
    assert_eq!(page.items().len(), 2);
    catalog.shutdown();
}

struct FailOrbbec;

impl OrbbecCapture for FailOrbbec {
    fn capture_color(
        &self,
        _serial: Option<&str>,
        _timeout: Duration,
        _max_bytes: u64,
    ) -> Result<Vec<u8>, OrbbecCaptureError> {
        Err(OrbbecCaptureError::NoDevice)
    }
}

#[tokio::test]
async fn orbbec_capture_errors_do_not_break_external_still_path() {
    let external_bytes = jpeg(4, 2, 3);
    let external_capture = CaptureScript::ok(external_bytes.clone());
    let config = a2a_lab_ot2::ImageServeConfig::new(Some(4), None, 1024, 100, 4)
        .unwrap()
        .with_orbbec(None)
        .unwrap();
    let images = image_catalog(
        "http://127.0.0.1:9",
        &config,
        Some(Arc::new(external_capture.clone())),
        Some(Arc::new(FailOrbbec)),
    )
    .unwrap();
    let orbbec_error = images
        .get_current_image(GetCurrentImageRequest::new(
            a2a_lab_dev_kit::ImageSourceId::new(ORBBEC_CAMERA_SOURCE_ID).unwrap(),
        ))
        .await
        .unwrap_err();
    assert_eq!(orbbec_error.code(), "unavailable");
    let external = images
        .get_current_image(GetCurrentImageRequest::new(
            a2a_lab_dev_kit::ImageSourceId::new(EXTERNAL_CAMERA_SOURCE_ID).unwrap(),
        ))
        .await
        .unwrap();
    assert_eq!(external.data(), external_bytes.as_slice());
    assert_eq!(external_capture.calls(), 1);
}

#[test]
fn video_config_summary_lists_orbbec_when_enabled() {
    let mut config = VideoServeConfig::new(Some(4), None, 4, 120, 1024).unwrap();
    config = config.with_orbbec(Some("ABC")).unwrap();
    let summary = config.summary();
    assert!(summary.contains("external-camera"));
    assert!(summary.contains("orbbec-camera serial ABC"));
}

#[tokio::test]
async fn orbbec_live_and_still_share_exact_bytes_from_session() {
    let shared = jpeg(10, 8, 44);
    let catalog = start_live_videos(
        None,
        Some(OrbbecVideoConfig { serial: None }),
        30,
        1024,
        LiveVideoInjections {
            external: None,
            orbbec: Some(Box::new(RememberingOrbbec {
                bytes: shared.clone(),
            })),
        },
    )
    .await
    .unwrap();
    wait_for_follow(&catalog.provider(), ORBBEC_CAMERA_SOURCE_ID).await;
    tokio::time::sleep(Duration::from_millis(40)).await;
    let calls = Arc::new(AtomicUsize::new(0));
    let camera = OrbbecCamera::with_shared_capture(
        None,
        ORBBEC_FIRST_FRAME_WAIT,
        1024,
        Arc::new(CountingOrbbec {
            calls: Arc::clone(&calls),
            bytes: jpeg(1, 1, 0),
        }),
    )
    .unwrap();
    let images = LiveImageCatalog::new(
        vec![(
            a2a_lab_ot2::orbbec::orbbec_camera_source().unwrap(),
            Arc::new(camera),
        )],
        ImageCatalogConfig::new(4, 1024).unwrap(),
    )
    .unwrap();
    let still = images
        .get_current_image(GetCurrentImageRequest::new(
            a2a_lab_dev_kit::ImageSourceId::new(ORBBEC_CAMERA_SOURCE_ID).unwrap(),
        ))
        .await
        .unwrap();
    assert_eq!(still.data(), shared.as_slice());
    assert_eq!(calls.load(Ordering::SeqCst), 0);
    catalog.shutdown();
}
