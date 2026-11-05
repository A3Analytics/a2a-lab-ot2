//! Synchronized history is one copy; color reads those slices.

use std::path::PathBuf;
use std::sync::Arc;

use a2a_lab_ot2::cameras::{SharedSession, StreamingDevice};
use a2a_lab_ot2::frame_sets::{
    CaptureRequest, CapturedFrameSet, ChannelCalibration, ChannelProfile, ChannelRole,
    FrameChannelId, FrameChannelParts, FrameSetError, FrameSetSource, FrameSetSourceId,
    SyncRecorder, SyncStore, frame_channel, validate_sync_paths,
};
use serde_json::Map;

struct Once {
    source: FrameSetSource,
    set: CapturedFrameSet,
}

impl StreamingDevice for Once {
    fn describe(&self) -> Result<FrameSetSource, FrameSetError> {
        Ok(self.source.clone())
    }

    fn next(&self, _request: &CaptureRequest) -> Result<CapturedFrameSet, FrameSetError> {
        Ok(self.set.clone())
    }
}

fn jpeg() -> Vec<u8> {
    vec![0xFF, 0xD8, 0xFF, 0xD9]
}

#[tokio::test]
async fn color_history_is_one_copy_without_a_second_mjpeg_tree() {
    let root = std::env::temp_dir().join(format!("sync-wire-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    let dvr = root.join("video-dvr");
    std::fs::create_dir_all(&dvr).unwrap();
    let source = FrameSetSource::new(
        FrameSetSourceId::new("shared-camera").unwrap(),
        "generic",
        "ABC",
        "color",
        vec![
            ChannelProfile::new(
                FrameChannelId::new("color").unwrap(),
                ChannelRole::color(),
                "image/jpeg",
                1,
                1,
                8,
                5.0,
            )
            .unwrap(),
        ],
        "device:ABC",
        100_000,
    )
    .unwrap();
    let set = CapturedFrameSet::new(
        &source,
        "2026-11-05T00:00:00Z",
        vec![frame_channel(FrameChannelParts {
            id: FrameChannelId::new("color").unwrap(),
            role: ChannelRole::color(),
            media_type: "image/jpeg".to_owned(),
            width: 1,
            height: 1,
            bit_depth: 8,
            device_timestamp_us: 1_000,
            frame_number: 1,
            depth: None,
            calibration: ChannelCalibration::default(),
            attributes: Map::new(),
            data: jpeg(),
        })],
        &[],
        Map::new(),
        1024,
    )
    .unwrap();
    let session = SharedSession::new(Arc::new(Once {
        source: source.clone(),
        set,
    }));
    let captured = session
        .pump_once(&CaptureRequest::new(source.id().clone(), Vec::new(), 1_000, 1024).unwrap())
        .unwrap();
    let store = SyncStore::open_with_limits(
        &root.join("sync.sqlite"),
        &root.join("chunks"),
        a2a_lab_ot2::frame_sets::PAYLOAD_QUOTA,
        0,
    )
    .await
    .unwrap();
    let recorder = SyncRecorder::new(store.clone());
    let id = recorder.record(&source, &captured).await.unwrap();
    assert_eq!(store.color_payload(&id).await.unwrap(), Some(jpeg()));
    assert!(!dvr.join("orbbec-camera").exists());
    assert!(!dvr.join("shared-camera").exists());
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn overlapping_sync_and_catalog_paths_are_rejected() {
    let db = PathBuf::from("history.sqlite");
    assert!(
        validate_sync_paths(&db, &db, &PathBuf::from("chunks"), &PathBuf::from("dvr")).is_err()
    );
}
