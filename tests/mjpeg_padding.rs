//! Hardware-free proof that padded MJPEG is normalized before every consumer.

use std::path::PathBuf;
use std::time::Duration;

use a2a_lab_dev_kit::videos::{
    AdvertisedCaptureProfile, CaptureFormat, CaptureMode, CaptureRate, ColorCamera,
    DvrFrameAtRequest, DvrMediaPayload, DvrMediaStore, GetVideoFrameAtRequest, RawCameraSample,
    SelectedCaptureProfile, VideoProvider,
};
use a2a_lab_dev_kit::{A2aLabError, MemoryVideos, VideoSourceId};
use a2a_lab_ot2::camera::{
    CameraCapture, CameraCaptureError, CameraStill, ExternalCamera, ExternalCameraFormat,
};
use a2a_lab_ot2::images::ImageCapture;
use a2a_lab_ot2::videos::{
    LiveVideoInjections, LiveVideoSourceDescriptor, start_live_video_catalog,
};

fn jpeg() -> Vec<u8> {
    vec![
        0xFF, 0xD8, 0xFF, 0xC0, 0x00, 0x0B, 0x08, 0x00, 0x01, 0x00, 0x01, 0x01, 0x01, 0x11, 0x00,
        0xFF, 0xDA, 0x00, 0x08, 0x01, 0x01, 0x00, 0x00, 0x3F, 0x00, 0xFF, 0xD9,
    ]
}

fn padded(exact: &[u8]) -> Vec<u8> {
    let mut bytes = exact.to_vec();
    bytes.resize(exact.len() + 64, 0);
    bytes
}

struct PaddedCamera {
    bytes: Vec<u8>,
    width: u32,
    height: u32,
}

impl ColorCamera for PaddedCamera {
    fn advertised(&mut self) -> Result<Vec<AdvertisedCaptureProfile>, A2aLabError> {
        let mode = CaptureMode::new(CaptureFormat::Mjpeg, self.width, self.height)?;
        Ok(vec![AdvertisedCaptureProfile::new(
            mode,
            CaptureRate::fps(30)?,
        )])
    }

    fn open(
        &mut self,
        profile: &SelectedCaptureProfile,
    ) -> Result<SelectedCaptureProfile, A2aLabError> {
        Ok(*profile)
    }

    fn raw_frame(&mut self) -> Result<RawCameraSample, A2aLabError> {
        Ok(RawCameraSample::new(
            CaptureFormat::Mjpeg,
            self.width,
            self.height,
            0,
            self.bytes.clone(),
        ))
    }

    fn stop(&mut self) -> Result<(), A2aLabError> {
        Ok(())
    }
}

struct ClosedCapture;

impl CameraCapture for ClosedCapture {
    fn capture(
        &self,
        _index: u32,
        _format: ExternalCameraFormat,
        _frames: u32,
    ) -> Result<Vec<u8>, CameraCaptureError> {
        Err(CameraCaptureError::Empty)
    }
}

async fn still_bytes(capture: impl ImageCapture, expected: &[u8]) -> Vec<u8> {
    for _ in 0..40 {
        if let Ok(frame) = capture.capture().await
            && frame.data() == expected
        {
            return frame.data().to_vec();
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    panic!("normalized JPEG did not reach the still cache");
}

#[tokio::test]
async fn padded_mjpeg_reaches_stills_live_h264_input_and_dvr() {
    let exact = jpeg();
    let root = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("mjpeg-padding");
    let _ = std::fs::remove_dir_all(&root);
    a2a_lab_ot2::camera::clear_camera_frame();
    let camera = LiveVideoSourceDescriptor::legacy_camera(12);
    let external = LiveVideoSourceDescriptor::legacy_external(4, "padded");
    let injections = LiveVideoInjections::none()
        .with_color_camera(
            camera.id.clone(),
            Box::new(PaddedCamera {
                bytes: padded(&exact),
                width: camera.width,
                height: camera.height,
            }),
        )
        .with_color_camera(
            external.id.clone(),
            Box::new(PaddedCamera {
                bytes: padded(&exact),
                width: external.width,
                height: external.height,
            }),
        );
    let catalog = start_live_video_catalog(
        &[camera.clone(), external.clone()],
        5,
        5,
        3_600,
        &root,
        exact.len() as u64,
        injections,
    )
    .await
    .unwrap();

    let camera_still = still_bytes(CameraStill::new(exact.len() as u64).unwrap(), &exact).await;
    let external_still = still_bytes(
        ExternalCamera::with_capture(
            4,
            Duration::from_millis(50),
            exact.len() as u64,
            ClosedCapture,
        )
        .unwrap(),
        &exact,
    )
    .await;
    assert_eq!(camera_still, exact);
    assert_eq!(external_still, exact);

    let source = VideoSourceId::new(&camera.id).unwrap();
    let request = GetVideoFrameAtRequest::new(
        source.clone(),
        a2a_lab_ot2::time::now().unwrap(),
        Some(5_000_000),
    );
    let mut live = None;
    for _ in 0..40 {
        if let Ok(frame) = catalog.provider().get_video_frame_at(request.clone()).await
            && frame.image().data() == exact.as_slice()
        {
            live = Some(frame.image().data().to_vec());
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert_eq!(live.unwrap(), exact);

    let track = MemoryVideos::mjpeg_track_id(&source).unwrap();
    let mut stored = None;
    for _ in 0..40 {
        if let Ok(frame) = catalog
            .dvr()
            .frame_at(DvrFrameAtRequest::new(
                source.clone(),
                track.clone(),
                a2a_lab_ot2::time::now().unwrap(),
                Some(5_000_000),
            ))
            .await
            && let DvrMediaPayload::MjpegFrame(bytes) = frame.payload()
            && bytes.as_slice() == exact.as_slice()
        {
            stored = Some(bytes.clone());
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert_eq!(stored.unwrap(), exact);
    catalog.shutdown();
    let _ = std::fs::remove_dir_all(&root);
}
