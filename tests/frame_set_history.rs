//! History behavior for the portable frame-set catalog.

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use a2a_lab_ot2::frame_sets::{
    CaptureRequest, ChannelCalibration, ChannelProfile, ChannelRole, DepthScale, DepthUnit,
    FrameChannelId, FrameChannelParts, FrameSetCapture, FrameSetCatalog, FrameSetError,
    FrameSetSource, FrameSetSourceId, frame_channel,
};
use serde_json::json;

fn scratch(name: &str) -> PathBuf {
    let path = std::env::temp_dir().join(format!(
        "a2a-lab-ot2-frame-sets-{name}-{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&path);
    std::fs::create_dir_all(&path).unwrap();
    path
}

fn profile(id: &str, role: ChannelRole, media: &str, bit_depth: u8) -> ChannelProfile {
    ChannelProfile::new(
        FrameChannelId::new(id).unwrap(),
        role,
        media,
        1,
        1,
        bit_depth,
        30.0,
    )
    .unwrap()
}

fn camera(id: &str) -> FrameSetSource {
    FrameSetSource::new(
        FrameSetSourceId::new(id).unwrap(),
        "generic",
        format!("serial-{id}"),
        "stereo-camera",
        vec![
            profile("color", ChannelRole::color(), "image/jpeg", 8),
            profile("depth", ChannelRole::depth(), "image/png", 16),
        ],
        format!("group-{id}"),
        1_000,
    )
    .unwrap()
}

fn jpeg() -> Vec<u8> {
    vec![0xFF, 0xD8, 0xFF, 0xD9]
}

fn png() -> Vec<u8> {
    let mut data = b"\x89PNG\r\n\x1a\n".to_vec();
    data.extend_from_slice(&13u32.to_be_bytes());
    data.extend_from_slice(b"IHDR");
    data.extend_from_slice(&1u32.to_be_bytes());
    data.extend_from_slice(&1u32.to_be_bytes());
    data.extend_from_slice(&[16, 0, 0, 0, 0, 0, 0, 0, 0]);
    data
}

struct Fake {
    sources: Vec<FrameSetSource>,
    captures: AtomicUsize,
}

impl FrameSetCapture for Fake {
    fn sources(&self) -> Result<Vec<FrameSetSource>, FrameSetError> {
        Ok(self.sources.clone())
    }

    fn capture(
        &self,
        request: &CaptureRequest,
    ) -> Result<a2a_lab_ot2::frame_sets::CapturedFrameSet, FrameSetError> {
        self.captures.fetch_add(1, Ordering::SeqCst);
        let source = self
            .sources
            .iter()
            .find(|source| source.id() == request.source_id())
            .ok_or_else(|| FrameSetError::NotFound {
                kind: "frame-set source",
                id: request.source_id().to_string(),
            })?;
        let color = frame_channel(FrameChannelParts {
            id: FrameChannelId::new("color").unwrap(),
            role: ChannelRole::color(),
            media_type: "image/jpeg".to_owned(),
            width: 1,
            height: 1,
            bit_depth: 8,
            device_timestamp_us: 10,
            frame_number: 3,
            depth: None,
            calibration: ChannelCalibration::default(),
            attributes: json!({"stream": "color"}).as_object().cloned().unwrap(),
            data: jpeg(),
        });
        let depth = frame_channel(FrameChannelParts {
            id: FrameChannelId::new("depth").unwrap(),
            role: ChannelRole::depth(),
            media_type: "image/png".to_owned(),
            width: 1,
            height: 1,
            bit_depth: 16,
            device_timestamp_us: 12,
            frame_number: 3,
            depth: Some(DepthScale::new(1.0, DepthUnit::Millimeter).unwrap()),
            calibration: ChannelCalibration::default(),
            attributes: serde_json::Map::new(),
            data: png(),
        });
        a2a_lab_ot2::frame_sets::CapturedFrameSet::new(
            source,
            "2026-10-30T01:02:03Z",
            vec![color, depth],
            request.channel_ids(),
            serde_json::Map::new(),
            request.max_channel_bytes(),
        )
    }
}

async fn open_catalog(dir: &std::path::Path, retention: usize) -> (FrameSetCatalog, Arc<Fake>) {
    let fake = Arc::new(Fake {
        sources: vec![camera("left"), camera("right")],
        captures: AtomicUsize::new(0),
    });
    let catalog = FrameSetCatalog::open(
        &dir.join("frames.sqlite"),
        Arc::clone(&fake) as Arc<dyn FrameSetCapture>,
        retention,
        1024,
    )
    .await
    .unwrap();
    (catalog, fake)
}

#[tokio::test]
async fn history_restores_bytes_and_keeps_sources_isolated() {
    let dir = scratch("restore");
    let (catalog, fake) = open_catalog(&dir, 2).await;
    assert_eq!(catalog.sources().len(), 2);
    let left =
        CaptureRequest::new(catalog.sources()[0].id().clone(), Vec::new(), 1000, 1024).unwrap();
    let right =
        CaptureRequest::new(catalog.sources()[1].id().clone(), Vec::new(), 1000, 1024).unwrap();
    let first = catalog.capture(&left).await.unwrap();
    let _second = catalog.capture(&left).await.unwrap();
    let other = catalog.capture(&right).await.unwrap();
    let third = catalog.capture(&left).await.unwrap();
    assert!(catalog.get(first.id()).await.is_err());
    assert!(
        catalog
            .channel(first.id(), &FrameChannelId::new("color").unwrap())
            .await
            .is_err()
    );
    assert_eq!(
        catalog.get(other.id()).await.unwrap().channels()[0].data(),
        jpeg()
    );
    drop(catalog);
    let (reopened, _) = open_catalog(&dir, 2).await;
    let restored = reopened.get(third.id()).await.unwrap();
    assert_eq!(restored.channels()[0].data(), jpeg());
    assert_eq!(restored.channels()[1].data(), png());
    let scale = restored.channels()[1].depth().unwrap().scale();
    assert!((scale - 1.0).abs() < f64::EPSILON);
    let listed = reopened.list(third.source_id(), None, 10).await.unwrap();
    assert_eq!(listed.items().len(), 2);
    assert!(
        listed
            .items()
            .iter()
            .all(|item| item.channels().iter().all(|channel| channel.byte_len() > 0))
    );
    assert_eq!(fake.captures.load(Ordering::SeqCst), 4);
    let before = fake.captures.load(Ordering::SeqCst);
    let _ = reopened.search(None, "depth", None, 1).await.unwrap();
    assert_eq!(fake.captures.load(Ordering::SeqCst), before);
    let page = reopened.list(third.source_id(), None, 1).await.unwrap();
    assert_eq!(page.items().len(), 1);
    let next = reopened
        .list(third.source_id(), Some(page.next_cursor().unwrap()), 1)
        .await
        .unwrap();
    assert_eq!(next.items().len(), 1);
    assert_ne!(page.items()[0].id(), next.items()[0].id());
}

#[tokio::test]
async fn corrupt_history_is_rejected_on_startup() {
    let dir = scratch("corrupt");
    let (catalog, _) = open_catalog(&dir, 2).await;
    let request =
        CaptureRequest::new(catalog.sources()[0].id().clone(), Vec::new(), 1000, 1024).unwrap();
    catalog.capture(&request).await.unwrap();
    drop(catalog);
    let pool = sqlx::sqlite::SqlitePoolOptions::new()
        .connect(&format!("sqlite://{}", dir.join("frames.sqlite").display()))
        .await
        .unwrap();
    sqlx::query("UPDATE frame_channels SET data = x'00' WHERE role = 'color'")
        .execute(&pool)
        .await
        .unwrap();
    pool.close().await;
    let opened = FrameSetCatalog::open(
        &dir.join("frames.sqlite"),
        Arc::new(Fake {
            sources: vec![camera("left"), camera("right")],
            captures: AtomicUsize::new(0),
        }) as Arc<dyn FrameSetCapture>,
        2,
        1024,
    )
    .await;
    let Err(error) = opened else {
        panic!("corrupt history opened");
    };
    assert_eq!(error.class(), "unavailable");
    assert!(error.to_string().contains("unusable"));
}
