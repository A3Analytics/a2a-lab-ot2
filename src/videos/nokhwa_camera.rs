//! Nokhwa adapter implementing the dev-kit [`ColorCamera`] contract.

use nokhwa::utils::{CameraFormat, CameraIndex, FrameFormat, RequestedFormat, RequestedFormatType};
use nokhwa::{Camera, nokhwa_initialize};

use a2a_lab_dev_kit::A2aLabError;
use a2a_lab_dev_kit::videos::{
    AdvertisedCaptureProfile, CaptureFormat, CaptureMode, CaptureRate, ColorCamera,
    RawCameraSample, SelectedCaptureProfile,
};

/// Indexed native camera opened through Nokhwa.
pub struct NokhwaColorCamera {
    camera: Camera,
}

impl NokhwaColorCamera {
    /// Opens `index` for profile negotiation. The stream stays closed until [`ColorCamera::open`].
    pub fn open_index(index: u32) -> Result<Self, A2aLabError> {
        nokhwa_initialize(|_granted| {});
        let requested = RequestedFormat::with_formats(
            RequestedFormatType::None,
            &[FrameFormat::MJPEG, FrameFormat::YUYV],
        );
        let camera = Camera::new(CameraIndex::Index(index), requested).map_err(camera_error)?;
        Ok(Self { camera })
    }
}

impl ColorCamera for NokhwaColorCamera {
    fn advertised(&mut self) -> Result<Vec<AdvertisedCaptureProfile>, A2aLabError> {
        let formats = self
            .camera
            .compatible_camera_formats()
            .map_err(camera_error)?;
        Ok(profiles_from_nokhwa(&formats))
    }

    fn open(
        &mut self,
        profile: &SelectedCaptureProfile,
    ) -> Result<SelectedCaptureProfile, A2aLabError> {
        let format = camera_format(profile)?;
        let fourcc = [format.format()];
        let requested = RequestedFormat::with_formats(RequestedFormatType::Exact(format), &fourcc);
        self.camera
            .set_camera_requset(requested)
            .map_err(camera_error)?;
        self.camera.open_stream().map_err(camera_error)?;
        let actual = self.camera.camera_format();
        negotiated_profile(profile, actual)
    }

    fn raw_frame(&mut self) -> Result<RawCameraSample, A2aLabError> {
        let format = self.camera.camera_format();
        let bytes = self.camera.frame_raw().map_err(camera_error)?.into_owned();
        sample_from_nokhwa(format, bytes)
    }

    fn stop(&mut self) -> Result<(), A2aLabError> {
        self.camera.stop_stream().map_err(camera_error)
    }
}

fn profiles_from_nokhwa(formats: &[CameraFormat]) -> Vec<AdvertisedCaptureProfile> {
    formats
        .iter()
        .filter_map(|format| profile_from_nokhwa(*format))
        .collect()
}

fn profile_from_nokhwa(format: CameraFormat) -> Option<AdvertisedCaptureProfile> {
    let capture = match format.format() {
        FrameFormat::MJPEG => CaptureFormat::Mjpeg,
        FrameFormat::YUYV => CaptureFormat::Yuy2,
        _ => return None,
    };
    let mode = CaptureMode::new(capture, format.width(), format.height()).ok()?;
    let rate = CaptureRate::fps(format.frame_rate()).ok()?;
    Some(AdvertisedCaptureProfile::new(mode, rate))
}

fn sample_from_nokhwa(
    format: CameraFormat,
    bytes: Vec<u8>,
) -> Result<RawCameraSample, A2aLabError> {
    let capture = match format.format() {
        FrameFormat::MJPEG => CaptureFormat::Mjpeg,
        FrameFormat::YUYV => CaptureFormat::Yuy2,
        other => {
            return Err(A2aLabError::invalid(
                "format",
                format!("unsupported raw camera layout {other}"),
            ));
        }
    };
    let stride = if capture == CaptureFormat::Yuy2 {
        if format.height() == 0 || !bytes.len().is_multiple_of(format.height() as usize) {
            return Err(A2aLabError::invalid(
                "stride",
                "YUY2 buffer is not a whole number of rows",
            ));
        }
        u32::try_from(bytes.len() / format.height() as usize)
            .map_err(|_| A2aLabError::invalid("stride", "YUY2 stride exceeds u32"))?
    } else {
        0
    };
    Ok(RawCameraSample::new(
        capture,
        format.width(),
        format.height(),
        stride,
        bytes,
    ))
}

fn camera_format(profile: &SelectedCaptureProfile) -> Result<CameraFormat, A2aLabError> {
    let format = match profile.mode().format() {
        CaptureFormat::Mjpeg => FrameFormat::MJPEG,
        CaptureFormat::Yuy2 => FrameFormat::YUYV,
    };
    if profile.sensor_rate().denominator() != 1 {
        return Err(A2aLabError::invalid(
            "rate",
            "Nokhwa advertises whole-number frame rates",
        ));
    }
    Ok(CameraFormat::new_from(
        profile.mode().width(),
        profile.mode().height(),
        format,
        profile.sensor_rate().numerator(),
    ))
}

fn negotiated_profile(
    requested: &SelectedCaptureProfile,
    actual: CameraFormat,
) -> Result<SelectedCaptureProfile, A2aLabError> {
    let Some(profile) = profile_from_nokhwa(actual) else {
        return Err(A2aLabError::invalid(
            "format",
            "device negotiated an unsupported raw layout",
        ));
    };
    Ok(SelectedCaptureProfile::negotiated(
        profile.mode(),
        profile.sensor_rate(),
        requested.output_rate(),
    ))
}

fn camera_error(error: impl std::fmt::Display) -> A2aLabError {
    A2aLabError::invalid("camera", error.to_string())
}
