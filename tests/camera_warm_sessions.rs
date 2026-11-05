//! Warm capture sessions, still/live reuse, and independent camera failure.

use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use a2a_lab_dev_kit::{
    A2aLabError, FollowVideoRequest, GetCurrentImageRequest, ImageProvider,
    ListVideoSourcesRequest, PageRequest, VideoProvider, VideoSourceId,
};
use a2a_lab_dev_kit::{ImageSource, ImageSourceId};
use a2a_lab_ot2::camera::{
    CAMERA_SOURCE_ID, CameraCapture, CameraCaptureError, CameraStill, EXTERNAL_CAMERA_SOURCE_ID,
    ExternalCamera, ExternalCameraFormat, clear_camera_frame, remember_camera_frame,
    remember_external_frame,
};
use a2a_lab_ot2::{
    ExternalVideoConfig, ImageCatalogConfig, LiveImageCatalog, LiveJpegSource, LiveVideoInjections,
    VideoServeConfig, WebcamVideoConfig, image_catalog, start_live_videos,
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

fn temp_dvr() -> PathBuf {
    std::env::temp_dir().join(format!(
        "a2a-lab-ot2-warm-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("clock")
            .as_nanos()
    ))
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

struct RememberingWebcam {
    bytes: Vec<u8>,
}

impl LiveJpegSource for RememberingWebcam {
    fn next_jpeg(&mut self) -> Result<Vec<u8>, A2aLabError> {
        Ok(self.bytes.clone())
    }
}

fn camera_image_source() -> ImageSource {
    ImageSource {
        id: ImageSourceId::new(CAMERA_SOURCE_ID).unwrap(),
        name: "Camera".to_owned(),
        description: "Still frame from the configured camera.".to_owned(),
        asset_id: None,
        semantic_id: None,
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
        5,
        86_400,
        &temp_dvr(),
        1024,
        LiveVideoInjections {
            external: Some(Box::new(RepeatJpeg {
                bytes: frame,
                reads: Arc::clone(&reads),
            })),
            webcam: None,
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
        5,
        86_400,
        &temp_dvr(),
        1024,
        LiveVideoInjections {
            external: Some(Box::new(RememberingExternal {
                bytes: live_bytes.clone(),
            })),
            webcam: None,
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
async fn camera_still_reuses_remembered_live_frame_without_opening_a_device() {
    let live_bytes = jpeg(8, 6, 7);
    remember_camera_frame(live_bytes.clone());
    let images = LiveImageCatalog::new(
        vec![(
            camera_image_source(),
            Arc::new(CameraStill::new(1024).unwrap()),
        )],
        ImageCatalogConfig::new(4, 1024).unwrap(),
    )
    .unwrap();
    let still = images
        .get_current_image(GetCurrentImageRequest::new(
            ImageSourceId::new(CAMERA_SOURCE_ID).unwrap(),
        ))
        .await
        .unwrap();
    assert_eq!(still.data(), live_bytes.as_slice());
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
        5,
        86_400,
        &temp_dvr(),
        1024,
        LiveVideoInjections {
            external: Some(Box::new(FlakyExternal {
                bytes: bytes.clone(),
                failures: Mutex::new(2),
                reads: AtomicUsize::new(0),
            })),
            webcam: None,
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
        5,
        86_400,
        &temp_dvr(),
        1024,
        LiveVideoInjections {
            external: Some(Box::new(RememberingExternal {
                bytes: jpeg(2, 2, 1),
            })),
            webcam: None,
        },
    )
    .await
    .unwrap();
    wait_for_follow(&catalog.provider(), EXTERNAL_CAMERA_SOURCE_ID).await;
    catalog.shutdown();
    tokio::time::sleep(Duration::from_millis(100)).await;
}

#[tokio::test]
async fn camera_live_survives_external_failure() {
    let camera_bytes = jpeg(5, 5, 9);
    let catalog = start_live_videos(
        Some(ExternalVideoConfig {
            index: 3,
            description: "external",
        }),
        Some(WebcamVideoConfig { index: 1 }),
        30,
        5,
        86_400,
        &temp_dvr(),
        1024,
        LiveVideoInjections {
            external: Some(Box::new(FailExternal)),
            webcam: Some(Box::new(RememberingWebcam {
                bytes: camera_bytes.clone(),
            })),
        },
    )
    .await
    .unwrap();
    let provider = catalog.provider();
    wait_for_follow(&provider, CAMERA_SOURCE_ID).await;
    let page = provider
        .list_video_sources(ListVideoSourcesRequest::new(page()).unwrap())
        .await
        .unwrap();
    assert_eq!(page.items().len(), 2);
    catalog.shutdown();
}

#[tokio::test]
async fn camera_without_a_frame_does_not_break_external_still_path() {
    clear_camera_frame();
    let external_bytes = jpeg(4, 2, 3);
    let external_capture = CaptureScript::ok(external_bytes.clone());
    let config = a2a_lab_ot2::ImageServeConfig::new(Some(4), None, 1024, 100, 4)
        .unwrap()
        .with_camera_index(1);
    let images = image_catalog(
        "http://127.0.0.1:9",
        &config,
        Some(Arc::new(external_capture.clone())),
        None,
    )
    .unwrap();
    let camera_error = images
        .get_current_image(GetCurrentImageRequest::new(
            ImageSourceId::new(CAMERA_SOURCE_ID).unwrap(),
        ))
        .await
        .unwrap_err();
    assert_eq!(camera_error.code(), "unavailable");
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
fn video_config_summary_lists_camera_when_enabled() {
    let config = VideoServeConfig::new(Some(4), None, 4, 120, 1024)
        .unwrap()
        .with_camera_index(7);
    let summary = config.summary();
    assert!(summary.contains("external-camera"));
    assert!(summary.contains("camera index 7"));
}

#[tokio::test]
async fn camera_live_and_still_share_exact_bytes_from_the_pump() {
    let shared = jpeg(10, 8, 44);
    let catalog = start_live_videos(
        None,
        Some(WebcamVideoConfig { index: 1 }),
        30,
        5,
        86_400,
        &temp_dvr(),
        1024,
        LiveVideoInjections {
            external: None,
            webcam: Some(Box::new(RememberingWebcam {
                bytes: shared.clone(),
            })),
        },
    )
    .await
    .unwrap();
    wait_for_follow(&catalog.provider(), CAMERA_SOURCE_ID).await;
    tokio::time::sleep(Duration::from_millis(40)).await;
    let images = LiveImageCatalog::new(
        vec![(
            camera_image_source(),
            Arc::new(CameraStill::new(1024).unwrap()),
        )],
        ImageCatalogConfig::new(4, 1024).unwrap(),
    )
    .unwrap();
    let still = images
        .get_current_image(GetCurrentImageRequest::new(
            ImageSourceId::new(CAMERA_SOURCE_ID).unwrap(),
        ))
        .await
        .unwrap();
    assert_eq!(still.data(), shared.as_slice());
    catalog.shutdown();
}
