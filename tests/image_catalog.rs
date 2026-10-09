//! Process-local live image catalog.

use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use a2a_lab_dev_kit::{
    A2aLabError, GetCurrentImageRequest, GetImageRequest, ImageId, ImageProvider, ImageSource,
    ImageSourceId, JsonObject, ListImageSourcesRequest, ListImagesRequest, PageRequest,
    SearchImagesRequest, TimeRange, UtcTimestamp,
};
use a2a_lab_ot2::{CapturedFrame, ImageCapture, ImageCatalogConfig, LiveImageCatalog};
use base64::Engine;

fn stamp(value: &str) -> UtcTimestamp {
    UtcTimestamp::parse(value).unwrap()
}

fn page(limit: u32) -> PageRequest {
    PageRequest::new(None, limit).unwrap()
}

fn page_at(cursor: &str, limit: u32) -> PageRequest {
    PageRequest::new(Some(cursor.to_owned()), limit).unwrap()
}

fn source(id: &str, name: &str) -> ImageSource {
    ImageSource {
        id: ImageSourceId::new(id).unwrap(),
        name: name.to_owned(),
        description: format!("{name} camera"),
        asset_id: None,
        semantic_id: None,
    }
}

fn frame(at: &str, caption: Option<&str>, data: &[u8]) -> CapturedFrame {
    CapturedFrame::new(
        stamp(at),
        "image/jpeg",
        4,
        2,
        caption.map(str::to_owned),
        JsonObject::parse(r#"{"view":"deck"}"#).unwrap(),
        data.to_vec(),
    )
}

struct Scripted {
    calls: AtomicUsize,
    results: Mutex<Vec<Result<CapturedFrame, A2aLabError>>>,
}

impl Scripted {
    fn new(results: Vec<Result<CapturedFrame, A2aLabError>>) -> Arc<Self> {
        Arc::new(Self {
            calls: AtomicUsize::new(0),
            results: Mutex::new(results),
        })
    }

    fn calls(&self) -> usize {
        self.calls.load(Ordering::SeqCst)
    }
}

impl ImageCapture for Scripted {
    fn capture(
        &self,
    ) -> Pin<Box<dyn Future<Output = Result<CapturedFrame, A2aLabError>> + Send + '_>> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        let result = {
            let mut results = self.results.lock().expect("script");
            if results.is_empty() {
                Err(A2aLabError::unavailable("scripted capture is exhausted"))
            } else {
                results.remove(0)
            }
        };
        Box::pin(std::future::ready(result))
    }
}

struct Counting {
    label: u8,
    calls: AtomicUsize,
}

impl Counting {
    fn new(label: u8) -> Arc<Self> {
        Arc::new(Self {
            label,
            calls: AtomicUsize::new(0),
        })
    }

    fn calls(&self) -> usize {
        self.calls.load(Ordering::SeqCst)
    }
}

impl ImageCapture for Counting {
    fn capture(
        &self,
    ) -> Pin<Box<dyn Future<Output = Result<CapturedFrame, A2aLabError>> + Send + '_>> {
        Box::pin(async move {
            let n = self.calls.fetch_add(1, Ordering::SeqCst);
            tokio::task::yield_now().await;
            let data = vec![self.label, u8::try_from(n).unwrap_or(255), 9, 9];
            Ok(CapturedFrame::new(
                stamp("2024-01-01T00:00:00Z"),
                "image/jpeg",
                2,
                2,
                Some(format!("frame-{n}")),
                JsonObject::empty(),
                data,
            ))
        })
    }
}

fn catalog(sources: Vec<(ImageSource, Arc<dyn ImageCapture>)>, retention: u64) -> LiveImageCatalog {
    LiveImageCatalog::new(sources, ImageCatalogConfig::new(retention, 1024).unwrap()).unwrap()
}

fn two_sources(
    left: Arc<dyn ImageCapture>,
    right: Arc<dyn ImageCapture>,
    retention: u64,
) -> LiveImageCatalog {
    catalog(
        vec![
            (source("zeta-cam", "Zeta"), left),
            (source("alpha-cam", "Alpha"), right),
        ],
        retention,
    )
}

#[tokio::test]
async fn lists_sources_in_id_order_and_rejects_unknown_ids() {
    let left = Scripted::new(vec![]);
    let right = Scripted::new(vec![]);
    let images = two_sources(left.clone(), right.clone(), 4);

    let first = images
        .list_image_sources(ListImageSourcesRequest::new(page(1)).unwrap())
        .await
        .unwrap();
    assert_eq!(first.items().len(), 1);
    assert_eq!(first.items()[0].id.as_str(), "alpha-cam");
    let cursor = first.next_cursor().unwrap();
    let second = images
        .list_image_sources(ListImageSourcesRequest::new(page_at(cursor, 1)).unwrap())
        .await
        .unwrap();
    assert_eq!(second.items()[0].id.as_str(), "zeta-cam");
    assert!(second.next_cursor().is_none());

    let missing = ImageSourceId::new("missing-cam").unwrap();
    let listed = images
        .list_images(ListImagesRequest::new(missing.clone(), page(10)).unwrap())
        .await
        .unwrap_err();
    assert_eq!(listed.code(), "not_found");
    let searched = images
        .search_images(
            SearchImagesRequest::new(
                Some(missing.clone()),
                None,
                Some("deck".to_owned()),
                page(10),
            )
            .unwrap(),
        )
        .await
        .unwrap_err();
    assert_eq!(searched.code(), "not_found");
    let current = images
        .get_current_image(GetCurrentImageRequest::new(missing))
        .await
        .unwrap_err();
    assert_eq!(current.code(), "not_found");
    assert_eq!(left.calls(), 0);
    assert_eq!(right.calls(), 0);

    let unknown = images
        .get_image(GetImageRequest::new(ImageId::new("img-missing").unwrap()))
        .await
        .unwrap_err();
    assert_eq!(unknown.code(), "not_found");
}

#[tokio::test]
async fn current_capture_is_source_scoped_and_retrievable() {
    let alpha = Scripted::new(vec![Ok(frame(
        "2024-06-01T00:00:00Z",
        Some("alpha deck"),
        b"alpha-bytes",
    ))]);
    let zeta = Scripted::new(vec![Ok(frame(
        "2024-06-01T00:00:01Z",
        Some("zeta deck"),
        b"zeta-bytes",
    ))]);
    let images = two_sources(zeta.clone(), alpha.clone(), 4);
    let alpha_id = ImageSourceId::new("alpha-cam").unwrap();

    let image = images
        .get_current_image(GetCurrentImageRequest::new(alpha_id.clone()))
        .await
        .unwrap();
    assert_eq!(image.data(), b"alpha-bytes");
    assert_eq!(image.descriptor().media_type(), "image/jpeg");
    assert_eq!(image.descriptor().width(), 4);
    assert_eq!(image.descriptor().height(), 2);
    assert_eq!(image.descriptor().source_id(), &alpha_id);
    assert_eq!(alpha.calls(), 1);
    assert_eq!(zeta.calls(), 0);

    let stored = images
        .get_image(GetImageRequest::new(image.descriptor().id().clone()))
        .await
        .unwrap();
    assert_eq!(stored.data(), image.data());
    assert_eq!(stored.descriptor(), image.descriptor());
}

async fn captured_pair() -> (LiveImageCatalog, ImageSourceId, ImageSourceId) {
    let alpha = Scripted::new(vec![
        Ok(frame(
            "2024-06-01T00:00:02Z",
            Some("Later Deck"),
            b"alpha-later",
        )),
        Ok(frame(
            "2024-06-01T00:00:00Z",
            Some("early deck"),
            b"alpha-early",
        )),
        Ok(frame("2024-06-01T00:00:05Z", None, b"alpha-plain")),
    ]);
    let zeta = Scripted::new(vec![Ok(frame(
        "2024-06-01T00:00:01Z",
        Some("zeta deck"),
        b"zeta-bytes",
    ))]);
    let images = two_sources(zeta, alpha, 8);
    let alpha_id = ImageSourceId::new("alpha-cam").unwrap();
    let zeta_id = ImageSourceId::new("zeta-cam").unwrap();
    for source_id in [&alpha_id, &alpha_id, &alpha_id, &zeta_id] {
        images
            .get_current_image(GetCurrentImageRequest::new(source_id.clone()))
            .await
            .unwrap();
    }
    (images, alpha_id, zeta_id)
}

#[tokio::test]
async fn lists_and_searches_metadata_without_pixel_bytes() {
    let (images, alpha_id, _) = captured_pair().await;

    let listed = images
        .list_images(ListImagesRequest::new(alpha_id.clone(), page(10)).unwrap())
        .await
        .unwrap();
    let captions: Vec<_> = listed
        .items()
        .iter()
        .map(|item| item.caption().map(str::to_owned))
        .collect();
    assert_eq!(
        captions,
        vec![
            Some("early deck".to_owned()),
            Some("Later Deck".to_owned()),
            None,
        ]
    );
    let encoded = base64::engine::general_purpose::STANDARD.encode(b"alpha-early");
    let json = serde_json::to_string(listed.items()).unwrap();
    assert!(!json.contains(&encoded));
    assert!(!json.contains("alpha-early"));
    assert!(json.contains("early deck"));

    let page_one = images
        .list_images(ListImagesRequest::new(alpha_id.clone(), page(2)).unwrap())
        .await
        .unwrap();
    assert_eq!(page_one.items().len(), 2);
    let page_two = images
        .list_images(
            ListImagesRequest::new(
                alpha_id.clone(),
                page_at(page_one.next_cursor().unwrap(), 2),
            )
            .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(page_two.items().len(), 1);
    assert!(page_two.next_cursor().is_none());
}

#[tokio::test]
async fn searches_honor_source_range_text_and_pages() {
    let (images, alpha_id, zeta_id) = captured_pair().await;
    let start = stamp("2024-06-01T00:00:00Z");
    let end = stamp("2024-06-01T00:00:02Z");
    let ranged = images
        .search_images(
            SearchImagesRequest::new(
                None,
                Some(TimeRange::new(start, end).unwrap()),
                None,
                page(10),
            )
            .unwrap(),
        )
        .await
        .unwrap();
    let ranged_ids: Vec<_> = ranged
        .items()
        .iter()
        .map(|item| item.source_id().as_str())
        .collect();
    assert_eq!(ranged_ids, vec!["alpha-cam", "zeta-cam"]);
    assert!(
        ranged
            .items()
            .iter()
            .all(|item| item.captured_at() >= start && item.captured_at() < end)
    );

    let at_end = images
        .search_images(
            SearchImagesRequest::new(
                Some(alpha_id.clone()),
                Some(TimeRange::new(end, stamp("2024-06-01T00:00:06Z")).unwrap()),
                None,
                page(10),
            )
            .unwrap(),
        )
        .await
        .unwrap();
    let bounded: Vec<_> = at_end
        .items()
        .iter()
        .map(|item| item.caption().map(str::to_owned))
        .collect();
    assert_eq!(bounded, vec![Some("Later Deck".to_owned()), None]);

    let text = images
        .search_images(
            SearchImagesRequest::new(None, None, Some("DECK".to_owned()), page(1)).unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(text.items().len(), 1);
    assert_eq!(text.items()[0].caption(), Some("early deck"));
    let text_rest = images
        .search_images(
            SearchImagesRequest::new(
                None,
                None,
                Some("DECK".to_owned()),
                page_at(text.next_cursor().unwrap(), 1),
            )
            .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(text_rest.items().len(), 1);
    assert!(text_rest.next_cursor().is_some());

    let combined = images
        .search_images(
            SearchImagesRequest::new(
                Some(zeta_id),
                Some(TimeRange::new(start, stamp("2024-06-01T00:00:03Z")).unwrap()),
                Some("zeta".to_owned()),
                page(10),
            )
            .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(combined.items().len(), 1);
    assert_eq!(combined.items()[0].source_id().as_str(), "zeta-cam");
    let combined_json = serde_json::to_string(combined.items()).unwrap();
    assert!(!combined_json.contains("zeta-bytes"));
}

#[tokio::test]
async fn repeated_captures_stay_ordered_when_timestamps_tie() {
    let alpha = Scripted::new(vec![
        Ok(frame("2024-06-01T00:00:01Z", Some("second"), b"two")),
        Ok(frame("2024-06-01T00:00:01Z", Some("first"), b"one")),
    ]);
    let zeta = Scripted::new(vec![]);
    let images = two_sources(zeta, alpha, 4);
    let alpha_id = ImageSourceId::new("alpha-cam").unwrap();
    let first = images
        .get_current_image(GetCurrentImageRequest::new(alpha_id.clone()))
        .await
        .unwrap();
    let second = images
        .get_current_image(GetCurrentImageRequest::new(alpha_id.clone()))
        .await
        .unwrap();
    assert_ne!(first.descriptor().id(), second.descriptor().id());
    let listed = images
        .list_images(ListImagesRequest::new(alpha_id, page(10)).unwrap())
        .await
        .unwrap();
    assert_eq!(listed.items()[0].id(), first.descriptor().id());
    assert_eq!(listed.items()[1].id(), second.descriptor().id());
}

#[tokio::test]
async fn retention_evicts_only_the_oldest_frames_of_one_source() {
    let alpha = Scripted::new(vec![
        Ok(frame("2024-06-01T00:00:03Z", Some("a3"), b"a3")),
        Ok(frame("2024-06-01T00:00:01Z", Some("a1"), b"a1")),
        Ok(frame("2024-06-01T00:00:02Z", Some("a2"), b"a2")),
        Ok(frame("2024-06-01T00:00:02Z", Some("a2b"), b"a2b")),
        Ok(frame("2024-05-01T00:00:00Z", Some("a0"), b"a0")),
    ]);
    let zeta = Scripted::new(vec![
        Ok(frame("2024-06-01T00:00:00Z", Some("z1"), b"z1")),
        Ok(frame("2024-06-01T00:00:04Z", Some("z2"), b"z2")),
    ]);
    let images = two_sources(zeta, alpha, 2);
    let alpha_id = ImageSourceId::new("alpha-cam").unwrap();
    let zeta_id = ImageSourceId::new("zeta-cam").unwrap();
    let mut alpha_ids = Vec::new();
    for _ in 0..4 {
        alpha_ids.push(
            images
                .get_current_image(GetCurrentImageRequest::new(alpha_id.clone()))
                .await
                .unwrap()
                .descriptor()
                .id()
                .clone(),
        );
    }
    let mut zeta_ids = Vec::new();
    for _ in 0..2 {
        zeta_ids.push(
            images
                .get_current_image(GetCurrentImageRequest::new(zeta_id.clone()))
                .await
                .unwrap()
                .descriptor()
                .id()
                .clone(),
        );
    }

    let kept = images
        .list_images(ListImagesRequest::new(alpha_id.clone(), page(10)).unwrap())
        .await
        .unwrap();
    let kept_captions: Vec<_> = kept
        .items()
        .iter()
        .map(|item| item.caption().unwrap())
        .collect();
    assert_eq!(kept_captions, vec!["a2b", "a3"]);
    assert_eq!(
        images
            .get_image(GetImageRequest::new(alpha_ids[1].clone()))
            .await
            .unwrap_err()
            .code(),
        "not_found"
    );
    assert_eq!(
        images
            .get_image(GetImageRequest::new(alpha_ids[2].clone()))
            .await
            .unwrap_err()
            .code(),
        "not_found"
    );
    for id in &zeta_ids {
        let image = images
            .get_image(GetImageRequest::new(id.clone()))
            .await
            .unwrap();
        assert!(!image.data().is_empty());
    }
    let older = images
        .get_current_image(GetCurrentImageRequest::new(alpha_id.clone()))
        .await
        .unwrap();
    assert_eq!(older.data(), b"a0");
    assert_eq!(
        images
            .get_image(GetImageRequest::new(older.descriptor().id().clone()))
            .await
            .unwrap()
            .data(),
        b"a0"
    );
    let zeta_listed = images
        .list_images(ListImagesRequest::new(zeta_id, page(10)).unwrap())
        .await
        .unwrap();
    assert_eq!(zeta_listed.items().len(), 2);
}

#[tokio::test]
async fn capture_failure_is_isolated_and_stores_nothing() {
    let alpha = Scripted::new(vec![
        Ok(frame("2024-06-01T00:00:00Z", Some("kept"), b"kept")),
        Err(A2aLabError::unavailable("alpha camera offline")),
        Ok(frame("2024-06-01T00:00:02Z", Some("after"), b"after")),
    ]);
    let zeta = Scripted::new(vec![Ok(frame(
        "2024-06-01T00:00:01Z",
        Some("zeta"),
        b"zeta-ok",
    ))]);
    let images = two_sources(zeta.clone(), alpha.clone(), 4);
    let alpha_id = ImageSourceId::new("alpha-cam").unwrap();
    let zeta_id = ImageSourceId::new("zeta-cam").unwrap();

    images
        .get_current_image(GetCurrentImageRequest::new(alpha_id.clone()))
        .await
        .unwrap();
    let failed = images
        .get_current_image(GetCurrentImageRequest::new(alpha_id.clone()))
        .await
        .unwrap_err();
    assert_eq!(failed.code(), "unavailable");
    assert!(failed.to_string().contains("alpha camera offline"));
    assert_eq!(zeta.calls(), 0);

    let zeta_image = images
        .get_current_image(GetCurrentImageRequest::new(zeta_id.clone()))
        .await
        .unwrap();
    assert_eq!(zeta_image.data(), b"zeta-ok");
    images
        .get_current_image(GetCurrentImageRequest::new(alpha_id.clone()))
        .await
        .unwrap();

    let listed = images
        .list_images(ListImagesRequest::new(alpha_id, page(10)).unwrap())
        .await
        .unwrap();
    assert_eq!(listed.items().len(), 2);
    assert_eq!(zeta.calls(), 1);
}

#[tokio::test]
async fn invalid_frames_are_not_retained() {
    let alpha = Scripted::new(vec![Ok(CapturedFrame::new(
        stamp("2024-06-01T00:00:00Z"),
        "image/jpeg",
        1,
        1,
        None,
        JsonObject::empty(),
        Vec::new(),
    ))]);
    let zeta = Scripted::new(vec![Ok(frame(
        "2024-06-01T00:00:01Z",
        Some("zeta"),
        b"zeta-ok",
    ))]);
    let images = two_sources(zeta, alpha, 4);
    let alpha_id = ImageSourceId::new("alpha-cam").unwrap();
    let failed = images
        .get_current_image(GetCurrentImageRequest::new(alpha_id.clone()))
        .await
        .unwrap_err();
    assert_eq!(failed.code(), "invalid");
    let listed = images
        .list_images(ListImagesRequest::new(alpha_id, page(10)).unwrap())
        .await
        .unwrap();
    assert!(listed.items().is_empty());
}

#[tokio::test]
async fn concurrent_captures_keep_unique_ids_and_bytes() {
    let alpha = Counting::new(1);
    let zeta = Counting::new(2);
    let images = two_sources(zeta.clone(), alpha.clone(), 32);
    let alpha_id = ImageSourceId::new("alpha-cam").unwrap();
    let zeta_id = ImageSourceId::new("zeta-cam").unwrap();
    let mut tasks = Vec::new();
    for _ in 0..12 {
        let images = images.clone();
        let alpha_id = alpha_id.clone();
        let zeta_id = zeta_id.clone();
        tasks.push(tokio::spawn(async move {
            let left = images
                .get_current_image(GetCurrentImageRequest::new(alpha_id))
                .await
                .unwrap();
            let right = images
                .get_current_image(GetCurrentImageRequest::new(zeta_id))
                .await
                .unwrap();
            (left, right)
        }));
    }
    let mut seen = Vec::new();
    for task in tasks {
        let (left, right) = task.await.unwrap();
        seen.push(left);
        seen.push(right);
    }
    let mut ids: Vec<_> = seen
        .iter()
        .map(|image| image.descriptor().id().as_str().to_owned())
        .collect();
    ids.sort();
    ids.dedup();
    assert_eq!(ids.len(), 24);
    for image in &seen {
        let stored = images
            .get_image(GetImageRequest::new(image.descriptor().id().clone()))
            .await
            .unwrap();
        assert_eq!(stored.data(), image.data());
        assert_eq!(stored.descriptor(), image.descriptor());
        assert_eq!(stored.data()[0], image.data()[0]);
    }
    assert_eq!(alpha.calls(), 12);
    assert_eq!(zeta.calls(), 12);
    let alpha_listed = images
        .list_images(ListImagesRequest::new(alpha_id, page(50)).unwrap())
        .await
        .unwrap();
    for item in alpha_listed.items() {
        let stored = images
            .get_image(GetImageRequest::new(item.id().clone()))
            .await
            .unwrap();
        assert_eq!(stored.data()[0], 1);
    }
}

#[test]
fn retention_configuration_rejects_zero_and_duplicate_sources() {
    assert_eq!(
        ImageCatalogConfig::new(0, 1024).unwrap_err().code(),
        "invalid"
    );
    assert_eq!(
        ImageCatalogConfig::with_defaults().retention_per_source(),
        usize::try_from(a2a_lab_ot2::DEFAULT_IMAGE_RETENTION_PER_SOURCE).unwrap()
    );
    let capture = Scripted::new(vec![]);
    match LiveImageCatalog::new(
        vec![
            (source("alpha-cam", "Alpha"), capture.clone()),
            (source("alpha-cam", "Again"), capture),
        ],
        ImageCatalogConfig::with_defaults(),
    ) {
        Ok(_) => panic!("duplicate source was accepted"),
        Err(error) => assert_eq!(error.code(), "invalid"),
    }
}
