//! Wraps test [`LiveJpegSource`] producers as devkit [`ColorCamera`] backends.

use a2a_lab_dev_kit::A2aLabError;
use a2a_lab_dev_kit::videos::{
    AdvertisedCaptureProfile, CaptureFormat, CaptureMode, CaptureRate, ColorCamera,
    RawCameraSample, SelectedCaptureProfile,
};

use crate::camera::{
    CAMERA_SOURCE_ID, EXTERNAL_CAMERA_HEIGHT, EXTERNAL_CAMERA_SOURCE_ID, EXTERNAL_CAMERA_WIDTH,
};
use crate::videos::external::LiveJpegSource;

/// Injected MJPEG producer for declarative runtime tests and legacy serve fixtures.
pub struct JpegSourceCamera {
    source: Box<dyn LiveJpegSource>,
    width: u32,
    height: u32,
}

impl JpegSourceCamera {
    #[must_use]
    pub fn new(source: Box<dyn LiveJpegSource>, width: u32, height: u32) -> Self {
        Self {
            source,
            width,
            height,
        }
    }

    #[must_use]
    pub fn for_legacy_source_id(id: &str, source: Box<dyn LiveJpegSource>) -> Self {
        let (width, height) = match id {
            EXTERNAL_CAMERA_SOURCE_ID => (EXTERNAL_CAMERA_WIDTH, EXTERNAL_CAMERA_HEIGHT),
            CAMERA_SOURCE_ID => (1280, 720),
            _ => (640, 480),
        };
        Self::new(source, width, height)
    }
}

impl ColorCamera for JpegSourceCamera {
    fn advertised(&mut self) -> Result<Vec<AdvertisedCaptureProfile>, A2aLabError> {
        Ok(vec![AdvertisedCaptureProfile::new(
            CaptureMode::new(CaptureFormat::Mjpeg, self.width, self.height).expect("mode"),
            CaptureRate::fps(30).expect("rate"),
        )])
    }

    fn open(
        &mut self,
        selected: &SelectedCaptureProfile,
    ) -> Result<SelectedCaptureProfile, A2aLabError> {
        Ok(*selected)
    }

    fn raw_frame(&mut self) -> Result<RawCameraSample, A2aLabError> {
        let bytes = self.source.next_jpeg()?;
        Ok(RawCameraSample::new(
            CaptureFormat::Mjpeg,
            self.width,
            self.height,
            0,
            bytes,
        ))
    }

    fn stop(&mut self) -> Result<(), A2aLabError> {
        Ok(())
    }
}
