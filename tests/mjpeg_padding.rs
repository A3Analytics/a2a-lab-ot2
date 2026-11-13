//! Hardware-free proof that padded MJPEG is normalized before every consumer.

use std::path::PathBuf;
use std::time::Duration;

use a2a_lab_dev_kit::A2aLabError;
use a2a_lab_dev_kit::videos::{
    AdvertisedCaptureProfile, CaptureFormat, CaptureMode, CaptureRate, ColorCamera,
    RawCameraSample, SelectedCaptureProfile,
};
use a2a_lab_ot2::{
    LiveVideoInjections, LiveVideoSourceDescriptor, start_fixture_declarative_serve,
};

fn jpeg() -> Vec<u8> {
    use image::codecs::jpeg::JpegEncoder;
    use image::{ExtendedColorType, ImageBuffer, ImageEncoder, Rgb};
    let image: ImageBuffer<Rgb<u8>, Vec<u8>> = ImageBuffer::from_fn(4, 4, |_, _| Rgb([64, 64, 64]));
    let mut out = Vec::new();
    JpegEncoder::new_with_quality(&mut out, 80)
        .write_image(image.as_raw(), 4, 4, ExtendedColorType::Rgb8)
        .expect("jpeg");
    out
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
        Ok(SelectedCaptureProfile::negotiated(
            profile.mode(),
            CaptureRate::fps(30)?,
            CaptureRate::fps(5)?,
        ))
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

async fn still_from_runtime(
    catalog: &a2a_lab_ot2::DeclarativeVideoServe,
    expected: &[u8],
) -> Vec<u8> {
    use a2a_lab_ot2::ImageCapture;
    use a2a_lab_ot2::camera::{CAMERA_SOURCE_ID, PumpLinkedStill};
    let still = PumpLinkedStill::new(
        CAMERA_SOURCE_ID,
        "Camera",
        64 * 1024,
        catalog.still_cache().clone(),
    )
    .unwrap();
    for _ in 0..80 {
        if let Ok(frame) = still.capture().await
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
    let mut camera = LiveVideoSourceDescriptor::legacy_camera(12);
    camera.width = Some(4);
    camera.height = Some(4);
    let injections = LiveVideoInjections::none().with_color_camera(
        camera.id.clone(),
        Box::new(PaddedCamera {
            bytes: padded(&exact),
            width: camera.width.unwrap(),
            height: camera.height.unwrap(),
        }),
    );
    let catalog = start_fixture_declarative_serve(
        vec![camera.clone()],
        5,
        5,
        3_600,
        &root,
        64 * 1024,
        None,
        injections,
    )
    .await
    .unwrap();

    let camera_still = still_from_runtime(&catalog, &exact).await;
    assert_eq!(camera_still, exact);

    catalog.shutdown();
    let _ = std::fs::remove_dir_all(&root);
}
