//! Hardware-free native capture profile candidate ordering.

use a2a_lab_dev_kit::videos::CaptureFormatPreference;
use a2a_lab_ot2::videos::{
    LiveCameraInputFormat, LiveVideoDelivery, LiveVideoSourceDescriptor,
    native_capture_profile_candidates,
};

fn custom_source() -> LiveVideoSourceDescriptor {
    LiveVideoSourceDescriptor {
        id: "mac-built-in".to_owned(),
        name: "Mac built-in".to_owned(),
        description: "test".to_owned(),
        device_index: 0,
        input: LiveCameraInputFormat::Mjpeg,
        width: 1280,
        height: 720,
        delivery: LiveVideoDelivery::H264,
    }
}

#[test]
fn legacy_source_keeps_single_exact_request() {
    let descriptor = LiveVideoSourceDescriptor::legacy_camera(4);
    let candidates = native_capture_profile_candidates(&descriptor, 5).unwrap();
    assert_eq!(candidates.len(), 1);
    assert_eq!(candidates[0].preference(), CaptureFormatPreference::Mjpeg);
    assert_eq!(candidates[0].width(), 1280);
}

#[test]
fn configured_source_keeps_exact_configured_resolution() {
    let descriptor = custom_source();
    let candidates = native_capture_profile_candidates(&descriptor, 5).unwrap();
    assert_eq!(candidates.len(), 1);
    assert_eq!(candidates[0].preference(), CaptureFormatPreference::Mjpeg);
    assert_eq!(candidates[0].width(), 1280);
    assert_eq!(candidates[0].height(), 720);
}
