//! Platform-native external camera capture.

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use a2a_lab_dev_kit::{
    GetCurrentImageRequest, GetImageRequest, ImageProvider, ImageSourceId, ListImageSourcesRequest,
    ListImagesRequest, PageRequest,
};
use a2a_lab_ot2::camera::{
    CameraCapture, CameraCaptureError, EXTERNAL_CAMERA_FPS, EXTERNAL_CAMERA_HEIGHT,
    EXTERNAL_CAMERA_SETTLING_FRAMES, EXTERNAL_CAMERA_SOURCE_ID, EXTERNAL_CAMERA_WIDTH,
    ExternalCamera, ExternalCameraFormat, external_camera_source,
};
use a2a_lab_ot2::opentrons::{OPENTRONS_CAMERA_SOURCE_ID, OpentronsCamera};
use tokio::net::TcpListener;

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

fn external_id() -> ImageSourceId {
    ImageSourceId::new(EXTERNAL_CAMERA_SOURCE_ID).unwrap()
}

type CaptureCall = (u32, ExternalCameraFormat, u32);

#[derive(Clone)]
struct Script {
    calls: Arc<Mutex<Vec<CaptureCall>>>,
    result: Arc<Mutex<Result<Vec<u8>, CameraCaptureError>>>,
}

impl Script {
    fn ok(bytes: Vec<u8>) -> Self {
        Self::with(Ok(bytes))
    }

    fn fail(error: CameraCaptureError) -> Self {
        Self::with(Err(error))
    }

    fn with(result: Result<Vec<u8>, CameraCaptureError>) -> Self {
        Self {
            calls: Arc::new(Mutex::new(Vec::new())),
            result: Arc::new(Mutex::new(result)),
        }
    }

    fn calls(&self) -> Vec<CaptureCall> {
        self.calls.lock().expect("calls").clone()
    }
}

impl CameraCapture for Script {
    fn capture(
        &self,
        index: u32,
        format: ExternalCameraFormat,
        frames: u32,
    ) -> Result<Vec<u8>, CameraCaptureError> {
        self.calls
            .lock()
            .expect("calls")
            .push((index, format, frames));
        self.result.lock().expect("result").clone()
    }
}

struct SlowCapture;

impl CameraCapture for SlowCapture {
    fn capture(
        &self,
        _index: u32,
        _format: ExternalCameraFormat,
        _frames: u32,
    ) -> Result<Vec<u8>, CameraCaptureError> {
        std::thread::sleep(Duration::from_millis(250));
        Ok(jpeg(1, 1, 30))
    }
}

fn camera(index: u32, capture: Script) -> ExternalCamera {
    ExternalCamera::with_capture(index, Duration::from_millis(500), 1024, capture).unwrap()
}

#[test]
fn index_and_timeout_validation_do_not_open_hardware() {
    let camera = ExternalCamera::new(7, Duration::from_secs(1), 1024).unwrap();
    assert_eq!(camera.index(), 7);
    match ExternalCamera::with_capture(0, Duration::ZERO, 1024, Script::ok(jpeg(1, 1, 30))) {
        Ok(_) => panic!("zero timeout was accepted"),
        Err(error) => assert_eq!(error.code(), "invalid"),
    }
}

#[tokio::test]
async fn exact_format_and_thirty_frame_sequence_return_untouched_final_bytes() {
    let frame_30 = jpeg(2, 3, 30);
    let capture = Script::ok(frame_30.clone());
    let images = camera(4, capture.clone()).catalog(4).unwrap();
    let first = images
        .get_current_image(GetCurrentImageRequest::new(external_id()))
        .await
        .unwrap();
    let second = images
        .get_current_image(GetCurrentImageRequest::new(external_id()))
        .await
        .unwrap();

    assert_eq!(first.data(), frame_30.as_slice());
    assert_eq!(second.data(), frame_30.as_slice());
    assert_ne!(first.descriptor().id(), second.descriptor().id());
    assert_eq!(first.descriptor().media_type(), "image/jpeg");
    assert_eq!(first.descriptor().width(), 2);
    assert_eq!(first.descriptor().height(), 3);
    let stored = images
        .get_image(GetImageRequest::new(first.descriptor().id().clone()))
        .await
        .unwrap();
    assert_eq!(stored.data(), frame_30.as_slice());
    assert_eq!(
        capture.calls(),
        vec![
            (
                4,
                ExternalCameraFormat {
                    width: EXTERNAL_CAMERA_WIDTH,
                    height: EXTERNAL_CAMERA_HEIGHT,
                    fps: EXTERNAL_CAMERA_FPS,
                },
                EXTERNAL_CAMERA_SETTLING_FRAMES,
            );
            2
        ]
    );
}

async fn capture_error(error: CameraCaptureError) -> a2a_lab_dev_kit::A2aLabError {
    let images = camera(9, Script::fail(error)).catalog(2).unwrap();
    let error = images
        .get_current_image(GetCurrentImageRequest::new(external_id()))
        .await
        .unwrap_err();
    let listed = images
        .list_images(ListImagesRequest::new(external_id(), page()).unwrap())
        .await
        .unwrap();
    assert!(listed.items().is_empty());
    assert!(error.to_string().contains("index 9"));
    error
}

#[tokio::test]
async fn backend_and_format_failures_are_actionable_and_store_nothing() {
    let format = capture_error(CameraCaptureError::ExactFormatUnavailable(
        "unsupported format".to_owned(),
    ))
    .await;
    assert_eq!(format.code(), "unavailable");
    assert!(
        format
            .to_string()
            .contains("exact MJPEG 3840x2160 at 30 fps")
    );

    let backend = capture_error(CameraCaptureError::Backend("permission denied".to_owned())).await;
    assert_eq!(backend.code(), "unavailable");
    assert!(
        backend
            .to_string()
            .contains("native backend capture failed")
    );
    assert!(backend.to_string().contains("permission denied"));

    let empty = capture_error(CameraCaptureError::Empty).await;
    assert_eq!(empty.code(), "invalid");
    assert!(empty.to_string().contains("empty"));
}

#[tokio::test]
async fn timeout_and_payload_validation_retain_no_partial_image() {
    let images = ExternalCamera::with_capture(3, Duration::from_millis(40), 1024, SlowCapture)
        .unwrap()
        .catalog(2)
        .unwrap();
    let started = Instant::now();
    let error = images
        .get_current_image(GetCurrentImageRequest::new(external_id()))
        .await
        .unwrap_err();
    assert!(started.elapsed() < Duration::from_millis(200));
    assert_eq!(error.code(), "unavailable");
    assert!(error.to_string().contains("index 3"));
    assert!(error.to_string().contains("timed out"));
    assert!(
        images
            .list_images(ListImagesRequest::new(external_id(), page()).unwrap())
            .await
            .unwrap()
            .items()
            .is_empty()
    );

    let malformed = camera(3, Script::ok(b"not-a-jpeg".to_vec()))
        .catalog(2)
        .unwrap()
        .get_current_image(GetCurrentImageRequest::new(external_id()))
        .await
        .unwrap_err();
    assert_eq!(malformed.code(), "invalid");
    assert!(malformed.to_string().contains("malformed"));

    let oversized =
        ExternalCamera::with_capture(3, Duration::from_secs(1), 8, Script::ok(jpeg(8, 8, 30)))
            .unwrap()
            .catalog(2)
            .unwrap()
            .get_current_image(GetCurrentImageRequest::new(external_id()))
            .await
            .unwrap_err();
    assert_eq!(oversized.code(), "invalid");
    assert!(oversized.to_string().contains("maximum"));
}

#[tokio::test]
async fn external_camera_isolated_from_unavailable_opentrons_camera() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    drop(listener);
    let opentrons = OpentronsCamera::new(
        &format!("http://{address}"),
        Duration::from_millis(200),
        1024,
    )
    .unwrap();
    let capture = Script::ok(jpeg(4, 2, 30));
    let external =
        ExternalCamera::with_capture(6, Duration::from_millis(500), 1024, capture.clone()).unwrap();
    let images = a2a_lab_ot2::LiveImageCatalog::new(
        vec![
            (
                a2a_lab_ot2::opentrons::opentrons_camera_source().unwrap(),
                Arc::new(opentrons),
            ),
            (external_camera_source().unwrap(), Arc::new(external)),
        ],
        a2a_lab_ot2::ImageCatalogConfig::new(4, 1024).unwrap(),
    )
    .unwrap();
    let failed = images
        .get_current_image(GetCurrentImageRequest::new(
            ImageSourceId::new(OPENTRONS_CAMERA_SOURCE_ID).unwrap(),
        ))
        .await
        .unwrap_err();
    assert_eq!(failed.code(), "unavailable");
    let image = images
        .get_current_image(GetCurrentImageRequest::new(external_id()))
        .await
        .unwrap();
    assert_eq!(image.data(), jpeg(4, 2, 30));
    assert_eq!(
        capture.calls(),
        [(
            6,
            ExternalCameraFormat::REQUIRED,
            EXTERNAL_CAMERA_SETTLING_FRAMES
        )]
    );
    let sources = images
        .list_image_sources(ListImageSourcesRequest::new(page()).unwrap())
        .await
        .unwrap();
    assert_eq!(sources.items().len(), 2);
}
