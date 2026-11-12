//! Nokhwa adapter implementing the dev-kit [`ColorCamera`] contract.

use std::collections::HashMap;

use nokhwa::utils::{
    CameraFormat, CameraIndex, FrameFormat, RequestedFormat, RequestedFormatType, Resolution,
};
use nokhwa::{Camera, nokhwa_initialize};

use a2a_lab_dev_kit::A2aLabError;
use a2a_lab_dev_kit::videos::{
    AdvertisedCaptureProfile, CaptureFormat, CaptureMode, CaptureRate, ColorCamera,
    RawCameraSample, SelectedCaptureProfile,
};

/// Indexed native camera opened through Nokhwa.
pub struct NokhwaColorCamera {
    index: u32,
    camera: Camera,
}

impl NokhwaColorCamera {
    /// Opens `index` for profile negotiation. The stream stays closed until [`ColorCamera::open`].
    ///
    /// Listing uses a probe handle. [`ColorCamera::open`] recreates the device
    /// with `RequestedFormatType::Exact` because `AVFoundation` aborts when the
    /// mode is changed after `Camera::new`.
    pub fn open_index(index: u32, format: FrameFormat) -> Result<Self, A2aLabError> {
        Ok(Self {
            index,
            camera: probe_camera(index, format)?,
        })
    }
}

impl ColorCamera for NokhwaColorCamera {
    fn advertised(&mut self) -> Result<Vec<AdvertisedCaptureProfile>, A2aLabError> {
        let formats = self
            .camera
            .compatible_camera_formats()
            .map_err(camera_error)?;
        let profiles = if formats.is_empty() {
            // nokhwa 0.10's AVFoundation `compatible_list_by_resolution` excludes the
            // fourcc it is asked for, so a device that maps to one fourcc reports
            // an empty `compatible_camera_formats` list. Query the other fourcc.
            let fourccs = self.camera.compatible_fourcc().map_err(camera_error)?;
            let mut profiles = Vec::new();
            for fourcc in [FrameFormat::MJPEG, FrameFormat::YUYV] {
                if !fourccs.contains(&fourcc) {
                    continue;
                }
                let probe = if fourcc == FrameFormat::MJPEG {
                    FrameFormat::YUYV
                } else {
                    FrameFormat::MJPEG
                };
                let listed = self
                    .camera
                    .compatible_list_by_resolution(probe)
                    .map_err(camera_error)?;
                profiles.extend(profiles_for_fourcc(fourcc, &listed));
            }
            profiles
        } else {
            profiles_from_nokhwa(&formats)
        };
        Ok(profiles)
    }

    fn open(
        &mut self,
        profile: &SelectedCaptureProfile,
    ) -> Result<SelectedCaptureProfile, A2aLabError> {
        let format = camera_format(profile)?;
        self.camera = open_exact_camera(self.index, format)?;
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
    let (stride, bytes) = if capture == CaptureFormat::Yuy2 {
        bounded_yuy2(format.width(), format.height(), bytes)?
    } else {
        (0, bytes)
    };
    Ok(RawCameraSample::new(
        capture,
        format.width(),
        format.height(),
        stride,
        bytes,
    ))
}

fn probe_camera(index: u32, format: FrameFormat) -> Result<Camera, A2aLabError> {
    nokhwa_initialize(|_granted| {});
    let mut last = None;
    for (width, height, fps) in [(1920, 1080, 30), (1280, 720, 30), (640, 480, 30)] {
        match open_exact_camera(index, CameraFormat::new_from(width, height, format, fps)) {
            Ok(camera) => return Ok(camera),
            Err(error) => last = Some(error),
        }
    }
    Err(last.unwrap_or_else(|| camera_error("probe could not open a 1080p-or-smaller mode")))
}

fn open_exact_camera(index: u32, format: CameraFormat) -> Result<Camera, A2aLabError> {
    nokhwa_initialize(|_granted| {});
    let formats = [format.format()];
    let requested = RequestedFormat::with_formats(RequestedFormatType::Exact(format), &formats);
    Camera::new(CameraIndex::Index(index), requested).map_err(camera_error)
}

fn profiles_for_fourcc(
    fourcc: FrameFormat,
    resolutions: &HashMap<Resolution, Vec<u32>>,
) -> Vec<AdvertisedCaptureProfile> {
    let mut formats = Vec::new();
    for (resolution, rates) in resolutions {
        let mut rates = rates.clone();
        rates.sort_unstable();
        rates.dedup();
        for fps in rates {
            formats.push(CameraFormat::new_from(
                resolution.width(),
                resolution.height(),
                fourcc,
                fps,
            ));
        }
    }
    profiles_from_nokhwa(&formats)
}

fn bounded_yuy2(
    width: u32,
    height: u32,
    mut bytes: Vec<u8>,
) -> Result<(u32, Vec<u8>), A2aLabError> {
    let row = usize::try_from(width)
        .ok()
        .and_then(|value| value.checked_mul(2))
        .ok_or_else(|| A2aLabError::invalid("stride", "YUY2 row size overflow"))?;
    let rows = usize::try_from(height)
        .map_err(|_| A2aLabError::invalid("stride", "YUY2 height exceeds usize"))?;
    let needed = row
        .checked_mul(rows)
        .ok_or_else(|| A2aLabError::invalid("stride", "YUY2 buffer size overflow"))?;
    if row == 0 || bytes.len() < needed {
        return Err(A2aLabError::invalid(
            "stride",
            format!("YUY2 buffer is {} bytes for {width}x{height}", bytes.len()),
        ));
    }
    bytes.truncate(needed);
    let stride = u32::try_from(row)
        .map_err(|_| A2aLabError::invalid("stride", "YUY2 stride exceeds u32"))?;
    Ok((stride, bytes))
}

fn camera_format(profile: &SelectedCaptureProfile) -> Result<CameraFormat, A2aLabError> {
    let format = match profile.mode().format() {
        CaptureFormat::Mjpeg => FrameFormat::MJPEG,
        CaptureFormat::Yuy2 => FrameFormat::YUYV,
    };
    if profile.capture_rate().denominator() != 1 {
        return Err(A2aLabError::invalid(
            "rate",
            "Nokhwa advertises whole-number frame rates",
        ));
    }
    Ok(CameraFormat::new_from(
        profile.mode().width(),
        profile.mode().height(),
        format,
        profile.capture_rate().numerator(),
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
        profile.capture_rate(),
        requested.output_rate(),
    ))
}

#[cfg(test)]
fn configured_camera_format(
    format: FrameFormat,
    width: u32,
    height: u32,
    capture_rate: u32,
) -> CameraFormat {
    CameraFormat::new_from(width, height, format, capture_rate)
}

fn camera_error(error: impl std::fmt::Display) -> A2aLabError {
    A2aLabError::invalid("camera", error.to_string())
}

#[cfg(test)]
mod tests {
    use super::{bounded_yuy2, configured_camera_format, profiles_for_fourcc};
    use a2a_lab_dev_kit::videos::{CaptureFormat, CaptureRate};
    use nokhwa::utils::{FrameFormat, Resolution};
    use std::collections::HashMap;

    #[test]
    fn recovered_fourcc_list_keeps_distinct_rates() {
        let mut listed = HashMap::new();
        listed.insert(Resolution::new(1280, 720), vec![30, 15, 30]);
        let profiles = profiles_for_fourcc(FrameFormat::YUYV, &listed);
        assert_eq!(profiles.len(), 2);
        assert!(profiles.iter().all(|profile| {
            profile.mode().format() == CaptureFormat::Yuy2
                && profile.mode().width() == 1280
                && profile.mode().height() == 720
        }));
        assert!(
            profiles
                .iter()
                .any(|profile| profile.capture_rate() == CaptureRate::fps(30).unwrap())
        );
        assert!(
            profiles
                .iter()
                .any(|profile| profile.capture_rate() == CaptureRate::fps(15).unwrap())
        );
    }

    #[test]
    fn exact_open_request_keeps_configured_mode() {
        let format = configured_camera_format(FrameFormat::YUYV, 640, 480, 15);
        assert_eq!(format.width(), 640);
        assert_eq!(format.height(), 480);
        assert_eq!(format.format(), FrameFormat::YUYV);
        assert_eq!(format.frame_rate(), 15);
    }

    #[test]
    fn yuy2_drops_a_trailing_padding_row() {
        let row = 1280 * 2;
        let bytes = vec![7_u8; row * 721];
        let (stride, trimmed) = bounded_yuy2(1280, 720, bytes).unwrap();
        assert_eq!(stride, u32::try_from(row).unwrap());
        assert_eq!(trimmed.len(), row * 720);
    }
}
