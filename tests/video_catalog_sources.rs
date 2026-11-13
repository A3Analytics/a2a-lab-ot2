//! Descriptor-list catalog, validation, and injected `ColorCamera` capture.

use std::path::PathBuf;
use std::time::Duration;

use a2a_lab_dev_kit::videos::{
    AdvertisedCaptureProfile, CaptureFormat, CaptureMode, CaptureRate, ColorCamera,
    RawCameraSample, SelectedCaptureProfile,
};
use a2a_lab_dev_kit::{
    A2aLabError, FollowVideoRequest, ListVideoSourcesRequest, PageRequest, VideoProvider,
    VideoSourceId,
};
use a2a_lab_ot2::{
    LiveCameraInputFormat, LiveVideoDelivery, LiveVideoInjections, LiveVideoSourceDescriptor,
    start_fixture_declarative_serve,
};

fn jpeg(width: u16, height: u16) -> Vec<u8> {
    use image::codecs::jpeg::JpegEncoder;
    use image::{ExtendedColorType, ImageBuffer, ImageEncoder, Rgb};
    let image: ImageBuffer<Rgb<u8>, Vec<u8>> =
        ImageBuffer::from_fn(u32::from(width), u32::from(height), |_, _| {
            Rgb([64, 64, 64])
        });
    let mut out = Vec::new();
    JpegEncoder::new_with_quality(&mut out, 80)
        .write_image(
            image.as_raw(),
            u32::from(width),
            u32::from(height),
            ExtendedColorType::Rgb8,
        )
        .expect("jpeg");
    out
}

fn temp_dvr(name: &str) -> PathBuf {
    std::env::temp_dir().join(format!("a2a-lab-ot2-catalog-{name}-{}", std::process::id()))
}

fn page() -> PageRequest {
    PageRequest::new(None, 20).unwrap()
}

fn descriptor(
    id: &str,
    device_index: u32,
    input: LiveCameraInputFormat,
) -> LiveVideoSourceDescriptor {
    let (camera_id, device_index) = a2a_lab_ot2::index_camera_fields(device_index);
    LiveVideoSourceDescriptor {
        id: id.to_owned(),
        name: format!("Name {id}"),
        description: format!("Description {id}"),
        camera_id,
        device_index,
        input,
        width: Some(4),
        height: Some(4),
        capture_rate: Some(5),
        delivery: LiveVideoDelivery::H264,
    }
}

struct RepeatJpeg(Vec<u8>);

impl a2a_lab_ot2::LiveJpegSource for RepeatJpeg {
    fn next_jpeg(&mut self) -> Result<Vec<u8>, A2aLabError> {
        Ok(self.0.clone())
    }
}

struct FailJpeg;

impl a2a_lab_ot2::LiveJpegSource for FailJpeg {
    fn next_jpeg(&mut self) -> Result<Vec<u8>, A2aLabError> {
        Err(A2aLabError::unavailable("injected failure"))
    }
}

struct ScriptCamera {
    formats: Vec<AdvertisedCaptureProfile>,
    raw: RawCameraSample,
    negotiated: SelectedCaptureProfile,
}

impl ColorCamera for ScriptCamera {
    fn advertised(&mut self) -> Result<Vec<AdvertisedCaptureProfile>, A2aLabError> {
        Ok(self.formats.clone())
    }

    fn open(
        &mut self,
        profile: &SelectedCaptureProfile,
    ) -> Result<SelectedCaptureProfile, A2aLabError> {
        assert_eq!(profile.mode().format(), self.negotiated.mode().format());
        Ok(self.negotiated)
    }

    fn raw_frame(&mut self) -> Result<RawCameraSample, A2aLabError> {
        Ok(self.raw.clone())
    }

    fn stop(&mut self) -> Result<(), A2aLabError> {
        Ok(())
    }
}

fn profile(format: CaptureFormat, width: u32, height: u32, fps: u32) -> AdvertisedCaptureProfile {
    AdvertisedCaptureProfile::new(
        CaptureMode::new(format, width, height).expect("mode"),
        CaptureRate::fps(fps).expect("rate"),
    )
}

fn yuy2_sample(width: u32, height: u32, stride: u32, fill: u8) -> RawCameraSample {
    let rows = u64::from(height) * u64::from(stride);
    RawCameraSample::new(
        CaptureFormat::Yuy2,
        width,
        height,
        stride,
        vec![fill; usize::try_from(rows).unwrap_or(usize::MAX)],
    )
}

async fn wait_for_runtime(catalog: &a2a_lab_ot2::DeclarativeVideoServe, id: &str) {
    for _ in 0..400 {
        if catalog.still_cache().latest(id).is_some() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    panic!("runtime never published still for {id}");
}

#[tokio::test]
async fn empty_catalog_lists_no_sources() {
    use a2a_lab_ot2::lab_config::{ResolvedDvrStorage, ResolvedVideoSettings, SettingProvenance};
    use a2a_lab_ot2::open_resolved_video_catalog;
    let settings = ResolvedVideoSettings {
        sources: vec![],
        fps: 5,
        live_retention_secs: 5,
        disk_retention_secs: 86_400,
        dvr_save_interval_nanos: None,
        dvr_storage: ResolvedDvrStorage {
            path: temp_dvr("empty"),
            provenance: SettingProvenance::ApplicationDefault,
        },
        max_image_bytes: 1024,
    };
    let catalog = open_resolved_video_catalog(&settings, LiveVideoInjections::none())
        .await
        .unwrap();
    assert!(catalog.is_none());
}

#[tokio::test]
async fn three_injected_sources_are_discoverable() {
    let sources = [
        descriptor("alpha", 0, LiveCameraInputFormat::Mjpeg),
        descriptor("beta", 1, LiveCameraInputFormat::Yuy2),
        descriptor("gamma", 2, LiveCameraInputFormat::Mjpeg),
    ];
    let frame = jpeg(4, 4);
    let negotiated_mjpeg = SelectedCaptureProfile::negotiated(
        CaptureMode::new(CaptureFormat::Mjpeg, 4, 4).expect("mode"),
        CaptureRate::fps(5).expect("rate"),
        CaptureRate::fps(5).expect("out"),
    );
    let negotiated_yuy2 = SelectedCaptureProfile::negotiated(
        CaptureMode::new(CaptureFormat::Yuy2, 4, 4).expect("mode"),
        CaptureRate::fps(5).expect("rate"),
        CaptureRate::fps(5).expect("out"),
    );
    let catalog = start_fixture_declarative_serve(
        sources.to_vec(),
        5,
        5,
        86_400,
        &temp_dvr("three"),
        8 * 1024 * 1024,
        None,
        LiveVideoInjections::none()
            .with_color_camera(
                "alpha",
                Box::new(ScriptCamera {
                    formats: vec![profile(CaptureFormat::Mjpeg, 4, 4, 5)],
                    raw: RawCameraSample::new(CaptureFormat::Mjpeg, 4, 4, 0, frame.clone()),
                    negotiated: negotiated_mjpeg,
                }),
            )
            .with_color_camera(
                "beta",
                Box::new(ScriptCamera {
                    formats: vec![profile(CaptureFormat::Yuy2, 4, 4, 5)],
                    raw: yuy2_sample(4, 4, 8, 9),
                    negotiated: negotiated_yuy2,
                }),
            )
            .with_color_camera(
                "gamma",
                Box::new(ScriptCamera {
                    formats: vec![profile(CaptureFormat::Mjpeg, 4, 4, 5)],
                    raw: RawCameraSample::new(CaptureFormat::Mjpeg, 4, 4, 0, frame),
                    negotiated: negotiated_mjpeg,
                }),
            ),
    )
    .await
    .unwrap();
    let provider = catalog.provider();
    wait_for_runtime(&catalog, "alpha").await;
    wait_for_runtime(&catalog, "beta").await;
    wait_for_runtime(&catalog, "gamma").await;
    let page = provider
        .list_video_sources(ListVideoSourcesRequest::new(page()).unwrap())
        .await
        .unwrap();
    assert_eq!(page.items().len(), 3);
    let mut ids = page
        .items()
        .iter()
        .map(|item| item.id().as_str().to_owned())
        .collect::<Vec<_>>();
    ids.sort();
    assert_eq!(ids, vec!["alpha", "beta", "gamma"]);
    let alpha = page
        .items()
        .iter()
        .find(|s| s.id().as_str() == "alpha")
        .unwrap();
    assert_eq!(alpha.name(), "Name alpha");
    assert_eq!(alpha.description(), "Description alpha");
    catalog.shutdown();
}

#[tokio::test]
async fn rejects_duplicate_ids_and_device_indices() {
    let dup_id = [
        descriptor("same", 0, LiveCameraInputFormat::Mjpeg),
        descriptor("same", 1, LiveCameraInputFormat::Yuy2),
    ];
    assert!(
        start_fixture_declarative_serve(
            dup_id.to_vec(),
            5,
            5,
            86_400,
            &temp_dvr("dup-id"),
            8 * 1024 * 1024,
            None,
            LiveVideoInjections::none(),
        )
        .await
        .is_err()
    );
    let dup_device = [
        descriptor("a", 2, LiveCameraInputFormat::Mjpeg),
        descriptor("b", 2, LiveCameraInputFormat::Yuy2),
    ];
    assert!(
        start_fixture_declarative_serve(
            dup_device.to_vec(),
            5,
            5,
            86_400,
            &temp_dvr("dup-dev"),
            8 * 1024 * 1024,
            None,
            LiveVideoInjections::none(),
        )
        .await
        .is_err()
    );
}

#[tokio::test]
async fn reordered_descriptors_keep_identity() {
    let first = [
        LiveVideoSourceDescriptor::legacy_external(0, "first"),
        LiveVideoSourceDescriptor::legacy_camera(1),
    ];
    let second = [
        LiveVideoSourceDescriptor::legacy_camera(1),
        LiveVideoSourceDescriptor::legacy_external(0, "first"),
    ];
    let frame = jpeg(4, 4);
    for order in [first, second] {
        let catalog = start_fixture_declarative_serve(
            order.to_vec(),
            5,
            5,
            86_400,
            &temp_dvr("order"),
            8 * 1024 * 1024,
            None,
            LiveVideoInjections::legacy_jpeg(
                Some(Box::new(RepeatJpeg(frame.clone()))),
                Some(Box::new(RepeatJpeg(frame.clone()))),
            ),
        )
        .await
        .unwrap();
        let page = catalog
            .provider()
            .list_video_sources(ListVideoSourcesRequest::new(page()).unwrap())
            .await
            .unwrap();
        let mut ids = page
            .items()
            .iter()
            .map(|item| item.id().as_str().to_owned())
            .collect::<Vec<_>>();
        ids.sort();
        assert_eq!(ids, vec!["camera", "external-camera"]);
        catalog.shutdown();
    }
}

#[tokio::test]
async fn one_failing_source_does_not_remove_healthy_sources() {
    let sources = [
        LiveVideoSourceDescriptor::legacy_external(0, "sick"),
        LiveVideoSourceDescriptor::legacy_camera(1),
    ];
    let catalog = start_fixture_declarative_serve(
        sources.to_vec(),
        5,
        5,
        86_400,
        &temp_dvr("isolate"),
        8 * 1024 * 1024,
        None,
        LiveVideoInjections::legacy_jpeg(
            Some(Box::new(FailJpeg)),
            Some(Box::new(RepeatJpeg(jpeg(4, 4)))),
        ),
    )
    .await
    .unwrap();
    let provider = catalog.provider();
    wait_for_runtime(&catalog, "camera").await;
    let page = provider
        .list_video_sources(ListVideoSourcesRequest::new(page()).unwrap())
        .await
        .unwrap();
    assert_eq!(page.items().len(), 2);
    catalog.shutdown();
}

#[tokio::test]
async fn color_camera_rejects_wrong_negotiated_format() {
    let sources = [LiveVideoSourceDescriptor::legacy_camera(3)];
    let camera = ScriptCamera {
        formats: vec![profile(CaptureFormat::Yuy2, 4, 4, 5)],
        raw: yuy2_sample(4, 4, 8, 1),
        negotiated: SelectedCaptureProfile::negotiated(
            CaptureMode::new(CaptureFormat::Yuy2, 4, 4).expect("mode"),
            CaptureRate::fps(5).expect("rate"),
            CaptureRate::fps(5).expect("out"),
        ),
    };
    let catalog = start_fixture_declarative_serve(
        sources.to_vec(),
        5,
        5,
        86_400,
        &temp_dvr("wrong-fmt"),
        8 * 1024 * 1024,
        None,
        LiveVideoInjections::none().with_color_camera("camera", Box::new(camera)),
    )
    .await
    .unwrap();
    tokio::time::sleep(Duration::from_millis(200)).await;
    let source = VideoSourceId::new("camera").unwrap();
    assert!(
        catalog
            .provider()
            .follow_video(FollowVideoRequest::new(source))
            .await
            .is_err()
    );
    catalog.shutdown();
}

#[tokio::test]
async fn malformed_packed_yuy2_fails_without_fallback() {
    let sources = [descriptor("yuy2", 4, LiveCameraInputFormat::Yuy2)];
    let camera = ScriptCamera {
        formats: vec![profile(CaptureFormat::Yuy2, 4, 4, 5)],
        raw: RawCameraSample::new(CaptureFormat::Yuy2, 4, 4, 8, vec![0; 3]),
        negotiated: SelectedCaptureProfile::negotiated(
            CaptureMode::new(CaptureFormat::Yuy2, 4, 4).expect("mode"),
            CaptureRate::fps(5).expect("rate"),
            CaptureRate::fps(5).expect("out"),
        ),
    };
    let catalog = start_fixture_declarative_serve(
        sources.to_vec(),
        5,
        5,
        86_400,
        &temp_dvr("bad-yuy2"),
        8 * 1024 * 1024,
        None,
        LiveVideoInjections::none().with_color_camera("yuy2", Box::new(camera)),
    )
    .await
    .unwrap();
    tokio::time::sleep(Duration::from_millis(200)).await;
    let source = VideoSourceId::new("yuy2").unwrap();
    assert!(
        catalog
            .provider()
            .follow_video(FollowVideoRequest::new(source))
            .await
            .is_err()
    );
    catalog.shutdown();
}

#[tokio::test]
async fn yuy2_injected_source_publishes_live_mjpeg() {
    let sources = [descriptor("raw-yuy2", 0, LiveCameraInputFormat::Yuy2)];
    let camera = ScriptCamera {
        formats: vec![profile(CaptureFormat::Yuy2, 4, 4, 5)],
        raw: yuy2_sample(4, 4, 8, 42),
        negotiated: SelectedCaptureProfile::negotiated(
            CaptureMode::new(CaptureFormat::Yuy2, 4, 4).expect("mode"),
            CaptureRate::fps(5).expect("rate"),
            CaptureRate::fps(5).expect("out"),
        ),
    };
    let catalog = start_fixture_declarative_serve(
        sources.to_vec(),
        5,
        5,
        86_400,
        &temp_dvr("yuy2-live"),
        64 * 1024,
        None,
        LiveVideoInjections::none().with_color_camera("raw-yuy2", Box::new(camera)),
    )
    .await
    .unwrap();
    wait_for_runtime(&catalog, "raw-yuy2").await;
    catalog.shutdown();
}
