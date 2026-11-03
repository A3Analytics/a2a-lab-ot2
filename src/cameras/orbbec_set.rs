//! Orbbec synchronized frame sets behind the portable capture contract.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde_json::{Map, Value};

use crate::frame_sets::{
    CaptureRequest, CapturedFrameSet, ChannelCalibration, ChannelProfile, ChannelRole, DepthScale,
    DepthUnit, Distortion, Extrinsics, FrameChannel, FrameChannelId, FrameChannelParts,
    FrameSetCapture, FrameSetError, FrameSetSource, FrameSetSourceId, Intrinsics, frame_channel,
};
use crate::orbbec::{ORBBEC_CAMERA_SOURCE_ID, ORBBEC_FIRST_FRAME_WAIT, OrbbecCaptureError};
use crate::orbbec_frames::{self, DeviceView, Plane, PlaneFormat};

use super::png::{encode_gray, y16_native};

const SYNC_TOLERANCE_US: u64 = 100_000;

/// Reads one Orbbec device without exposing SDK types to the catalog.
pub(crate) trait OrbbecDevice: Send + Sync {
    /// Profiles the device advertises. This does not wait for a frame.
    fn describe(&self, serial: Option<&str>) -> Result<DeviceView, OrbbecCaptureError>;

    /// One synchronized frameset.
    fn capture(
        &self,
        serial: Option<&str>,
        timeout: Duration,
        max_bytes: u64,
    ) -> Result<DeviceView, OrbbecCaptureError>;
}

/// SDK-backed device. The library loads on the first call.
#[derive(Debug, Default, Clone, Copy)]
pub struct SdkOrbbecDevice;

impl OrbbecDevice for SdkOrbbecDevice {
    fn describe(&self, serial: Option<&str>) -> Result<DeviceView, OrbbecCaptureError> {
        orbbec_frames::describe(serial)
    }

    fn capture(
        &self,
        serial: Option<&str>,
        timeout: Duration,
        max_bytes: u64,
    ) -> Result<DeviceView, OrbbecCaptureError> {
        orbbec_frames::capture(serial, timeout, max_bytes)
    }
}

/// Portable capture over an [`OrbbecDevice`].
pub struct OrbbecFrameCapture {
    device: Arc<dyn OrbbecDevice>,
    serial: Option<String>,
    source: Mutex<Option<FrameSetSource>>,
}

impl OrbbecFrameCapture {
    /// Captures through the Orbbec SDK when a frame is requested.
    #[must_use]
    pub fn sdk(serial: Option<&str>) -> Self {
        Self::with_device(serial, SdkOrbbecDevice)
    }

    /// Captures through an injected device.
    #[must_use]
    pub(crate) fn with_device(serial: Option<&str>, device: impl OrbbecDevice + 'static) -> Self {
        Self {
            device: Arc::new(device),
            serial: serial.map(str::to_owned),
            source: Mutex::new(None),
        }
    }

    fn cached(&self) -> Option<FrameSetSource> {
        self.source
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }

    fn remember(&self, source: FrameSetSource) {
        *self
            .source
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(source);
    }
}

impl FrameSetCapture for OrbbecFrameCapture {
    fn sources(&self) -> Result<Vec<FrameSetSource>, FrameSetError> {
        if let Some(source) = self.cached() {
            return Ok(vec![source]);
        }
        let view = self
            .device
            .describe(self.serial.as_deref())
            .map_err(|error| map_orbbec(self.label(), error))?;
        let source = source_from(&view)?;
        self.remember(source.clone());
        Ok(vec![source])
    }

    fn capture(&self, request: &CaptureRequest) -> Result<CapturedFrameSet, FrameSetError> {
        let timeout = Duration::from_millis(request.timeout_ms()).max(ORBBEC_FIRST_FRAME_WAIT);
        let view = self
            .device
            .capture(self.serial.as_deref(), timeout, request.max_channel_bytes())
            .map_err(|error| map_orbbec(self.label(), error))?;
        let source = source_from(&view)?;
        self.remember(source.clone());
        let channels = channels_from(&view, request)?;
        CapturedFrameSet::new(
            &source,
            host_now(),
            channels,
            request.channel_ids(),
            Map::new(),
            request.max_channel_bytes(),
        )
    }
}

impl OrbbecFrameCapture {
    fn label(&self) -> String {
        self.serial
            .clone()
            .unwrap_or_else(|| ORBBEC_CAMERA_SOURCE_ID.to_owned())
    }
}

fn source_from(view: &DeviceView) -> Result<FrameSetSource, FrameSetError> {
    let mut channels = Vec::new();
    for plane in &view.planes {
        channels.push(profile(plane)?);
    }
    let serial = if view.serial.is_empty() {
        "unknown".to_owned()
    } else {
        view.serial.clone()
    };
    let model = if view.model.is_empty() {
        "orbbec".to_owned()
    } else {
        view.model.clone()
    };
    FrameSetSource::new(
        FrameSetSourceId::new(ORBBEC_CAMERA_SOURCE_ID)?,
        "orbbec",
        serial.clone(),
        model,
        channels,
        format!("device:{serial}"),
        SYNC_TOLERANCE_US,
    )
}

fn profile(plane: &Plane) -> Result<ChannelProfile, FrameSetError> {
    let (media_type, bit_depth) = encoding(plane.format);
    ChannelProfile::new(
        FrameChannelId::new(plane.channel_id)?,
        ChannelRole::new(plane.role)?,
        media_type,
        plane.width,
        plane.height,
        bit_depth,
        if plane.fps.is_finite() && plane.fps > 0.0 {
            plane.fps
        } else {
            30.0
        },
    )
}

fn channels_from(
    view: &DeviceView,
    request: &CaptureRequest,
) -> Result<Vec<FrameChannel>, FrameSetError> {
    let selected = select_planes(&view.planes, request.channel_ids())?;
    let mut channels = Vec::with_capacity(selected.len());
    for plane in selected {
        channels.push(channel_from(plane)?);
    }
    Ok(channels)
}

fn select_planes<'a>(
    planes: &'a [Plane],
    requested: &[FrameChannelId],
) -> Result<Vec<&'a Plane>, FrameSetError> {
    if requested.is_empty() {
        return Ok(planes.iter().collect());
    }
    let mut selected = Vec::with_capacity(requested.len());
    for id in requested {
        let plane = planes.iter().find(|plane| plane.channel_id == id.as_str());
        let Some(plane) = plane else {
            return Err(FrameSetError::UnsupportedProfile {
                device: "orbbec".to_owned(),
                detail: format!("channel {id} is not advertised"),
            });
        };
        selected.push(plane);
    }
    Ok(selected)
}

fn channel_from(plane: &Plane) -> Result<FrameChannel, FrameSetError> {
    let (media_type, bit_depth) = encoding(plane.format);
    let data = encode_plane(plane, bit_depth)?;
    let depth = if plane.role == "depth" {
        Some(depth_scale(plane)?)
    } else {
        None
    };
    Ok(frame_channel(FrameChannelParts {
        id: FrameChannelId::new(plane.channel_id)?,
        role: ChannelRole::new(plane.role)?,
        media_type: media_type.to_owned(),
        width: plane.width,
        height: plane.height,
        bit_depth,
        device_timestamp_us: plane.timestamp_us,
        frame_number: plane.frame_number,
        depth,
        calibration: calibration(plane),
        attributes: plane_attributes(plane.format),
        data,
    }))
}

fn encode_plane(plane: &Plane, bit_depth: u8) -> Result<Vec<u8>, FrameSetError> {
    match plane.format {
        PlaneFormat::Jpeg => Ok(plane.samples.clone()),
        PlaneFormat::Gray8 => encode_gray(&plane.samples, plane.width, plane.height, bit_depth),
        PlaneFormat::Gray16Le => {
            let samples = y16_native(&plane.samples)?;
            encode_gray(&samples, plane.width, plane.height, bit_depth)
        }
    }
}

fn depth_scale(plane: &Plane) -> Result<DepthScale, FrameSetError> {
    let scale = plane.depth_scale_mm.ok_or_else(|| {
        FrameSetError::Malformed("depth frame did not report a value scale".to_owned())
    })?;
    DepthScale::new(f64::from(scale), DepthUnit::Millimeter).map_err(|_| {
        FrameSetError::Malformed(format!(
            "depth value scale {scale} is not a positive finite millimeter multiplier"
        ))
    })
}

fn calibration(plane: &Plane) -> ChannelCalibration {
    let intrinsics = plane.intrinsics.as_ref().and_then(|value| {
        Intrinsics::new(
            f64::from(value.fx),
            f64::from(value.fy),
            f64::from(value.cx),
            f64::from(value.cy),
            value.width,
            value.height,
        )
        .ok()
    });
    let distortion = plane.distortion.as_ref().and_then(|(model, coefficients)| {
        Distortion::new(
            model.clone(),
            coefficients.iter().copied().map(f64::from).collect(),
        )
        .ok()
    });
    let extrinsics = match (&plane.rotation, &plane.translation) {
        (Some(rotation), Some(translation)) if plane.role != "color" => {
            let rotation = rotation.map(f64::from);
            let translation = translation.map(f64::from);
            FrameChannelId::new("color")
                .ok()
                .and_then(|reference| Extrinsics::new(rotation, translation, reference).ok())
        }
        _ => None,
    };
    ChannelCalibration::new(intrinsics, distortion, extrinsics)
}

fn plane_attributes(format: PlaneFormat) -> Map<String, Value> {
    let mut attributes = Map::new();
    attributes.insert(
        "pixel_format".to_owned(),
        Value::String(
            match format {
                PlaneFormat::Jpeg => "MJPG",
                PlaneFormat::Gray8 => "Y8",
                PlaneFormat::Gray16Le => "Y16",
            }
            .to_owned(),
        ),
    );
    attributes
}

fn encoding(format: PlaneFormat) -> (&'static str, u8) {
    match format {
        PlaneFormat::Jpeg => ("image/jpeg", 8),
        PlaneFormat::Gray8 => ("image/png", 8),
        PlaneFormat::Gray16Le => ("image/png", 16),
    }
}

fn host_now() -> String {
    jiff::Timestamp::now().to_string()
}

fn map_orbbec(device: String, error: OrbbecCaptureError) -> FrameSetError {
    let detail = error.to_string();
    match error {
        OrbbecCaptureError::Library(_)
        | OrbbecCaptureError::NoDevice
        | OrbbecCaptureError::MissingSerial(_)
        | OrbbecCaptureError::Ambiguous(_) => {
            FrameSetError::Unavailable(format!("{device}: {detail}"))
        }
        OrbbecCaptureError::Permission(detail) => FrameSetError::Permission { device, detail },
        OrbbecCaptureError::NoColorProfile(detail) => {
            FrameSetError::UnsupportedProfile { device, detail }
        }
        OrbbecCaptureError::Timeout => FrameSetError::Timeout {
            device,
            detail: "timed out".to_owned(),
        },
        OrbbecCaptureError::Empty | OrbbecCaptureError::Malformed(_) => {
            FrameSetError::Malformed(format!("{device}: {detail}"))
        }
        OrbbecCaptureError::Oversize => FrameSetError::Oversize,
        OrbbecCaptureError::Sdk(detail) if busy(&detail) => FrameSetError::Busy { device, detail },
        OrbbecCaptureError::Sdk(detail) => {
            FrameSetError::Unavailable(format!("{device}: {detail}"))
        }
    }
}

fn busy(detail: &str) -> bool {
    detail.to_ascii_lowercase().contains("busy")
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Fixed {
        view: DeviceView,
    }

    impl OrbbecDevice for Fixed {
        fn describe(&self, _serial: Option<&str>) -> Result<DeviceView, OrbbecCaptureError> {
            Ok(self.view.clone())
        }

        fn capture(
            &self,
            _serial: Option<&str>,
            _timeout: Duration,
            _max_bytes: u64,
        ) -> Result<DeviceView, OrbbecCaptureError> {
            Ok(self.view.clone())
        }
    }

    fn view() -> DeviceView {
        DeviceView {
            serial: "ABC".to_owned(),
            model: "gemini".to_owned(),
            planes: vec![
                Plane {
                    channel_id: "color",
                    role: "color",
                    format: PlaneFormat::Jpeg,
                    width: 1,
                    height: 1,
                    fps: 30.0,
                    timestamp_us: 10,
                    frame_number: 4,
                    depth_scale_mm: None,
                    samples: vec![0xFF, 0xD8, 0xFF, 0xD9],
                    intrinsics: None,
                    distortion: None,
                    rotation: None,
                    translation: None,
                },
                Plane {
                    channel_id: "depth",
                    role: "depth",
                    format: PlaneFormat::Gray16Le,
                    width: 1,
                    height: 1,
                    fps: 30.0,
                    timestamp_us: 12,
                    frame_number: 4,
                    depth_scale_mm: Some(1.0),
                    samples: vec![0x2C, 0x01],
                    intrinsics: None,
                    distortion: None,
                    rotation: None,
                    translation: None,
                },
            ],
        }
    }

    #[test]
    fn little_endian_depth_becomes_a_sixteen_bit_png() {
        let capture = OrbbecFrameCapture::with_device(Some("ABC"), Fixed { view: view() });
        let request = CaptureRequest::new(
            FrameSetSourceId::new(ORBBEC_CAMERA_SOURCE_ID).unwrap(),
            Vec::new(),
            20,
            1024,
        )
        .unwrap();
        let set = capture.capture(&request).unwrap();
        let published = set
            .publish(
                crate::frame_sets::FrameSetId::new("frame-1").unwrap(),
                &capture.sources().unwrap()[0],
                &[],
                1024,
            )
            .unwrap();
        let depth = &published.channels()[1];
        assert_eq!(depth.media_type(), "image/png");
        assert_eq!(depth.bit_depth(), 16);
        assert!((depth.depth().unwrap().scale() - 1.0).abs() < f64::EPSILON);
        assert_eq!(depth.data()[0], 0x89);
        let pixel = image::load_from_memory(depth.data())
            .unwrap()
            .to_luma16()
            .get_pixel(0, 0)[0];
        assert_eq!(pixel, 0x012C);
    }

    struct Fail;

    impl OrbbecDevice for Fail {
        fn describe(&self, _serial: Option<&str>) -> Result<DeviceView, OrbbecCaptureError> {
            Err(OrbbecCaptureError::Timeout)
        }

        fn capture(
            &self,
            _serial: Option<&str>,
            _timeout: Duration,
            _max_bytes: u64,
        ) -> Result<DeviceView, OrbbecCaptureError> {
            Err(OrbbecCaptureError::Timeout)
        }
    }

    #[test]
    fn timeout_and_unsynchronized_sets_are_not_published() {
        let capture = OrbbecFrameCapture::with_device(None, Fail);
        let request = CaptureRequest::new(
            FrameSetSourceId::new(ORBBEC_CAMERA_SOURCE_ID).unwrap(),
            Vec::new(),
            20,
            1024,
        )
        .unwrap();
        assert_eq!(capture.capture(&request).unwrap_err().class(), "timeout");
        let mut drifted = view();
        drifted.planes[1].timestamp_us = 200_000;
        let capture = OrbbecFrameCapture::with_device(Some("ABC"), Fixed { view: drifted });
        assert_eq!(
            capture.capture(&request).unwrap_err().class(),
            "unsynchronized"
        );
    }
}
