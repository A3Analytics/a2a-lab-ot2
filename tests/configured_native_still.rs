//! Still capture for configured native video source ids.

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use a2a_lab_ot2::camera::{
    CameraCapture, CameraCaptureError, ConfiguredNativeStill, ExternalCameraFormat,
    remember_live_still,
};
use a2a_lab_ot2::{
    ImageCapture, LiveCameraInputFormat, LiveVideoDelivery, LiveVideoSourceDescriptor,
};

fn jpeg() -> Vec<u8> {
    let mut data = vec![0xFF, 0xD8, 0xFF, 0xC0, 0x00, 0x0B, 0x08];
    data.extend_from_slice(&4_u16.to_be_bytes());
    data.extend_from_slice(&4_u16.to_be_bytes());
    data.extend_from_slice(&[0x01, 0x01, 0x11, 0x00, 0xFF, 0xD9]);
    data
}

fn descriptor(id: &str) -> LiveVideoSourceDescriptor {
    LiveVideoSourceDescriptor {
        id: id.to_owned(),
        name: format!("Name {id}"),
        description: format!("Description {id}"),
        device_index: 7,
        input: LiveCameraInputFormat::Mjpeg,
        width: Some(4),
        height: Some(4),
        capture_rate: Some(5),
        delivery: LiveVideoDelivery::Mjpeg,
    }
}

struct CountingCapture {
    calls: Arc<AtomicUsize>,
    bytes: Vec<u8>,
}

impl CameraCapture for CountingCapture {
    fn capture(
        &self,
        _index: u32,
        _format: ExternalCameraFormat,
        _frames: u32,
    ) -> Result<Vec<u8>, CameraCaptureError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        Ok(self.bytes.clone())
    }
}

#[tokio::test]
async fn configured_still_reuses_live_buffer_without_opening_device() {
    let calls = Arc::new(AtomicUsize::new(0));
    let frame = jpeg();
    remember_live_still("mac-built-in", frame.clone());
    let still = ConfiguredNativeStill::from_descriptor(
        &descriptor("mac-built-in"),
        std::time::Duration::from_millis(500),
        64 * 1024,
        Some(Arc::new(CountingCapture {
            calls: Arc::clone(&calls),
            bytes: jpeg(),
        })),
    )
    .unwrap();
    let captured = still.capture().await.unwrap();
    assert_eq!(captured.data(), frame.as_slice());
    assert_eq!(calls.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn configured_still_reports_unavailable_when_pump_and_device_fail() {
    let still = ConfiguredNativeStill::from_descriptor(
        &descriptor("usb-webcam"),
        std::time::Duration::from_millis(50),
        64 * 1024,
        Some(Arc::new(CountingCapture {
            calls: Arc::new(AtomicUsize::new(0)),
            bytes: vec![0xFF, 0xD8, 0xFF, 0xD9],
        })),
    )
    .unwrap();
    let error = still.capture().await.unwrap_err().to_string();
    assert!(error.contains("usb-webcam"));
    assert!(error.contains("device index 7"));
}
