//! Hardware-free profile diagnostic formatting and strict negotiation failures.

use a2a_lab_dev_kit::videos::{
    AdvertisedCaptureProfile, CaptureFormat, CaptureFormatPreference, CaptureMode,
    CaptureProfileRequest, CaptureRate, CaptureSession, describe_capture_profile_error,
    format_advertised_capture_profiles, format_capture_profile_request,
};
use a2a_lab_dev_kit::{A2aLabError, VideoSourceId};
use a2a_lab_ot2::videos::native_capture_profile_candidates;
use a2a_lab_ot2::{
    ExplicitSetting, LabConfigInputs, LiveCameraInputFormat, LiveVideoDelivery,
    LiveVideoSourceDescriptor, resolve_lab_config,
};

fn profile(format: CaptureFormat, width: u32, height: u32, fps: u32) -> AdvertisedCaptureProfile {
    AdvertisedCaptureProfile::new(
        CaptureMode::new(format, width, height).expect("mode"),
        CaptureRate::fps(fps).expect("rate"),
    )
}

struct DiagnosticCamera {
    advertised: Vec<AdvertisedCaptureProfile>,
}

impl a2a_lab_dev_kit::videos::ColorCamera for DiagnosticCamera {
    fn advertised(&mut self) -> Result<Vec<AdvertisedCaptureProfile>, A2aLabError> {
        Ok(self.advertised.clone())
    }

    fn open(
        &mut self,
        _profile: &a2a_lab_dev_kit::videos::SelectedCaptureProfile,
    ) -> Result<a2a_lab_dev_kit::videos::SelectedCaptureProfile, A2aLabError> {
        Err(A2aLabError::invalid("camera", "injected open failure"))
    }

    fn raw_frame(&mut self) -> Result<a2a_lab_dev_kit::videos::RawCameraSample, A2aLabError> {
        Err(A2aLabError::unavailable("not used"))
    }

    fn stop(&mut self) -> Result<(), A2aLabError> {
        Ok(())
    }
}

#[test]
fn configured_source_yields_single_explicit_candidate() {
    let descriptor = LiveVideoSourceDescriptor {
        id: "usb-webcam".to_owned(),
        name: "USB".to_owned(),
        description: "test".to_owned(),
        device_index: 1,
        input: LiveCameraInputFormat::Mjpeg,
        width: 1920,
        height: 1080,
        delivery: LiveVideoDelivery::H264,
    };
    let candidates = native_capture_profile_candidates(&descriptor, 5).unwrap();
    assert_eq!(candidates.len(), 1);
    assert_eq!(candidates[0].width(), 1920);
    assert_eq!(candidates[0].height(), 1080);
    assert_eq!(candidates[0].preference(), CaptureFormatPreference::Mjpeg);
}

#[test]
fn negotiation_failure_lists_configured_and_advertised_profiles() {
    let descriptor = LiveVideoSourceDescriptor {
        id: "mac-built-in".to_owned(),
        name: "Built-in".to_owned(),
        description: "test".to_owned(),
        device_index: 0,
        input: LiveCameraInputFormat::Mjpeg,
        width: 1280,
        height: 720,
        delivery: LiveVideoDelivery::H264,
    };
    let advertised = vec![
        profile(CaptureFormat::Mjpeg, 1920, 1080, 30),
        profile(CaptureFormat::Yuy2, 1280, 720, 30),
    ];
    let request = descriptor.capture_request(5).unwrap();
    let error = CaptureSession::new(
        DiagnosticCamera {
            advertised: advertised.clone(),
        },
        request,
    )
    .open()
    .unwrap_err()
    .to_string();
    let listed = format_advertised_capture_profiles(&advertised);
    assert!(error.contains("configured profile MJPEG 1280x720 output 5fps"));
    assert!(error.contains("available:"));
    assert!(error.contains(&listed));
    assert!(error.contains("MJPEG 1920x1080@30fps"));
}

#[test]
fn profile_request_and_list_formatting_are_stable() {
    let request = CaptureProfileRequest::new(
        CaptureFormatPreference::Mjpeg,
        1280,
        720,
        CaptureRate::fps(5).unwrap(),
    )
    .unwrap();
    assert_eq!(
        format_capture_profile_request(&request),
        "MJPEG 1280x720 output 5fps"
    );
    let advertised = [profile(CaptureFormat::Yuy2, 640, 480, 15)];
    assert_eq!(
        format_advertised_capture_profiles(&advertised),
        "YUYV 640x480@15fps"
    );
    let message = describe_capture_profile_error(
        &request,
        &a2a_lab_dev_kit::videos::CaptureProfileError::Unsupported {
            available: advertised.to_vec(),
        },
        &advertised,
    );
    assert!(message.contains("YUYV 640x480@15fps"));
    let mut session = CaptureSession::new(
        DiagnosticCamera {
            advertised: advertised.to_vec(),
        },
        request,
    );
    let open_error = session.open().unwrap_err().to_string();
    assert!(open_error.contains("available: YUYV 640x480@15fps"));
    let _ = VideoSourceId::new("mac-built-in").unwrap();
}

#[test]
fn custom_source_without_width_fails_resolve() {
    let dir = std::env::temp_dir().join(format!("a2a-lab-ot2-missing-size-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("a2a-lab.json");
    std::fs::write(
        &path,
        r#"{"schema_version":1,"videos":{"sources":[{"id":"mac-built-in","format":"mjpeg","device_index":0}]}}"#,
    )
    .unwrap();
    let inputs = LabConfigInputs {
        config_path: ExplicitSetting::cli(Some(path)),
        ..LabConfigInputs::default()
    };
    let error = resolve_lab_config(&inputs).unwrap_err().to_string();
    assert!(error.contains("videos.sources.width"));
    let _ = std::fs::remove_dir_all(dir);
}
