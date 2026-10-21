//! Durable camera history.

use std::future::Future;
use std::pin::Pin;
use std::process::Command;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use a2a_lab_dev_kit::{
    A2aLabError, GetCurrentImageRequest, GetImageRequest, ImageId, ImageProvider, ImageSource,
    ImageSourceId, JsonObject, ListImagesRequest, PageRequest, SearchImagesRequest, UtcTimestamp,
};
use a2a_lab_ot2::camera::EXTERNAL_CAMERA_SOURCE_ID;
use a2a_lab_ot2::{CapturedFrame, ImageCapture, ImageCatalogConfig, LiveImageCatalog};

fn bin() -> Command {
    Command::new(env!("CARGO_BIN_EXE_a2a-lab-ot2"))
}

fn scratch(name: &str) -> std::path::PathBuf {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let path = std::env::temp_dir().join(format!(
        "a2a-lab-ot2-{name}-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::SeqCst)
    ));
    let _ = std::fs::remove_dir_all(&path);
    std::fs::create_dir_all(&path).unwrap();
    path
}

fn page() -> PageRequest {
    PageRequest::new(None, 20).unwrap()
}

fn source(id: &str) -> ImageSource {
    ImageSource {
        id: ImageSourceId::new(id).unwrap(),
        name: id.to_owned(),
        description: format!("{id} camera"),
        asset_id: None,
        semantic_id: None,
    }
}

fn frame(at: UtcTimestamp, caption: &str, byte: u8) -> CapturedFrame {
    CapturedFrame::new(
        at,
        "image/jpeg",
        2,
        2,
        Some(caption.to_owned()),
        JsonObject::parse(r#"{"view":"deck"}"#).unwrap(),
        vec![0xFF, 0xD8, byte, 0xD9],
    )
}

struct Script {
    calls: AtomicUsize,
    frames: Mutex<Vec<Result<CapturedFrame, A2aLabError>>>,
}

impl Script {
    fn new(frames: Vec<Result<CapturedFrame, A2aLabError>>) -> Arc<Self> {
        Arc::new(Self {
            calls: AtomicUsize::new(0),
            frames: Mutex::new(frames),
        })
    }

    fn calls(&self) -> usize {
        self.calls.load(Ordering::SeqCst)
    }
}

impl ImageCapture for Script {
    fn capture(
        &self,
    ) -> Pin<Box<dyn Future<Output = Result<CapturedFrame, A2aLabError>> + Send + '_>> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        let result = {
            let mut frames = self.frames.lock().expect("frames");
            if frames.is_empty() {
                Err(A2aLabError::unavailable("scripted capture is exhausted"))
            } else {
                frames.remove(0)
            }
        };
        Box::pin(std::future::ready(result))
    }
}

fn catalog(sources: Vec<(ImageSource, Arc<Script>)>, retention: u64) -> LiveImageCatalog {
    LiveImageCatalog::new(
        sources
            .into_iter()
            .map(|(source, capture)| (source, Arc::clone(&capture) as Arc<dyn ImageCapture>))
            .collect(),
        ImageCatalogConfig::new(retention, 1024).unwrap(),
    )
    .unwrap()
}

async fn persisted(
    sources: Vec<(ImageSource, Arc<Script>)>,
    retention: u64,
    path: &std::path::Path,
    source_id: &ImageSourceId,
) -> LiveImageCatalog {
    persisted_sources(sources, retention, path, std::slice::from_ref(source_id)).await
}

async fn persisted_sources(
    sources: Vec<(ImageSource, Arc<Script>)>,
    retention: u64,
    path: &std::path::Path,
    source_ids: &[ImageSourceId],
) -> LiveImageCatalog {
    catalog(sources, retention)
        .with_persisted_sources(source_ids, path)
        .await
        .unwrap()
}

async fn ids(catalog: &LiveImageCatalog, source_id: &ImageSourceId) -> Vec<String> {
    catalog
        .list_images(ListImagesRequest::new(source_id.clone(), page()).unwrap())
        .await
        .unwrap()
        .items()
        .iter()
        .map(|item| item.id().as_str().to_owned())
        .collect()
}

#[tokio::test]
async fn requested_frames_from_both_sources_survive_restart() {
    let dir = scratch("restart");
    let db = dir.join("images.sqlite");
    let external_id = ImageSourceId::new(EXTERNAL_CAMERA_SOURCE_ID).unwrap();
    let robot_id = ImageSourceId::new("opentrons-camera").unwrap();
    let now = a2a_lab_ot2::time::now().unwrap();
    let external = Script::new(vec![Ok(frame(now, "deck view", 1))]);
    let robot = Script::new(vec![Ok(frame(now, "on board", 2))]);
    let persisted_ids = [external_id.clone(), robot_id.clone()];
    let first = persisted_sources(
        vec![
            (source(EXTERNAL_CAMERA_SOURCE_ID), Arc::clone(&external)),
            (source("opentrons-camera"), Arc::clone(&robot)),
        ],
        4,
        &db,
        &persisted_ids,
    )
    .await;
    assert_eq!(external.calls(), 0);
    let saved = first
        .get_current_image(GetCurrentImageRequest::new(external_id.clone()))
        .await
        .unwrap();
    let saved_robot = first
        .get_current_image(GetCurrentImageRequest::new(robot_id.clone()))
        .await
        .unwrap();
    assert_eq!(external.calls(), 1);
    drop(first);

    let external = Script::new(vec![Ok(frame(now, "again", 3))]);
    let robot = Script::new(vec![]);
    let second = persisted_sources(
        vec![
            (source(EXTERNAL_CAMERA_SOURCE_ID), Arc::clone(&external)),
            (source("opentrons-camera"), Arc::clone(&robot)),
        ],
        4,
        &db,
        &persisted_ids,
    )
    .await;
    assert_eq!(external.calls(), 0);
    assert_eq!(robot.calls(), 0);
    assert_eq!(
        ids(&second, &external_id).await,
        vec![saved.descriptor().id().as_str()]
    );
    assert_eq!(
        ids(&second, &robot_id).await,
        vec![saved_robot.descriptor().id().as_str()]
    );
    let robot_loaded = second
        .get_image(GetImageRequest::new(saved_robot.descriptor().id().clone()))
        .await
        .unwrap();
    assert_eq!(robot_loaded.data(), saved_robot.data());
    let listed = second
        .search_images(
            SearchImagesRequest::new(
                Some(external_id.clone()),
                None,
                Some("Deck".to_owned()),
                page(),
            )
            .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(listed.items().len(), 1);
    assert_eq!(external.calls(), 0);
    let loaded = second
        .get_image(GetImageRequest::new(saved.descriptor().id().clone()))
        .await
        .unwrap();
    assert_eq!(loaded.data(), saved.data());
    let next = second
        .get_current_image(GetCurrentImageRequest::new(external_id.clone()))
        .await
        .unwrap();
    assert!(next.descriptor().id().as_str() > saved.descriptor().id().as_str());
    assert_eq!(ids(&second, &external_id).await.len(), 2);
    std::fs::remove_dir_all(dir).ok();
}

#[tokio::test]
async fn retention_keeps_the_newest_frames_across_restart() {
    let dir = scratch("retention");
    let db = dir.join("images.sqlite");
    let source_id = ImageSourceId::new(EXTERNAL_CAMERA_SOURCE_ID).unwrap();
    let now = a2a_lab_ot2::time::now().unwrap();
    let frames = (0..3)
        .map(|index| {
            Ok(frame(
                a2a_lab_ot2::time::shift_seconds(now, index).unwrap(),
                &format!("frame-{index}"),
                u8::try_from(index).unwrap(),
            ))
        })
        .collect();
    let capture = Script::new(frames);
    let catalog = persisted(
        vec![(source(EXTERNAL_CAMERA_SOURCE_ID), Arc::clone(&capture))],
        2,
        &db,
        &source_id,
    )
    .await;
    let mut captured = Vec::new();
    for _ in 0..3 {
        captured.push(
            catalog
                .get_current_image(GetCurrentImageRequest::new(source_id.clone()))
                .await
                .unwrap()
                .descriptor()
                .id()
                .as_str()
                .to_owned(),
        );
    }
    assert_eq!(ids(&catalog, &source_id).await, captured[1..].to_vec());
    let missing = catalog
        .get_image(GetImageRequest::new(
            ImageId::new(captured[0].clone()).unwrap(),
        ))
        .await
        .unwrap_err();
    assert_eq!(missing.code(), "not_found");
    drop(catalog);
    let capture = Script::new(vec![]);
    let restored = persisted(
        vec![(source(EXTERNAL_CAMERA_SOURCE_ID), capture)],
        2,
        &db,
        &source_id,
    )
    .await;
    assert_eq!(ids(&restored, &source_id).await, captured[1..].to_vec());
    std::fs::remove_dir_all(dir).ok();
}

#[tokio::test]
async fn retention_is_applied_independently_to_each_source() {
    let dir = scratch("per-source-retention");
    let db = dir.join("images.sqlite");
    let external_id = ImageSourceId::new(EXTERNAL_CAMERA_SOURCE_ID).unwrap();
    let robot_id = ImageSourceId::new("opentrons-camera").unwrap();
    let now = a2a_lab_ot2::time::now().unwrap();
    let frames = |offset: i64| {
        (0..3)
            .map(|index| {
                Ok(frame(
                    a2a_lab_ot2::time::shift_seconds(now, offset + index).unwrap(),
                    &format!("frame-{offset}-{index}"),
                    u8::try_from(offset + index).unwrap(),
                ))
            })
            .collect()
    };
    let external = Script::new(frames(0));
    let robot = Script::new(frames(10));
    let persisted_ids = [external_id.clone(), robot_id.clone()];
    let catalog = persisted_sources(
        vec![
            (source(EXTERNAL_CAMERA_SOURCE_ID), external),
            (source("opentrons-camera"), robot),
        ],
        2,
        &db,
        &persisted_ids,
    )
    .await;
    for _ in 0..3 {
        catalog
            .get_current_image(GetCurrentImageRequest::new(external_id.clone()))
            .await
            .unwrap();
        catalog
            .get_current_image(GetCurrentImageRequest::new(robot_id.clone()))
            .await
            .unwrap();
    }
    assert_eq!(ids(&catalog, &external_id).await.len(), 2);
    assert_eq!(ids(&catalog, &robot_id).await.len(), 2);
    drop(catalog);

    let restored = persisted_sources(
        vec![
            (source(EXTERNAL_CAMERA_SOURCE_ID), Script::new(Vec::new())),
            (source("opentrons-camera"), Script::new(Vec::new())),
        ],
        2,
        &db,
        &persisted_ids,
    )
    .await;
    assert_eq!(ids(&restored, &external_id).await.len(), 2);
    assert_eq!(ids(&restored, &robot_id).await.len(), 2);
    std::fs::remove_dir_all(dir).ok();
}

#[tokio::test]
async fn frames_older_than_seven_days_expire_on_startup_and_maintenance() {
    let dir = scratch("expiry");
    let db = dir.join("images.sqlite");
    let source_id = ImageSourceId::new(EXTERNAL_CAMERA_SOURCE_ID).unwrap();
    let now = a2a_lab_ot2::time::now().unwrap();
    let old = a2a_lab_ot2::time::shift_seconds(now, -(8 * 24 * 60 * 60)).unwrap();
    let fresh = a2a_lab_ot2::time::shift_seconds(now, -60).unwrap();
    let capture = Script::new(vec![Ok(frame(old, "old", 1)), Ok(frame(fresh, "fresh", 2))]);
    let catalog = persisted(
        vec![(source(EXTERNAL_CAMERA_SOURCE_ID), Arc::clone(&capture))],
        8,
        &db,
        &source_id,
    )
    .await;
    let old_image = catalog
        .get_current_image(GetCurrentImageRequest::new(source_id.clone()))
        .await
        .unwrap();
    assert_eq!(
        ids(&catalog, &source_id).await,
        vec![old_image.descriptor().id().as_str()]
    );
    let fresh_image = catalog
        .get_current_image(GetCurrentImageRequest::new(source_id.clone()))
        .await
        .unwrap();
    assert_eq!(
        ids(&catalog, &source_id).await,
        vec![fresh_image.descriptor().id().as_str()]
    );
    drop(catalog);

    let capture = Script::new(vec![]);
    let restored = persisted(
        vec![(source(EXTERNAL_CAMERA_SOURCE_ID), capture)],
        8,
        &db,
        &source_id,
    )
    .await;
    assert_eq!(
        ids(&restored, &source_id).await,
        vec![fresh_image.descriptor().id().as_str()]
    );
    let later = a2a_lab_ot2::time::shift_seconds(now, 8 * 24 * 60 * 60).unwrap();
    restored.expire_history_at(later).await.unwrap();
    assert!(ids(&restored, &source_id).await.is_empty());
    let missing = restored
        .get_image(GetImageRequest::new(fresh_image.descriptor().id().clone()))
        .await
        .unwrap_err();
    assert_eq!(missing.code(), "not_found");
    assert!(restored.maintain_history().is_some());
    std::fs::remove_dir_all(dir).ok();
}

#[tokio::test]
async fn corrupt_or_oversized_history_is_rejected_without_a_partial_catalog() {
    let dir = scratch("corrupt");
    let db = dir.join("images.sqlite");
    let source_id = ImageSourceId::new(EXTERNAL_CAMERA_SOURCE_ID).unwrap();
    let now = a2a_lab_ot2::time::now().unwrap();
    let capture = Script::new(vec![Ok(frame(now, "deck", 4))]);
    let catalog = persisted(
        vec![(source(EXTERNAL_CAMERA_SOURCE_ID), capture)],
        4,
        &db,
        &source_id,
    )
    .await;
    let image = catalog
        .get_current_image(GetCurrentImageRequest::new(source_id.clone()))
        .await
        .unwrap();
    drop(catalog);

    let pool = history_pool(&db).await;
    sqlx::query("UPDATE image_history SET media_type = 'text/plain'")
        .execute(&pool)
        .await
        .unwrap();
    pool.close().await;
    let error = catalog_error(&db, 1024).await;
    assert_eq!(error.code(), "invalid");
    assert!(error.to_string().contains(image.descriptor().id().as_str()));
    assert_eq!(catalog_error(&db, 1024).await.code(), "invalid");

    let pool = history_pool(&db).await;
    sqlx::query("UPDATE image_history SET media_type = 'image/jpeg'")
        .execute(&pool)
        .await
        .unwrap();
    pool.close().await;
    let error = catalog_error(&db, 3).await;
    assert_eq!(error.code(), "invalid");
    assert!(error.to_string().contains("maximum"));
    let restored = persisted(
        vec![(source(EXTERNAL_CAMERA_SOURCE_ID), Script::new(vec![]))],
        4,
        &db,
        &source_id,
    )
    .await;
    assert_eq!(ids(&restored, &source_id).await.len(), 1);
    std::fs::remove_dir_all(dir).ok();
}

async fn history_pool(path: &std::path::Path) -> sqlx::SqlitePool {
    sqlx::sqlite::SqlitePoolOptions::new()
        .max_connections(1)
        .connect_with(
            sqlx::sqlite::SqliteConnectOptions::new()
                .filename(path)
                .read_only(false),
        )
        .await
        .unwrap()
}

async fn catalog_error(path: &std::path::Path, max_bytes: u64) -> A2aLabError {
    let capture: Arc<dyn ImageCapture> = Script::new(vec![]);
    let opened = LiveImageCatalog::new(
        vec![(source(EXTERNAL_CAMERA_SOURCE_ID), capture)],
        ImageCatalogConfig::new(4, max_bytes).unwrap(),
    )
    .unwrap()
    .with_persisted_source(
        &ImageSourceId::new(EXTERNAL_CAMERA_SOURCE_ID).unwrap(),
        path,
    )
    .await;
    match opened {
        Ok(_catalog) => panic!("expected image history to fail"),
        Err(error) => error,
    }
}

#[tokio::test]
async fn failed_capture_stores_nothing() {
    let dir = scratch("disabled");
    let db = dir.join("images.sqlite");
    let source_id = ImageSourceId::new(EXTERNAL_CAMERA_SOURCE_ID).unwrap();
    let capture = Script::new(vec![Err(A2aLabError::unavailable("camera unavailable"))]);
    let catalog = persisted(
        vec![(source(EXTERNAL_CAMERA_SOURCE_ID), Arc::clone(&capture))],
        4,
        &db,
        &source_id,
    )
    .await;
    let error = catalog
        .get_current_image(GetCurrentImageRequest::new(source_id.clone()))
        .await
        .unwrap_err();
    assert_eq!(error.code(), "unavailable");
    assert!(ids(&catalog, &source_id).await.is_empty());
    drop(catalog);
    let restored = persisted(
        vec![(source(EXTERNAL_CAMERA_SOURCE_ID), Script::new(vec![]))],
        4,
        &db,
        &source_id,
    )
    .await;
    assert!(ids(&restored, &source_id).await.is_empty());

    std::fs::remove_dir_all(dir).ok();
}

#[test]
fn listing_sources_and_a_failed_start_do_not_create_the_history_database() {
    let dir = scratch("cli");
    let listed = bin()
        .current_dir(&dir)
        .args(["--external-camera-index", "4", "list-image-sources"])
        .output()
        .unwrap();
    assert!(
        listed.status.success(),
        "{}",
        String::from_utf8_lossy(&listed.stderr)
    );
    assert!(!dir.join(".a2a-lab-ot2/images.sqlite").exists());

    let started = bin()
        .current_dir(&dir)
        .env("A2ALAB_OPENTRONS_URL", "http://127.0.0.1:9")
        .args(["--external-camera-index", "4"])
        .output()
        .unwrap();
    let stdout = String::from_utf8(started.stdout).unwrap();
    assert!(stdout.contains("history .a2a-lab-ot2/images.sqlite age 7d"));
    assert!(!started.status.success());
    assert!(!dir.join(".a2a-lab-ot2/images.sqlite").exists());
    std::fs::remove_dir_all(dir).ok();
}
