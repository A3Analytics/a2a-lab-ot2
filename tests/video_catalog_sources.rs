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
    start_live_video_catalog,
};

fn jpeg(width: u16, height: u16) -> Vec<u8> {
    let mut data = vec![0xFF, 0xD8, 0xFF, 0xC0, 0x00, 0x0B, 0x08];
    data.extend_from_slice(&height.to_be_bytes());
    data.extend_from_slice(&width.to_be_bytes());
    data.extend_from_slice(&[0x01, 0x01, 0x11, 0x00, 0xFF, 0xD9]);
    data
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
    LiveVideoSourceDescriptor {
        id: id.to_owned(),
        name: format!("Name {id}"),
        description: format!("Description {id}"),
        device_index,
        input,
        width: 4,
        height: 4,
        delivery: LiveVideoDelivery::Mjpeg,
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

async fn wait_for_live(provider: &a2a_lab_ot2::VideoDvrCatalog, id: &str) {
    let source = VideoSourceId::new(id).unwrap();
    for _ in 0..50 {
        if provider
            .follow_video(FollowVideoRequest::new(source.clone()))
            .await
            .is_ok()
        {
            return;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    panic!("live follow never became available for {id}");
}

#[tokio::test]
async fn empty_catalog_lists_no_sources() {
    let catalog = start_live_video_catalog(
        &[],
        5,
        5,
        86_400,
        &temp_dvr("empty"),
        1024,
        LiveVideoInjections::none(),
    )
    .await
    .unwrap();
    let page = catalog
        .provider()
        .list_video_sources(ListVideoSourcesRequest::new(page()).unwrap())
        .await
        .unwrap();
    assert!(page.items().is_empty());
    catalog.shutdown();
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
    let catalog = start_live_video_catalog(
        &sources,
        5,
        5,
        86_400,
        &temp_dvr("three"),
        64 * 1024,
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
    wait_for_live(&provider, "alpha").await;
    wait_for_live(&provider, "beta").await;
    wait_for_live(&provider, "gamma").await;
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
        start_live_video_catalog(
            &dup_id,
            5,
            5,
            86_400,
            &temp_dvr("dup-id"),
            1024,
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
        start_live_video_catalog(
            &dup_device,
            5,
            5,
            86_400,
            &temp_dvr("dup-dev"),
            1024,
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
        let catalog = start_live_video_catalog(
            &order,
            5,
            5,
            86_400,
            &temp_dvr("order"),
            1024,
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
    let catalog = start_live_video_catalog(
        &sources,
        5,
        5,
        86_400,
        &temp_dvr("isolate"),
        1024,
        LiveVideoInjections::legacy_jpeg(
            Some(Box::new(FailJpeg)),
            Some(Box::new(RepeatJpeg(jpeg(4, 4)))),
        ),
    )
    .await
    .unwrap();
    let provider = catalog.provider();
    wait_for_live(&provider, "camera").await;
    let page = provider
        .list_video_sources(ListVideoSourcesRequest::new(page()).unwrap())
        .await
        .unwrap();
    assert_eq!(page.items().len(), 2);
    catalog.shutdown();
}

#[tokio::test]
async fn color_camera_rejects_wrong_negotiated_format() {
    let sources = [descriptor("mjpeg-only", 3, LiveCameraInputFormat::Mjpeg)];
    let camera = ScriptCamera {
        formats: vec![profile(CaptureFormat::Yuy2, 4, 4, 5)],
        raw: yuy2_sample(4, 4, 8, 1),
        negotiated: SelectedCaptureProfile::negotiated(
            CaptureMode::new(CaptureFormat::Yuy2, 4, 4).expect("mode"),
            CaptureRate::fps(5).expect("rate"),
            CaptureRate::fps(5).expect("out"),
        ),
    };
    let catalog = start_live_video_catalog(
        &sources,
        5,
        5,
        86_400,
        &temp_dvr("wrong-fmt"),
        1024,
        LiveVideoInjections::none().with_color_camera("mjpeg-only", Box::new(camera)),
    )
    .await
    .unwrap();
    tokio::time::sleep(Duration::from_millis(200)).await;
    let source = VideoSourceId::new("mjpeg-only").unwrap();
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
    let catalog = start_live_video_catalog(
        &sources,
        5,
        5,
        86_400,
        &temp_dvr("bad-yuy2"),
        1024,
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
    let catalog = start_live_video_catalog(
        &sources,
        5,
        5,
        86_400,
        &temp_dvr("yuy2-live"),
        64 * 1024,
        LiveVideoInjections::none().with_color_camera("raw-yuy2", Box::new(camera)),
    )
    .await
    .unwrap();
    wait_for_live(&catalog.provider(), "raw-yuy2").await;
    catalog.shutdown();
}
