//! Optional continuous recorder of complete synchronized sets.

use std::sync::atomic::{AtomicU64, Ordering};

use super::store::{SyncChannel, SyncSet, SyncStore};
use crate::frame_sets::{CapturedFrameSet, FrameSetError, FrameSetSource};

/// Writes complete captured sets into synchronized history.
pub struct SyncRecorder {
    store: SyncStore,
    next_id: AtomicU64,
}

impl SyncRecorder {
    /// Records into `store`.
    #[must_use]
    pub fn new(store: SyncStore) -> Self {
        Self {
            store,
            next_id: AtomicU64::new(1),
        }
    }

    /// Shared store used by color video reads.
    #[must_use]
    pub fn store(&self) -> &SyncStore {
        &self.store
    }

    /// Inserts one complete frame. Only the color JPEG is written.
    pub async fn record(
        &self,
        source: &FrameSetSource,
        set: &CapturedFrameSet,
    ) -> Result<String, FrameSetError> {
        let Some(color) = set
            .channels()
            .iter()
            .find(|channel| channel.role().is_color() && channel.media_type() == "image/jpeg")
        else {
            return Err(FrameSetError::invalid(
                "channels",
                "a recorded set needs a color JPEG",
            ));
        };
        let id = format!("sync-{}", self.next_id.fetch_add(1, Ordering::Relaxed));
        let stored = SyncSet {
            id: id.clone(),
            source_id: source.id().as_str().to_owned(),
            serial: source.serial().to_owned(),
            captured_at: set.captured_at().to_owned(),
            timestamp_us: set.timestamp_us(),
            stereo: set.has_stereo(),
            channels: vec![SyncChannel {
                channel_id: color.id().as_str().to_owned(),
                media_type: color.media_type().to_owned(),
                payload: color.data().to_vec(),
            }],
        };
        self.store.insert(&stored).await?;
        Ok(id)
    }
}

#[cfg(test)]
mod tests {
    use serde_json::Map;

    use super::SyncRecorder;
    use crate::cameras::png::encode_gray;
    use crate::frame_sets::sync::SyncStore;
    use crate::frame_sets::{
        CapturedFrameSet, ChannelCalibration, ChannelProfile, ChannelRole, DepthScale, DepthUnit,
        FrameChannelId, FrameChannelParts, FrameSetSource, FrameSetSourceId, frame_channel,
    };

    fn jpeg() -> Vec<u8> {
        vec![0xFF, 0xD8, 0xFF, 0xD9]
    }

    fn profile(id: &str, role: ChannelRole, media: &str, depth: u8) -> ChannelProfile {
        ChannelProfile::new(
            FrameChannelId::new(id).unwrap(),
            role,
            media,
            1,
            1,
            depth,
            5.0,
        )
        .unwrap()
    }

    #[tokio::test]
    #[allow(clippy::too_many_lines)]
    async fn sync_history_keeps_only_the_color_jpeg() {
        let root = std::env::temp_dir().join(format!("sync-color-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        let source = FrameSetSource::new(
            FrameSetSourceId::new("shared-camera").unwrap(),
            "generic",
            "ABC",
            "stereo",
            vec![
                profile("color", ChannelRole::color(), "image/jpeg", 8),
                profile("depth", ChannelRole::depth(), "image/png", 16),
                profile(
                    "infrared-left",
                    ChannelRole::infrared_left(),
                    "image/png",
                    8,
                ),
                profile(
                    "infrared-right",
                    ChannelRole::infrared_right(),
                    "image/png",
                    8,
                ),
            ],
            "device:ABC",
            100_000,
        )
        .unwrap();
        let scale = DepthScale::new(1.0, DepthUnit::Millimeter).unwrap();
        let set = CapturedFrameSet::new(
            &source,
            "2026-11-05T00:00:00Z",
            vec![
                frame_channel(FrameChannelParts {
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
                }),
                frame_channel(FrameChannelParts {
                    id: FrameChannelId::new("depth").unwrap(),
                    role: ChannelRole::depth(),
                    media_type: "image/png".to_owned(),
                    width: 1,
                    height: 1,
                    bit_depth: 16,
                    device_timestamp_us: 1_000,
                    frame_number: 1,
                    depth: Some(scale),
                    calibration: ChannelCalibration::default(),
                    attributes: Map::new(),
                    data: encode_gray(&[0, 1], 1, 1, 16).unwrap(),
                }),
                frame_channel(FrameChannelParts {
                    id: FrameChannelId::new("infrared-left").unwrap(),
                    role: ChannelRole::infrared_left(),
                    media_type: "image/png".to_owned(),
                    width: 1,
                    height: 1,
                    bit_depth: 8,
                    device_timestamp_us: 1_000,
                    frame_number: 1,
                    depth: None,
                    calibration: ChannelCalibration::default(),
                    attributes: Map::new(),
                    data: encode_gray(&[0], 1, 1, 8).unwrap(),
                }),
                frame_channel(FrameChannelParts {
                    id: FrameChannelId::new("infrared-right").unwrap(),
                    role: ChannelRole::infrared_right(),
                    media_type: "image/png".to_owned(),
                    width: 1,
                    height: 1,
                    bit_depth: 8,
                    device_timestamp_us: 1_000,
                    frame_number: 1,
                    depth: None,
                    calibration: ChannelCalibration::default(),
                    attributes: Map::new(),
                    data: encode_gray(&[0], 1, 1, 8).unwrap(),
                }),
            ],
            &[],
            Map::new(),
            4096,
        )
        .unwrap();
        let store = SyncStore::open_with_limits(
            &root.join("sync.sqlite"),
            &root.join("chunks"),
            crate::frame_sets::PAYLOAD_QUOTA,
            0,
        )
        .await
        .unwrap();
        let recorder = SyncRecorder::new(store.clone());
        let id = recorder.record(&source, &set).await.unwrap();
        assert_eq!(store.color_payload(&id).await.unwrap(), Some(jpeg()));
        assert_eq!(
            store.payload_bytes().await.unwrap(),
            u64::try_from(jpeg().len()).unwrap()
        );
        let _ = std::fs::remove_dir_all(&root);
    }
}
