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
    camera: Camera,
}

impl NokhwaColorCamera {
    /// Opens `index` for profile negotiation. The stream stays closed until [`ColorCamera::open`].
    ///
    /// The configured mode is applied while the device is created. Changing it
    /// later makes AVFoundation abort on some Mac cameras.
    pub fn open_index(
        index: u32,
        format: FrameFormat,
        width: u32,
        height: u32,
        fps: u32,
    ) -> Result<Self, A2aLabError> {
        Ok(Self {
            camera: open_camera(index, format, width, height, fps)?,
        })
    }
}

impl ColorCamera for NokhwaColorCamera {
    fn advertised(&mut self) -> Result<Vec<AdvertisedCaptureProfile>, A2aLabError> {
        let active = self.camera.camera_format();
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
        // AVFoundation's list includes range minimums that `set_all` rejects.
        // Keep the rate the device is already running so a slower save rate
        // does not reconfigure the sensor.
        Ok(rates_for_open_mode(profiles, active))
    }

    fn open(
        &mut self,
        profile: &SelectedCaptureProfile,
    ) -> Result<SelectedCaptureProfile, A2aLabError> {
        let format = camera_format(profile)?;
        let active = self.camera.camera_format();
        let already = active.format() == format.format()
            && active.width() == format.width()
            && active.height() == format.height()
            && active.frame_rate() == format.frame_rate();
        if !already {
            let fourcc = [format.format()];
            let requested =
                RequestedFormat::with_formats(RequestedFormatType::Exact(format), &fourcc);
            self.camera
                .set_camera_requset(requested)
                .map_err(camera_error)?;
        }
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

fn open_camera(
    index: u32,
    format: FrameFormat,
    width: u32,
    height: u32,
    fps: u32,
) -> Result<Camera, A2aLabError> {
    nokhwa_initialize(|_granted| {});
    // `fps` is the save rate. This camera's sensor only accepts a range
    // maximum (30 for 1280x720), so a slower save rate retries that same
    // size and format at the next settable rate.
    let mut last = None;
    for rate in sensor_rate_attempts(fps) {
        match open_exact(index, format, width, height, rate) {
            Ok(camera) => return Ok(camera),
            Err(error) => last = Some(error),
        }
    }
    Err(last.unwrap_or_else(|| A2aLabError::invalid("camera", "no sensor rate opened")))
}

fn sensor_rate_attempts(save_fps: u32) -> Vec<u32> {
    // Rates under 15 are range minimums on this camera: `set_all` rejects them.
    // The catalog still saves at `save_fps`; the sensor stays on a settable maximum.
    let mut rates = if save_fps >= 15 {
        vec![save_fps, 30, 60, 24, 25, 20, 15]
    } else {
        vec![30, 60, 24, 25, 20, 15, 10]
    };
    let mut seen = Vec::new();
    rates.retain(|rate| {
        if seen.contains(rate) {
            false
        } else {
            seen.push(*rate);
            true
        }
    });
    rates
}

fn open_exact(
    index: u32,
    format: FrameFormat,
    width: u32,
    height: u32,
    fps: u32,
) -> Result<Camera, A2aLabError> {
    let configured = CameraFormat::new_from(width, height, format, fps);
    let formats = [format];
    let exact = RequestedFormat::with_formats(RequestedFormatType::Exact(configured), &formats);
    Camera::new(CameraIndex::Index(index), exact).map_err(camera_error)
}

fn rates_for_open_mode(
    profiles: Vec<AdvertisedCaptureProfile>,
    active: CameraFormat,
) -> Vec<AdvertisedCaptureProfile> {
    let Some(capture) = capture_format(active.format()) else {
        return profiles;
    };
    let matches = |profile: &AdvertisedCaptureProfile| {
        profile.mode().format() == capture
            && profile.mode().width() == active.width()
            && profile.mode().height() == active.height()
    };
    let active_listed = profiles.iter().any(|profile| {
        matches(profile) && whole_fps(profile.sensor_rate()) == Some(active.frame_rate())
    });
    if active_listed {
        return profiles
            .into_iter()
            .filter(|profile| {
                !matches(profile) || whole_fps(profile.sensor_rate()) == Some(active.frame_rate())
            })
            .collect();
    }
    let Some(fastest) = profiles
        .iter()
        .filter(|profile| matches(profile))
        .max_by(|left, right| rate_ord(left.sensor_rate(), right.sensor_rate()))
        .map(|profile| profile.sensor_rate())
    else {
        return profiles;
    };
    profiles
        .into_iter()
        .filter(|profile| !matches(profile) || profile.sensor_rate() == fastest)
        .collect()
}

fn capture_format(format: FrameFormat) -> Option<CaptureFormat> {
    match format {
        FrameFormat::MJPEG => Some(CaptureFormat::Mjpeg),
        FrameFormat::YUYV => Some(CaptureFormat::Yuy2),
        _ => None,
    }
}

fn whole_fps(rate: CaptureRate) -> Option<u32> {
    (rate.denominator() == 1).then_some(rate.numerator())
}

fn rate_ord(left: CaptureRate, right: CaptureRate) -> std::cmp::Ordering {
    let left_value = u64::from(left.numerator()) * u64::from(right.denominator());
    let right_value = u64::from(right.numerator()) * u64::from(left.denominator());
    left_value.cmp(&right_value)
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

#[cfg(test)]
mod tests {
    use super::{bounded_yuy2, profiles_for_fourcc, rates_for_open_mode, sensor_rate_attempts};
    use a2a_lab_dev_kit::videos::{CaptureFormat, CaptureRate};
    use nokhwa::utils::{CameraFormat, FrameFormat, Resolution};
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
                .any(|profile| profile.sensor_rate() == CaptureRate::fps(30).unwrap())
        );
        assert!(
            profiles
                .iter()
                .any(|profile| profile.sensor_rate() == CaptureRate::fps(15).unwrap())
        );
    }

    #[test]
    fn save_rate_below_fifteen_still_opens_the_thirty_fps_sensor() {
        assert_eq!(sensor_rate_attempts(5), vec![30, 60, 24, 25, 20, 15, 10]);
    }

    #[test]
    fn open_mode_keeps_the_active_sensor_rate() {
        let mut listed = HashMap::new();
        listed.insert(Resolution::new(1280, 720), vec![15, 30]);
        let profiles = profiles_for_fourcc(FrameFormat::YUYV, &listed);
        let active = CameraFormat::new_from(1280, 720, FrameFormat::YUYV, 30);
        let kept = rates_for_open_mode(profiles, active);
        assert_eq!(kept.len(), 1);
        assert_eq!(kept[0].sensor_rate(), CaptureRate::fps(30).unwrap());
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
