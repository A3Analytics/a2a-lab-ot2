//! Hardware-free capture preference resolution and validation.

use a2a_lab_dev_kit::videos::{
    AdvertisedCaptureProfile, CaptureFormat, CaptureFormatPreference, CaptureMode,
    CaptureProfileRequest, CaptureRate, CaptureSession,
};
use a2a_lab_dev_kit::{A2aLabError, VideoInputFormat};
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
fn auto_format_parses_from_config_file() {
    let dir = std::env::temp_dir().join(format!("a2a-lab-ot2-auto-format-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("a2a-lab.json");
    std::fs::write(
        &path,
        r#"{"schema_version":1,"videos":{"sources":[{"id":"mac-built-in","format":"auto","width":1280,"height":720,"capture_rate":30,"device_index":0}]}}"#,
    )
    .unwrap();
    let resolved = resolve_lab_config(&LabConfigInputs {
        config_path: ExplicitSetting::cli(Some(path)),
        ..LabConfigInputs::default()
    })
    .unwrap();
    assert_eq!(resolved.video.sources[0].input, LiveCameraInputFormat::Auto);
    assert_eq!(
        resolved.video.sources[0]
            .capture_request(resolved.video.fps)
            .unwrap()
            .preference(),
        CaptureFormatPreference::Auto
    );
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn default_output_fps_stays_five_without_file_override() {
    let resolved = resolve_lab_config(&LabConfigInputs::default()).unwrap();
    assert_eq!(resolved.video.fps, 5);
}

#[test]
fn legacy_external_camera_stays_mjpeg_1080() {
    let external = LiveVideoSourceDescriptor::legacy_external(2, "overview");
    assert_eq!(external.input, LiveCameraInputFormat::Mjpeg);
    assert_eq!(external.width, Some(1920));
    assert_eq!(external.height, Some(1080));
    let request = external.capture_request(5).unwrap();
    assert_eq!(request.preference(), CaptureFormatPreference::Mjpeg);
    assert_eq!(request.width(), Some(1920));
    assert_eq!(request.height(), Some(1080));
    assert_eq!(request.output_rate().numerator(), 5);
}

#[test]
fn unsupported_1080_yuy2_reports_available_modes() {
    let (camera_id, device_index) = a2a_lab_ot2::index_camera_fields(0);
    let descriptor = LiveVideoSourceDescriptor {
        id: "external-camera".to_owned(),
        name: "External".to_owned(),
        description: "HD raw".to_owned(),
        camera_id,
        device_index,
        input: LiveCameraInputFormat::Yuy2,
        width: Some(1920),
        height: Some(1080),
        capture_rate: Some(5),
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
    assert!(error.contains("YUYV 1920x1080@5fps output 5fps"), "{error}");
    assert!(error.contains("available:"), "{error}");
    assert!(error.contains("MJPEG 1920x1080@30fps"), "{error}");
    assert!(error.contains("YUYV 1280x720@30fps"), "{error}");
}

#[test]
fn odd_width_yuy2_fails_resolve_before_open() {
    let dir = std::env::temp_dir().join(format!("a2a-lab-ot2-odd-yuy2-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("a2a-lab.json");
    std::fs::write(
        &path,
        r#"{"schema_version":1,"videos":{"sources":[{"id":"raw","format":"yuy2","width":1281,"height":720,"capture_rate":30,"device_index":0}]}}"#,
    )
    .unwrap();
    let error = resolve_lab_config(&LabConfigInputs {
        config_path: ExplicitSetting::cli(Some(path)),
        ..LabConfigInputs::default()
    })
    .unwrap_err()
    .to_string();
    assert!(error.contains("width"), "{error}");
    assert!(error.contains("YUY2"), "{error}");
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn zero_output_fps_fails_resolve() {
    let dir = std::env::temp_dir().join(format!("a2a-lab-ot2-zero-fps-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("a2a-lab.json");
    std::fs::write(
        &path,
        r#"{"schema_version":1,"videos":{"sources":[{"id":"cam","format":"mjpeg","width":640,"height":480,"capture_rate":30,"device_index":0}],"fps":0}}"#,
    )
    .unwrap();
    let error = resolve_lab_config(&LabConfigInputs {
        config_path: ExplicitSetting::cli(Some(path)),
        ..LabConfigInputs::default()
    })
    .unwrap_err()
    .to_string();
    assert!(
        error.contains("videos.fps") || error.contains("video_fps"),
        "{error}"
    );
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn yuyv_alias_normalizes_to_yuy2_preference() {
    let document = r#"{"schema_version":1,"videos":{"sources":[{"id":"cam","format":"yuyv","width":640,"height":480,"capture_rate":15,"device_index":0}]}}"#;
    let config = a2a_lab_dev_kit::A2aLabConfig::parse(document).unwrap();
    let source = &config.videos.unwrap().sources[0];
    assert_eq!(source.format, VideoInputFormat::Yuy2);
    assert_eq!(
        source.format.capture_preference(),
        CaptureFormatPreference::Yuy2
    );
    let request = CaptureProfileRequest::exact(
        source.format.capture_preference(),
        640,
        480,
        CaptureRate::fps(5).unwrap(),
        CaptureRate::fps(15).unwrap(),
    )
    .unwrap();
    assert_eq!(request.preference(), CaptureFormatPreference::Yuy2);
}
