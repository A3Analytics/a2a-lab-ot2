//! Image catalog wiring for configuration-driven native video source ids.

use a2a_lab_dev_kit::{ImageProvider, ListImageSourcesRequest, PageRequest};
use a2a_lab_ot2::ImageCapture;
use a2a_lab_ot2::camera::{PumpLinkedStill, remember_live_still};
use a2a_lab_ot2::{
    ExplicitSetting, ImageServeConfig, LabConfigInputs, image_catalog, resolve_lab_config,
};

fn fixture_path() -> std::path::PathBuf {
    std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("fixtures/native-macos-media-smoke-v1.json")
}

fn jpeg() -> Vec<u8> {
    let mut data = vec![0xFF, 0xD8, 0xFF, 0xC0, 0x00, 0x0B, 0x08];
    data.extend_from_slice(&4_u16.to_be_bytes());
    data.extend_from_slice(&4_u16.to_be_bytes());
    data.extend_from_slice(&[0x01, 0x01, 0x11, 0x00, 0xFF, 0xD9]);
    data
}

#[tokio::test]
async fn native_macos_fixture_registers_custom_image_sources() {
    let inputs = LabConfigInputs {
        config_path: ExplicitSetting::cli(Some(fixture_path())),
        ..LabConfigInputs::default()
    };
    let resolved = resolve_lab_config(&inputs).unwrap();
    let config = ImageServeConfig::new(None, None, 64 * 1024, 10_000, 30)
        .unwrap()
        .with_native_video_sources(&resolved.video.sources);
    let catalog = image_catalog("http://127.0.0.1:31950", &config, None, None).unwrap();
    let page = catalog
        .list_image_sources(
            ListImageSourcesRequest::new(PageRequest::new(None, 20).unwrap()).unwrap(),
        )
        .await
        .unwrap();
    let ids = page
        .items()
        .iter()
        .map(|item| item.id.as_str().to_owned())
        .collect::<Vec<_>>();
    assert!(ids.contains(&"mac-built-in".to_owned()));
    assert!(!ids.contains(&"usb-webcam".to_owned()));
    assert!(!ids.contains(&"camera".to_owned()));
}

#[tokio::test]
async fn custom_source_stills_are_isolated() {
    a2a_lab_ot2::camera::clear_all_live_stills();
    let built_in = PumpLinkedStill::new("mac-built-in", "Mac built-in", 64 * 1024).unwrap();
    let usb = PumpLinkedStill::new("usb-webcam", "USB webcam", 64 * 1024).unwrap();
    let frame = jpeg();
    remember_live_still("mac-built-in", frame.clone());
    let built_in_frame = built_in.capture().await.expect("built-in still");
    assert_eq!(built_in_frame.data(), frame.as_slice());
    let usb_err = usb.capture().await.unwrap_err().to_string();
    assert!(usb_err.contains("usb-webcam"));
    assert!(usb_err.contains("no frame yet"));

    remember_live_still("usb-webcam", jpeg());
    usb.capture().await.expect("usb still");

    let built_in_current = built_in.capture().await.expect("built-in still after usb");
    assert_eq!(built_in_current.data(), frame.as_slice());
}
