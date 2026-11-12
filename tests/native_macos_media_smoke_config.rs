//! Local native macOS media smoke fixture: custom source ids and 3 h DVR retention.

use std::path::PathBuf;

use a2a_lab_dev_kit::{ImageProvider, ListImageSourcesRequest, PageRequest};
use a2a_lab_ot2::{
    ExplicitSetting, ImageServeConfig, LabConfigInputs, image_catalog, resolve_lab_config,
    uses_resolved_video_configuration,
};

const FIXTURE: &str = "fixtures/native-macos-media-smoke-v1.json";

fn fixture_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(FIXTURE)
}

fn inputs_for_fixture() -> LabConfigInputs {
    LabConfigInputs {
        config_path: ExplicitSetting::cli(Some(fixture_path())),
        ..LabConfigInputs::default()
    }
}

#[test]
fn native_macos_media_smoke_config_resolves() {
    let inputs = inputs_for_fixture();
    assert!(uses_resolved_video_configuration(&inputs).unwrap());
    let resolved = resolve_lab_config(&inputs).unwrap();
    assert_eq!(resolved.video.disk_retention_secs, 10_800);
    assert_eq!(resolved.video.sources.len(), 1);
    let ids = resolved
        .video
        .sources
        .iter()
        .map(|source| source.id.as_str())
        .collect::<Vec<_>>();
    assert_eq!(ids, ["mac-built-in"]);
    assert!(
        ids.iter()
            .all(|id| *id != "camera" && *id != "external-camera")
    );
    assert_eq!(resolved.video.fps, 5);
    assert_eq!(resolved.video.sources[0].name, "Mac built-in");
    assert_eq!(
        resolved.video.sources[0].input,
        a2a_lab_ot2::LiveCameraInputFormat::Yuy2
    );
    assert_eq!(resolved.video.sources[0].device_index, 0);
    assert_eq!(resolved.video.sources[0].width, 1280);
    assert_eq!(resolved.video.sources[0].height, 720);
}

#[tokio::test]
async fn native_macos_fixture_wires_image_inventory() {
    let inputs = inputs_for_fixture();
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
    let ids: Vec<_> = page.items().iter().map(|item| item.id.as_str()).collect();
    assert!(ids.contains(&"mac-built-in"));
    assert!(!ids.contains(&"usb-webcam"));
}

#[test]
fn native_macos_smoke_leaves_shared_disk_default_at_three_days() {
    assert_eq!(
        a2a_lab_ot2::DEFAULT_DISK_VIDEO_RETENTION_SECS,
        3 * 24 * 60 * 60
    );
}
