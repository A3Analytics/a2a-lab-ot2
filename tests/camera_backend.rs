//! Camera backend selection without hardware.

use std::time::Duration;

use a2a_lab_ot2::cameras::{
    CameraBackend, CameraDevice, CameraError, CameraSelection, select_camera,
};
use a2a_lab_ot2::frame_sets::{ChannelProfile, ChannelRole, FrameChannelId};

struct Script {
    devices: Vec<CameraDevice>,
    error: Option<CameraError>,
    calls: std::sync::atomic::AtomicUsize,
}

impl CameraBackend for Script {
    fn id(&self) -> &'static str {
        "fake"
    }

    fn discover(&self) -> Result<Vec<CameraDevice>, CameraError> {
        self.calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        self.error.clone().map_or(Ok(self.devices.clone()), Err)
    }

    fn capture_color(
        &self,
        _serial: Option<&str>,
        _timeout: Duration,
        _max_bytes: u64,
    ) -> Result<Vec<u8>, CameraError> {
        self.calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        self.error
            .clone()
            .map_or(Ok(vec![0xFF, 0xD8, 0xFF, 0xD9]), Err)
    }
}

fn device(serial: &str) -> CameraDevice {
    CameraDevice::new(
        "fake",
        serial,
        "stereo",
        vec![
            ChannelProfile::new(
                FrameChannelId::new("color").unwrap(),
                ChannelRole::color(),
                "image/jpeg",
                1,
                1,
                8,
                30.0,
            )
            .unwrap(),
        ],
    )
}

fn error(class: &str) -> CameraError {
    let device = "cam".to_owned();
    let detail = class.to_owned();
    match class {
        "permission" => CameraError::Permission { device, detail },
        "busy" => CameraError::Busy { device, detail },
        "unsupported_profile" => CameraError::UnsupportedProfile { device, detail },
        "timeout" => CameraError::Timeout { device, detail },
        "malformed" => CameraError::Malformed { device, detail },
        "unsynchronized" => CameraError::Unsynchronized { device, detail },
        _ => CameraError::Unavailable { device, detail },
    }
}

#[test]
fn selection_covers_zero_one_missing_and_ambiguous_devices() {
    assert_eq!(select_camera(&[], None).unwrap_err().class(), "unavailable");
    let one = vec![device("only")];
    assert_eq!(select_camera(&one, None).unwrap(), 0);
    assert!(select_camera(&one, Some("missing")).is_err());
    let many = vec![device("b"), device("a")];
    let ambiguous = select_camera(&many, None).unwrap_err();
    assert!(ambiguous.to_string().contains('a'));
    assert_eq!(select_camera(&many, Some("b")).unwrap(), 0);
    assert!(CameraSelection::new("generic", Some("")).is_err());
}

#[test]
fn fake_backend_reports_profiles_and_every_error_class() {
    let backend = Script {
        devices: vec![device("only")],
        error: None,
        calls: std::sync::atomic::AtomicUsize::new(0),
    };
    assert_eq!(backend.discover().unwrap()[0].channels().len(), 1);
    assert_eq!(
        backend
            .capture_color(None, Duration::from_millis(1), 8)
            .unwrap()[0],
        0xFF
    );
    for class in [
        "unavailable",
        "permission",
        "busy",
        "unsupported_profile",
        "timeout",
        "malformed",
        "unsynchronized",
    ] {
        let backend = Script {
            devices: Vec::new(),
            error: Some(error(class)),
            calls: std::sync::atomic::AtomicUsize::new(0),
        };
        assert_eq!(backend.discover().unwrap_err().class(), class);
        assert_eq!(
            backend
                .capture_color(None, Duration::from_millis(1), 8)
                .unwrap_err()
                .class(),
            class
        );
    }
}
