//! Hardware-free Orbbec color capture.

use std::process::Command;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use a2a_lab_dev_kit::{
    GetCurrentImageRequest, GetImageRequest, ImageProvider, ImageSourceId, ListImageSourcesRequest,
    ListImagesRequest, PageRequest,
};
use a2a_lab_ot2::camera::{CameraCapture, CameraCaptureError, EXTERNAL_CAMERA_SOURCE_ID};
use a2a_lab_ot2::opentrons::OPENTRONS_CAMERA_SOURCE_ID;
use a2a_lab_ot2::orbbec::{
    ORBBEC_CAMERA_SOURCE_ID, OrbbecCamera, OrbbecCapture, OrbbecCaptureError,
};
use a2a_lab_ot2::{ImageServeConfig, image_catalog};

fn bin() -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_a2a-lab-ot2"));
    command
        .env_remove("A2ALAB_ORBBEC_CAMERA")
        .env_remove("A2ALAB_ORBBEC_SERIAL")
        .env_remove("A2ALAB_EXTERNAL_CAMERA_INDEX")
        .env_remove("A2ALAB_ORBBEC_SDK_LIBRARY");
    command
}

fn jpeg(width: u16, height: u16) -> Vec<u8> {
    let mut data = vec![0xFF, 0xD8, 0xFF, 0xC0, 0x00, 0x0B, 0x08];
    data.extend_from_slice(&height.to_be_bytes());
    data.extend_from_slice(&width.to_be_bytes());
    data.extend_from_slice(&[0x01, 0x01, 0x11, 0x00, 0xFF, 0xD9]);
    data
}

fn page() -> PageRequest {
    PageRequest::new(None, 10).unwrap()
}

fn orbbec_id() -> ImageSourceId {
    ImageSourceId::new(ORBBEC_CAMERA_SOURCE_ID).unwrap()
}

#[derive(Clone)]
struct Script {
    calls: Arc<Mutex<Vec<Option<String>>>>,
    result: Arc<Mutex<Result<Vec<u8>, OrbbecCaptureError>>>,
}

impl Script {
    fn ok(bytes: Vec<u8>) -> Self {
        Self::with(Ok(bytes))
    }

    fn fail(error: OrbbecCaptureError) -> Self {
        Self::with(Err(error))
    }

    fn with(result: Result<Vec<u8>, OrbbecCaptureError>) -> Self {
        Self {
            calls: Arc::new(Mutex::new(Vec::new())),
            result: Arc::new(Mutex::new(result)),
        }
    }

    fn calls(&self) -> Vec<Option<String>> {
        self.calls.lock().expect("calls").clone()
    }
}

impl OrbbecCapture for Script {
    fn capture_color(
        &self,
        serial: Option<&str>,
        _timeout: Duration,
        _max_bytes: u64,
    ) -> Result<Vec<u8>, OrbbecCaptureError> {
        self.calls
            .lock()
            .expect("calls")
            .push(serial.map(str::to_owned));
        self.result.lock().expect("result").clone()
    }
}

struct SlowCapture;

#[derive(Clone)]
struct Native {
    fail: bool,
}

impl CameraCapture for Native {
    fn capture(
        &self,
        _index: u32,
        _format: a2a_lab_ot2::camera::ExternalCameraFormat,
        _frames: u32,
    ) -> Result<Vec<u8>, CameraCaptureError> {
        if self.fail {
            Err(CameraCaptureError::Backend("external down".to_owned()))
        } else {
            Ok(jpeg(2, 2))
        }
    }
}

impl OrbbecCapture for SlowCapture {
    fn capture_color(
        &self,
        _serial: Option<&str>,
        _timeout: Duration,
        _max_bytes: u64,
    ) -> Result<Vec<u8>, OrbbecCaptureError> {
        std::thread::sleep(Duration::from_millis(200));
        Ok(jpeg(1, 1))
    }
}

fn config(orbbec: bool, serial: Option<&str>) -> ImageServeConfig {
    let config = ImageServeConfig::new(None, None, 1024, 500, 4).unwrap();
    if orbbec {
        config.with_orbbec(serial).unwrap()
    } else {
        config
    }
}

fn catalog(
    orbbec: bool,
    serial: Option<&str>,
    capture: impl OrbbecCapture + 'static,
) -> a2a_lab_ot2::LiveImageCatalog {
    image_catalog(
        "http://127.0.0.1:9",
        &config(orbbec, serial),
        None,
        Some(Arc::new(capture)),
    )
    .unwrap()
}

#[test]
fn conflicting_orbbec_storage_paths_are_rejected_before_serve() {
    let same = bin()
        .args([
            "--orbbec-camera",
            "--sync-db",
            "history.sqlite",
            "--frame-set-db",
            "history.sqlite",
            "list-image-sources",
        ])
        .env("A2ALAB_OPENTRONS_URL", "http://127.0.0.1:9")
        .output()
        .unwrap();
    assert!(!same.status.success());
    let err = String::from_utf8_lossy(&same.stderr);
    assert!(err.contains("must not be the same file"), "{err}");
}

#[test]
fn disabled_configuration_rejects_a_serial_and_does_not_enable_the_source() {
    let error = bin()
        .args(["--orbbec-serial", "ABC", "list-image-sources"])
        .output()
        .unwrap();
    assert!(!error.status.success());
    let err = String::from_utf8_lossy(&error.stderr);
    assert!(err.contains("requires Orbbec color capture"), "{err}");

    let empty = bin()
        .args(["--orbbec-camera", "--orbbec-serial", ""])
        .env("A2ALAB_OPENTRONS_URL", "http://127.0.0.1:9")
        .output()
        .unwrap();
    assert!(!empty.status.success());
    assert!(String::from_utf8_lossy(&empty.stderr).contains("must not be empty"));
}

#[test]
fn help_and_startup_keep_orbbec_off_unless_requested() {
    let help = bin().arg("--help").output().unwrap();
    let help_text = String::from_utf8(help.stdout).unwrap();
    assert!(help_text.contains("--orbbec-camera"));
    assert!(help_text.contains("A2ALAB_ORBBEC_CAMERA"));
    assert!(help_text.contains("--orbbec-serial"));
    assert!(help_text.contains("A2ALAB_ORBBEC_SERIAL"));
    assert!(help_text.contains("orbbec-sdk-version"));

    let off = bin()
        .env("A2ALAB_OPENTRONS_URL", "http://127.0.0.1:9")
        .output()
        .unwrap();
    let off_out = String::from_utf8_lossy(&off.stdout);
    assert!(off_out.contains("orbbec-camera off"));
    assert!(off_out.contains("external-camera off"));

    let on = bin()
        .env("A2ALAB_OPENTRONS_URL", "http://127.0.0.1:9")
        .env("A2ALAB_ORBBEC_SERIAL", "FROM-ENV")
        .args(["--orbbec-camera", "--orbbec-serial", "FROM-FLAG"])
        .output()
        .unwrap();
    let on_out = String::from_utf8_lossy(&on.stdout);
    assert!(on_out.contains("orbbec-camera serial FROM-FLAG"));
    assert!(!on_out.contains("FROM-ENV"));
}

#[test]
fn version_command_names_orbbec_when_the_sdk_is_absent() {
    let output = bin().arg("orbbec-sdk-version").output().unwrap();
    assert!(!output.status.success());
    let err = String::from_utf8_lossy(&output.stderr);
    assert!(err.contains("orbbec-camera"), "{err}");
    assert!(err.contains("libOrbbecSDK"), "{err}");
}

#[test]
fn listing_sources_omits_orbbec_until_it_is_enabled() {
    let off = bin().arg("list-image-sources").output().unwrap();
    assert!(off.status.success());
    let off_text = String::from_utf8(off.stdout).unwrap();
    assert!(off_text.contains("opentrons-camera"));
    assert!(!off_text.contains("orbbec-camera"));

    let on = bin()
        .args(["--orbbec-camera", "list-image-sources"])
        .output()
        .unwrap();
    assert!(on.status.success());
    let on_text = String::from_utf8(on.stdout).unwrap();
    assert_eq!(on_text.matches("orbbec-camera").count(), 1);
    assert!(on_text.contains("Orbbec camera"));
    assert!(on_text.contains("Still color frame from the configured Orbbec camera."));
}

#[tokio::test]
async fn enabled_discovery_does_not_open_a_device() {
    let capture = Script::ok(jpeg(2, 2));
    let images = catalog(true, None, capture.clone());
    let listed = images
        .list_image_sources(ListImageSourcesRequest::new(page()).unwrap())
        .await
        .unwrap();
    let ids: Vec<_> = listed.items().iter().map(|item| item.id.as_str()).collect();
    assert_eq!(
        ids,
        vec![OPENTRONS_CAMERA_SOURCE_ID, ORBBEC_CAMERA_SOURCE_ID]
    );
    assert!(capture.calls().is_empty());

    let disabled = catalog(false, None, Script::ok(jpeg(1, 1)));
    let disabled_page = disabled
        .list_image_sources(ListImageSourcesRequest::new(page()).unwrap())
        .await
        .unwrap();
    assert!(
        !disabled_page
            .items()
            .iter()
            .any(|item| item.id.as_str() == ORBBEC_CAMERA_SOURCE_ID)
    );
}

#[tokio::test]
async fn sole_device_and_serial_selection_store_one_jpeg() {
    let frame = jpeg(4, 5);
    let capture = Script::ok(frame.clone());
    let images = catalog(true, Some("ABC123"), capture.clone());
    let current = images
        .get_current_image(GetCurrentImageRequest::new(orbbec_id()))
        .await
        .unwrap();
    assert_eq!(current.data(), frame.as_slice());
    assert_eq!(current.descriptor().media_type(), "image/jpeg");
    assert_eq!(current.descriptor().width(), 4);
    assert_eq!(current.descriptor().height(), 5);
    assert_eq!(
        current.descriptor().source_id().as_str(),
        ORBBEC_CAMERA_SOURCE_ID
    );
    let stored = images
        .get_image(GetImageRequest::new(current.descriptor().id().clone()))
        .await
        .unwrap();
    assert_eq!(stored.data(), frame.as_slice());
    let listed = images
        .list_images(ListImagesRequest::new(orbbec_id(), page()).unwrap())
        .await
        .unwrap();
    assert_eq!(listed.items().len(), 1);
    assert_eq!(capture.calls(), vec![Some("ABC123".to_owned())]);

    let sole = Script::ok(frame);
    let images = catalog(true, None, sole.clone());
    images
        .get_current_image(GetCurrentImageRequest::new(orbbec_id()))
        .await
        .unwrap();
    assert_eq!(sole.calls(), vec![None]);
}

#[tokio::test]
async fn device_profile_and_payload_failures_store_nothing() {
    let cases = [
        (
            OrbbecCaptureError::NoDevice,
            "unavailable",
            "no Orbbec device is connected",
        ),
        (
            OrbbecCaptureError::MissingSerial("ZZZ".to_owned()),
            "unavailable",
            "serial ZZZ was not found",
        ),
        (
            OrbbecCaptureError::Ambiguous(vec!["A".to_owned(), "B".to_owned()]),
            "unavailable",
            "multiple Orbbec devices",
        ),
        (
            OrbbecCaptureError::Permission("libusb_error_access".to_owned()),
            "unavailable",
            "permission denied",
        ),
        (
            OrbbecCaptureError::NoColorProfile("no MJPEG".to_owned()),
            "unavailable",
            "no usable MJPEG color profile",
        ),
        (
            OrbbecCaptureError::Sdk("startup failed".to_owned()),
            "unavailable",
            "Orbbec SDK failed",
        ),
        (OrbbecCaptureError::Timeout, "unavailable", "timed out"),
        (OrbbecCaptureError::Empty, "invalid", "empty frame"),
        (
            OrbbecCaptureError::Malformed("truncated".to_owned()),
            "invalid",
            "malformed JPEG",
        ),
        (
            OrbbecCaptureError::Oversize,
            "invalid",
            "configured maximum",
        ),
    ];
    for (error, code, text) in cases {
        let images = catalog(true, Some("ZZZ"), Script::fail(error));
        let failed = images
            .get_current_image(GetCurrentImageRequest::new(orbbec_id()))
            .await
            .unwrap_err();
        assert_eq!(failed.code(), code, "{failed}");
        let rendered = failed.to_string();
        assert!(rendered.contains("orbbec-camera"), "{rendered}");
        assert!(rendered.contains("ZZZ"), "{rendered}");
        assert!(rendered.contains(text), "{rendered}");
        let listed = images
            .list_images(ListImagesRequest::new(orbbec_id(), page()).unwrap())
            .await
            .unwrap();
        assert!(listed.items().is_empty(), "{rendered}");
    }

    let images = catalog(true, None, Script::ok(b"not-a-jpeg".to_vec()));
    let failed = images
        .get_current_image(GetCurrentImageRequest::new(orbbec_id()))
        .await
        .unwrap_err();
    assert_eq!(failed.code(), "invalid");
    assert!(failed.to_string().contains("malformed JPEG"));

    let tight = ImageServeConfig::new(None, None, 8, 500, 4)
        .unwrap()
        .with_orbbec(None)
        .unwrap();
    let images = image_catalog(
        "http://127.0.0.1:9",
        &tight,
        None,
        Some(Arc::new(Script::ok(jpeg(8, 8)))),
    )
    .unwrap();
    let failed = images
        .get_current_image(GetCurrentImageRequest::new(orbbec_id()))
        .await
        .unwrap_err();
    assert_eq!(failed.code(), "invalid");
    assert!(failed.to_string().contains("configured maximum"));
}

#[tokio::test]
async fn capture_timeout_and_other_sources_stay_isolated() {
    let images = OrbbecCamera::with_capture(None, Duration::from_millis(20), 1024, SlowCapture)
        .unwrap()
        .catalog(2)
        .unwrap();
    let failed = images
        .get_current_image(GetCurrentImageRequest::new(orbbec_id()))
        .await
        .unwrap_err();
    assert_eq!(failed.code(), "unavailable");
    assert!(failed.to_string().contains("orbbec-camera: timed out"));

    let config = ImageServeConfig::new(Some(3), None, 1024, 500, 4)
        .unwrap()
        .with_orbbec(Some("ONLY"))
        .unwrap();
    let images = image_catalog(
        "http://127.0.0.1:9",
        &config,
        Some(Arc::new(Native { fail: true })),
        Some(Arc::new(Script::ok(jpeg(2, 2)))),
    )
    .unwrap();
    let orbbec = images
        .get_current_image(GetCurrentImageRequest::new(orbbec_id()))
        .await
        .unwrap();
    assert_eq!(
        orbbec.descriptor().source_id().as_str(),
        ORBBEC_CAMERA_SOURCE_ID
    );
    let external = images
        .get_current_image(GetCurrentImageRequest::new(
            ImageSourceId::new(EXTERNAL_CAMERA_SOURCE_ID).unwrap(),
        ))
        .await
        .unwrap_err();
    assert_eq!(external.code(), "unavailable");
    assert!(external.to_string().contains("external camera index 3"));
    assert!(
        !external.to_string().contains("orbbec-camera"),
        "{external}"
    );
}

#[tokio::test]
async fn stored_orbbec_frame_survives_reopen() {
    let dir =
        std::env::temp_dir().join(format!("a2a-lab-ot2-orbbec-history-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("images.sqlite");
    let frame = jpeg(2, 3);
    let config = config(true, None);
    let sources = [
        ImageSourceId::new(OPENTRONS_CAMERA_SOURCE_ID).unwrap(),
        orbbec_id(),
    ];
    let images = image_catalog(
        "http://127.0.0.1:9",
        &config,
        None,
        Some(Arc::new(Script::ok(frame.clone()))),
    )
    .unwrap()
    .with_persisted_sources(&sources, &path)
    .await
    .unwrap();
    let current = images
        .get_current_image(GetCurrentImageRequest::new(orbbec_id()))
        .await
        .unwrap();
    let id = current.descriptor().id().clone();
    drop(images);

    let restored = image_catalog(
        "http://127.0.0.1:9",
        &config,
        None,
        Some(Arc::new(Script::fail(OrbbecCaptureError::NoDevice))),
    )
    .unwrap()
    .with_persisted_sources(&sources, &path)
    .await
    .unwrap();
    let stored = restored.get_image(GetImageRequest::new(id)).await.unwrap();
    assert_eq!(stored.data(), frame.as_slice());
    std::fs::remove_dir_all(dir).ok();
}
