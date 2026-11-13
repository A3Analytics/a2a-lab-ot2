//! Injected MJPEG and YUY2 color sources through the shared `OpenH264` path.

use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use a2a_lab_dev_kit::videos::{
    AdvertisedCaptureProfile, CaptureFormat, CaptureMode, CaptureRate, RawCameraSample,
    SelectedCaptureProfile,
};
use a2a_lab_dev_kit::{
    A2aLabError, DvrFrameAtRequest, DvrMediaPayload, DvrMediaStore, DvrTrackId, VideoCodec,
    VideoSourceId,
};
use a2a_lab_ot2::{
    LiveCameraInputFormat, LiveVideoDelivery, LiveVideoInjections, LiveVideoSourceDescriptor,
    start_fixture_declarative_serve,
};

fn temp_dvr(name: &str) -> PathBuf {
    std::env::temp_dir().join(format!(
        "a2a-lab-ot2-live-h264-{name}-{}",
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

fn profile(format: CaptureFormat, fps: u32) -> AdvertisedCaptureProfile {
    AdvertisedCaptureProfile::new(
        CaptureMode::new(format, 16, 16).unwrap(),
        CaptureRate::fps(fps).unwrap(),
    )
}

struct ScriptCam {
    format: CaptureFormat,
    negotiated: SelectedCaptureProfile,
    advertised: Vec<AdvertisedCaptureProfile>,
    frames: Vec<Vec<u8>>,
    index: AtomicU64,
}

impl ScriptCam {
    fn mjpeg(count: usize) -> Self {
        let format = CaptureFormat::Mjpeg;
        Self::new(
            format,
            (0..count)
                .map(|index| tiny_jpeg(u8::try_from(index).unwrap_or(255)))
                .collect(),
        )
    }

    fn yuy2(count: usize) -> Self {
        let mut frames = Vec::with_capacity(count);
        for _ in 0..count {
            let mut packed = Vec::with_capacity(16 * 16 * 2);
            for _ in 0..(16 * 16 / 2) {
                packed.extend_from_slice(&[40, 128, 40, 128]);
            }
            frames.push(packed);
        }
        Self::new(CaptureFormat::Yuy2, frames)
    }

    fn new(format: CaptureFormat, frames: Vec<Vec<u8>>) -> Self {
        let negotiated = SelectedCaptureProfile::negotiated(
            CaptureMode::new(format, 16, 16).unwrap(),
            CaptureRate::fps(30).unwrap(),
            CaptureRate::fps(5).unwrap(),
        );
        Self {
            format,
            negotiated,
            advertised: vec![profile(format, 30)],
            frames,
            index: AtomicU64::new(0),
        }
    }
}

impl a2a_lab_dev_kit::videos::ColorCamera for ScriptCam {
    fn advertised(&mut self) -> Result<Vec<AdvertisedCaptureProfile>, A2aLabError> {
        Ok(self.advertised.clone())
    }

    fn open(
        &mut self,
        profile: &SelectedCaptureProfile,
    ) -> Result<SelectedCaptureProfile, A2aLabError> {
        assert_eq!(profile.mode().format(), self.format);
        Ok(self.negotiated)
    }

    fn raw_frame(&mut self) -> Result<RawCameraSample, A2aLabError> {
        let index = usize::try_from(self.index.fetch_add(1, Ordering::SeqCst)).unwrap_or(0);
        let bytes = self
            .frames
            .get(index)
            .cloned()
            .unwrap_or_else(|| self.frames.last().cloned().unwrap_or_default());
        let stride = if self.format == CaptureFormat::Yuy2 {
            32
        } else {
            0
        };
        Ok(RawCameraSample::new(self.format, 16, 16, stride, bytes))
    }

    fn stop(&mut self) -> Result<(), A2aLabError> {
        Ok(())
    }
}

fn h264_source(id: &str, input: LiveCameraInputFormat) -> LiveVideoSourceDescriptor {
    let (camera_id, device_index) = a2a_lab_ot2::index_camera_fields(1);
    LiveVideoSourceDescriptor {
        id: id.to_owned(),
        name: format!("Name {id}"),
        description: format!("Description {id}"),
        camera_id,
        device_index,
        input,
        width: Some(16),
        height: Some(16),
        capture_rate: Some(30),
        delivery: LiveVideoDelivery::H264,
    }
}

async fn wait_h264_disk(
    catalog: &a2a_lab_ot2::DeclarativeVideoServe,
    source: &VideoSourceId,
) -> (Vec<u8>, Vec<u8>) {
    let track = DvrTrackId::new("h264").unwrap();
    for _ in 0..80 {
        if let Ok(frame) = catalog
            .dvr()
            .frame_at(DvrFrameAtRequest::new(
                source.clone(),
                track.clone(),
                a2a_lab_ot2::time::now().unwrap(),
                Some(60_000_000),
            ))
            .await
            && let DvrMediaPayload::EncodedSample { bytes, .. } = frame.payload()
            && !bytes.is_empty()
        {
            return (Vec::new(), bytes.clone());
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    panic!("expected H.264 DVR sample");
}

#[tokio::test]
async fn mjpeg_fixture_emits_openh264_live_and_disk() {
    let root = temp_dvr("mjpeg");
    let _ = std::fs::remove_dir_all(&root);
    let id = "mjpeg-h264";
    let source = h264_source(id, LiveCameraInputFormat::Mjpeg);
    let catalog = start_fixture_declarative_serve(
        vec![source],
        5,
        5,
        3_600,
        &root,
        64 * 1024,
        None,
        LiveVideoInjections::none().with_color_camera(id, Box::new(ScriptCam::mjpeg(48))),
    )
    .await
    .unwrap();
    let source_id = VideoSourceId::new(id).unwrap();
    let (_init, sample) = wait_h264_disk(&catalog, &source_id).await;
    assert!(!sample.is_empty());
    let live = catalog
        .provider()
        .live_h264()
        .get(&source_id)
        .expect("live store");
    let snapshot = live.inner().snapshot().unwrap();
    assert!(!snapshot.epochs().is_empty());
    assert!(!snapshot.samples().is_empty());
    catalog.shutdown();
    let _ = std::fs::remove_dir_all(&root);
}

#[tokio::test]
async fn yuy2_fixture_emits_openh264_live_and_disk() {
    let root = temp_dvr("yuy2");
    let _ = std::fs::remove_dir_all(&root);
    let id = "yuy2-h264";
    let source = h264_source(id, LiveCameraInputFormat::Yuy2);
    let catalog = start_fixture_declarative_serve(
        vec![source],
        5,
        5,
        3_600,
        &root,
        64 * 1024,
        None,
        LiveVideoInjections::none().with_color_camera(id, Box::new(ScriptCam::yuy2(48))),
    )
    .await
    .unwrap();
    let source_id = VideoSourceId::new(id).unwrap();
    let (_init, sample) = wait_h264_disk(&catalog, &source_id).await;
    assert!(!sample.is_empty());
    let tracks = catalog.dvr().list_tracks(&source_id).await.unwrap();
    assert!(tracks.iter().any(|track| track.codec() == VideoCodec::H264));
    catalog.shutdown();
    let _ = std::fs::remove_dir_all(&root);
}
