//! Bounded multi-source catalog of newly captured frames.

use std::cmp::Ordering;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use a2a_lab_dev_kit::{
    A2aLabError, GetCurrentImageRequest, GetImageRequest, Image, ImageDescriptor, ImageId,
    ImageProvider, ImageSource, ImageSourceId, ImageTransportConfig, JsonObject,
    ListImageSourcesRequest, ListImagesRequest, Page, SearchImagesRequest, UtcTimestamp,
};
use tokio::sync::Mutex;

use crate::images::history::{IMAGE_HISTORY_CLEANUP_INTERVAL, ImageHistory};
use crate::page::slice_page;

/// Stops hourly image-history cleanup when dropped.
#[must_use = "dropping this stops image history cleanup"]
pub struct ImageHistoryMaintenance {
    task: tokio::task::JoinHandle<()>,
}

impl Drop for ImageHistoryMaintenance {
    fn drop(&mut self) {
        self.task.abort();
    }
}

/// Default number of recent frames retained for each source.
///
/// Age cleanup is 30 days. This count is high enough that a month of stills
/// is not dropped by the per-source cap.
pub const DEFAULT_IMAGE_RETENTION_PER_SOURCE: u64 = 100_000;

/// One newly captured frame before the catalog assigns an image ID.
///
/// Pixel bytes stay on this value until [`LiveImageCatalog`] validates and
/// stores them. List and search results never return this type.
#[derive(Debug, Clone, PartialEq)]
pub struct CapturedFrame {
    captured_at: UtcTimestamp,
    media_type: String,
    width: u32,
    height: u32,
    caption: Option<String>,
    attributes: JsonObject,
    data: Vec<u8>,
}

impl CapturedFrame {
    /// Builds a frame from capture output.
    ///
    /// The catalog validates the media type, dimensions, and payload before
    /// retention. A frame that fails validation is not stored.
    #[must_use]
    pub fn new(
        captured_at: UtcTimestamp,
        media_type: impl Into<String>,
        width: u32,
        height: u32,
        caption: Option<String>,
        attributes: JsonObject,
        data: Vec<u8>,
    ) -> Self {
        Self {
            captured_at,
            media_type: media_type.into(),
            width,
            height,
            caption,
            attributes,
            data,
        }
    }

    /// When the frame was captured.
    #[must_use]
    pub const fn captured_at(&self) -> UtcTimestamp {
        self.captured_at
    }

    /// `image/*` media type declared by the capture backend.
    #[must_use]
    pub fn media_type(&self) -> &str {
        &self.media_type
    }

    /// Width in pixels declared by the capture backend.
    #[must_use]
    pub const fn width(&self) -> u32 {
        self.width
    }

    /// Height in pixels declared by the capture backend.
    #[must_use]
    pub const fn height(&self) -> u32 {
        self.height
    }

    /// Optional caption used by text search.
    #[must_use]
    pub fn caption(&self) -> Option<&str> {
        self.caption.as_deref()
    }

    /// Structured attributes. These do not carry pixel bytes.
    #[must_use]
    pub const fn attributes(&self) -> &JsonObject {
        &self.attributes
    }

    /// Decoded payload bytes.
    #[must_use]
    pub fn data(&self) -> &[u8] {
        &self.data
    }
}

/// Capture backend for one configured image source.
///
/// Implementations own device or protocol access. The catalog calls
/// [`Self::capture`] only for the requested source.
pub trait ImageCapture: Send + Sync {
    /// Captures one current frame, or returns an error without a partial frame.
    fn capture(
        &self,
    ) -> Pin<Box<dyn Future<Output = Result<CapturedFrame, A2aLabError>> + Send + '_>>;
}

/// Retention and payload limits for a [`LiveImageCatalog`].
///
/// `retention_per_source` is the maximum number of frames kept for each source.
/// When a newly validated frame would exceed that bound, the
/// catalog removes the oldest other frames of that source until the bound
/// holds. The frame just stored stays readable through `get_image`. Oldest
/// means earliest `captured_at`, then lowest image ID. Eviction does not
/// remove frames from any other source. Evicted IDs then return `not_found`.
/// History lives only in this process unless persistence is configured.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ImageCatalogConfig {
    retention_per_source: usize,
    transport: ImageTransportConfig,
}

impl ImageCatalogConfig {
    /// Rejects a zero retention bound and an invalid decoded-byte maximum.
    pub fn new(retention_per_source: u64, max_image_bytes: u64) -> Result<Self, A2aLabError> {
        if retention_per_source == 0 {
            return Err(A2aLabError::invalid(
                "retention_per_source",
                "must be greater than zero",
            ));
        }
        let retention_per_source = usize::try_from(retention_per_source)
            .map_err(|_| A2aLabError::invalid("retention_per_source", "is overflow-prone"))?;
        Ok(Self {
            retention_per_source,
            transport: ImageTransportConfig::new(max_image_bytes)?,
        })
    }

    /// Uses [`DEFAULT_IMAGE_RETENTION_PER_SOURCE`] and the devkit payload maximum.
    #[must_use]
    pub fn with_defaults() -> Self {
        Self {
            retention_per_source: default_retention(),
            transport: ImageTransportConfig::default(),
        }
    }

    /// Frames kept for each source.
    #[must_use]
    pub const fn retention_per_source(self) -> usize {
        self.retention_per_source
    }

    /// Decoded-byte limit applied before a frame is retained.
    #[must_use]
    pub const fn transport(self) -> ImageTransportConfig {
        self.transport
    }
}

struct SourceSlot {
    source: ImageSource,
    capture: Arc<dyn ImageCapture>,
}

struct CatalogState {
    images: Vec<Image>,
    next_id: u64,
}

/// [`ImageProvider`] over independently captured sources.
///
/// Current-image reads call only the named source. A capture error is returned
/// unchanged and stores nothing, so one source failing leaves every other
/// source usable.
#[derive(Clone)]
pub struct LiveImageCatalog {
    sources: Arc<[SourceSlot]>,
    config: ImageCatalogConfig,
    state: Arc<Mutex<CatalogState>>,
    history: Option<Arc<ImageHistory>>,
    persisted_sources: Arc<[ImageSourceId]>,
}

impl LiveImageCatalog {
    /// Creates a catalog for `sources` when their IDs are unique.
    pub fn new(
        sources: Vec<(ImageSource, Arc<dyn ImageCapture>)>,
        config: ImageCatalogConfig,
    ) -> Result<Self, A2aLabError> {
        let mut ids: Vec<&str> = sources
            .iter()
            .map(|(source, _)| source.id.as_str())
            .collect();
        ids.sort_unstable();
        if ids.windows(2).any(|pair| pair[0] == pair[1]) {
            return Err(A2aLabError::invalid("id", "image source already exists"));
        }
        let sources = sources
            .into_iter()
            .map(|(source, capture)| SourceSlot { source, capture })
            .collect();
        Ok(Self {
            sources,
            config,
            state: Arc::new(Mutex::new(CatalogState {
                images: Vec::new(),
                next_id: 0,
            })),
            history: None,
            persisted_sources: Arc::from([]),
        })
    }

    /// Loads `source_id` from `path` and writes later captures of that source there.
    ///
    /// Frames older than thirty days are deleted during the load. This does not capture a frame.
    pub async fn with_persisted_source(
        self,
        source_id: &ImageSourceId,
        path: &std::path::Path,
    ) -> Result<Self, A2aLabError> {
        self.with_persisted_sources(std::slice::from_ref(source_id), path)
            .await
    }

    /// Loads `source_ids` from `path` and writes later captures of those sources there.
    ///
    /// Frames older than thirty days are deleted during the load. This does not capture a frame.
    pub async fn with_persisted_sources(
        mut self,
        source_ids: &[ImageSourceId],
        path: &std::path::Path,
    ) -> Result<Self, A2aLabError> {
        if self.history.is_some() {
            return Err(A2aLabError::invalid("image history", "is already open"));
        }
        if source_ids.is_empty() {
            return Err(A2aLabError::invalid(
                "image history",
                "requires at least one source",
            ));
        }
        for (index, source_id) in source_ids.iter().enumerate() {
            self.require_source(source_id)?;
            if source_ids[..index].iter().any(|other| other == source_id) {
                return Err(A2aLabError::invalid(
                    "image history",
                    "source ids must be unique",
                ));
            }
        }
        let history = ImageHistory::open(path, source_ids).await?;
        let now = crate::time::now()?;
        let (images, next_sequence) = history
            .restore(self.config.retention_per_source, self.config.transport, now)
            .await?;
        {
            let mut state = self.state.lock().await;
            state.next_id = state.next_id.max(next_sequence);
            state.images.extend(images);
        }
        self.history = Some(Arc::new(history));
        self.persisted_sources = Arc::from(source_ids);
        Ok(self)
    }

    /// Deletes persisted frames older than thirty days or beyond the retention count.
    pub async fn expire_history_at(&self, now: UtcTimestamp) -> Result<(), A2aLabError> {
        let Some(history) = &self.history else {
            return Ok(());
        };
        let mut state = self.state.lock().await;
        let removed = history
            .expire(self.config.retention_per_source, now)
            .await?;
        forget(&mut state.images, &removed);
        Ok(())
    }

    /// Runs [`Self::expire_history_at`] once per hour until the returned guard is dropped.
    #[must_use]
    pub fn maintain_history(&self) -> Option<ImageHistoryMaintenance> {
        self.history.as_ref()?;
        let catalog = self.clone();
        let task = tokio::spawn(async move {
            let mut interval = tokio::time::interval(IMAGE_HISTORY_CLEANUP_INTERVAL);
            interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
            interval.tick().await;
            loop {
                interval.tick().await;
                if let Err(error) = cleanup(&catalog).await {
                    eprintln!("image history: {error}");
                }
            }
        });
        Some(ImageHistoryMaintenance { task })
    }

    #[cfg(test)]
    pub(crate) fn fail_next_history_write(&self) {
        self.history.as_ref().expect("history").fail_next_write();
    }

    fn require_source(&self, source_id: &ImageSourceId) -> Result<(), A2aLabError> {
        if self.sources.iter().any(|slot| &slot.source.id == source_id) {
            Ok(())
        } else {
            Err(A2aLabError::not_found(
                "image source",
                source_id.to_string(),
            ))
        }
    }

    fn capture_for(&self, source_id: &ImageSourceId) -> Result<Arc<dyn ImageCapture>, A2aLabError> {
        self.sources
            .iter()
            .find(|slot| &slot.source.id == source_id)
            .map(|slot| Arc::clone(&slot.capture))
            .ok_or_else(|| A2aLabError::not_found("image source", source_id.to_string()))
    }

    async fn store(
        &self,
        source_id: ImageSourceId,
        frame: CapturedFrame,
    ) -> Result<Image, A2aLabError> {
        if self
            .persisted_sources
            .iter()
            .any(|persisted| persisted == &source_id)
        {
            self.store_persisted(source_id, frame).await
        } else {
            self.store_memory(source_id, frame).await
        }
    }

    async fn store_memory(
        &self,
        source_id: ImageSourceId,
        frame: CapturedFrame,
    ) -> Result<Image, A2aLabError> {
        let mut state = self.state.lock().await;
        let id = next_image_id(&mut state.next_id)?;
        let image = image_from_frame(id, &source_id, frame, self.config.transport)?;
        state.images.push(image.clone());
        evict(
            &mut state.images,
            &source_id,
            image.descriptor().id(),
            self.config.retention_per_source,
        );
        Ok(image)
    }

    async fn store_persisted(
        &self,
        source_id: ImageSourceId,
        frame: CapturedFrame,
    ) -> Result<Image, A2aLabError> {
        let history = self
            .history
            .as_ref()
            .ok_or_else(|| A2aLabError::unavailable("image history is not open"))?;
        let mut state = self.state.lock().await;
        let sequence = state
            .next_id
            .checked_add(1)
            .ok_or_else(|| A2aLabError::unavailable("image id space is exhausted"))?;
        let image = image_from_frame(
            image_id(sequence)?,
            &source_id,
            frame,
            self.config.transport,
        )?;
        let removed = history
            .record(
                &image,
                self.config.retention_per_source,
                crate::time::now()?,
            )
            .await?;
        forget(&mut state.images, &removed);
        state.next_id = sequence;
        state.images.push(image.clone());
        Ok(image)
    }
}

impl ImageProvider for LiveImageCatalog {
    #[allow(clippy::unused_async_trait_impl)]
    async fn list_image_sources(
        &self,
        request: ListImageSourcesRequest,
    ) -> Result<Page<ImageSource>, A2aLabError> {
        let mut sources: Vec<_> = self
            .sources
            .iter()
            .map(|slot| slot.source.clone())
            .collect();
        sources.sort_by(|left, right| left.id.as_str().cmp(right.id.as_str()));
        slice_page(&sources, request.page())
    }

    async fn list_images(
        &self,
        request: ListImagesRequest,
    ) -> Result<Page<ImageDescriptor>, A2aLabError> {
        self.require_source(request.source_id())?;
        let descriptors = self
            .descriptors_where(|descriptor| descriptor.source_id() == request.source_id())
            .await;
        slice_page(&descriptors, request.page())
    }

    async fn search_images(
        &self,
        request: SearchImagesRequest,
    ) -> Result<Page<ImageDescriptor>, A2aLabError> {
        request.check()?;
        if let Some(source_id) = request.source_id() {
            self.require_source(source_id)?;
        }
        let descriptors = self
            .descriptors_where(|descriptor| matches_query(descriptor, &request))
            .await;
        slice_page(&descriptors, request.page())
    }

    async fn get_image(&self, request: GetImageRequest) -> Result<Image, A2aLabError> {
        let state = self.state.lock().await;
        state
            .images
            .iter()
            .find(|image| image.descriptor().id() == request.id())
            .cloned()
            .ok_or_else(|| A2aLabError::not_found("image", request.id().to_string()))
    }

    async fn get_current_image(
        &self,
        request: GetCurrentImageRequest,
    ) -> Result<Image, A2aLabError> {
        let capture = self.capture_for(request.source_id())?;
        let frame = capture.capture().await?;
        self.store(request.source_id().clone(), frame).await
    }
}

impl LiveImageCatalog {
    async fn descriptors_where(
        &self,
        include: impl Fn(&ImageDescriptor) -> bool,
    ) -> Vec<ImageDescriptor> {
        let state = self.state.lock().await;
        let mut descriptors: Vec<_> = state
            .images
            .iter()
            .filter(|image| include(image.descriptor()))
            .map(|image| image.descriptor().clone())
            .collect();
        descriptors.sort_by(descriptor_order);
        descriptors
    }
}

fn image_from_frame(
    id: ImageId,
    source_id: &ImageSourceId,
    frame: CapturedFrame,
    transport: ImageTransportConfig,
) -> Result<Image, A2aLabError> {
    let descriptor = ImageDescriptor::new(
        id,
        source_id.clone(),
        frame.captured_at,
        frame.media_type,
        frame.width,
        frame.height,
        frame.caption,
        frame.attributes,
    )?;
    Image::with_transport(descriptor, frame.data, transport)
}

fn next_image_id(next: &mut u64) -> Result<ImageId, A2aLabError> {
    let id = next
        .checked_add(1)
        .ok_or_else(|| A2aLabError::unavailable("image id space is exhausted"))?;
    *next = id;
    image_id(id)
}

fn image_id(sequence: u64) -> Result<ImageId, A2aLabError> {
    ImageId::new(format!("img-{sequence:020}"))
        .map_err(|error| A2aLabError::unavailable(error.to_string()))
}

fn forget(images: &mut Vec<Image>, removed: &[String]) {
    images.retain(|image| {
        !removed
            .iter()
            .any(|id| id == image.descriptor().id().as_str())
    });
}

async fn cleanup(catalog: &LiveImageCatalog) -> Result<(), A2aLabError> {
    catalog.expire_history_at(crate::time::now()?).await
}

fn default_retention() -> usize {
    usize::try_from(DEFAULT_IMAGE_RETENTION_PER_SOURCE).unwrap_or(100_000)
}

fn evict(images: &mut Vec<Image>, source_id: &ImageSourceId, keep: &ImageId, retention: usize) {
    loop {
        let mut owned: Vec<usize> = images
            .iter()
            .enumerate()
            .filter(|(_, image)| image.descriptor().source_id() == source_id)
            .map(|(index, _)| index)
            .collect();
        if owned.len() <= retention {
            return;
        }
        owned.sort_by(|&left, &right| {
            descriptor_order(images[left].descriptor(), images[right].descriptor())
        });
        let Some(index) = owned
            .into_iter()
            .find(|&index| images[index].descriptor().id() != keep)
        else {
            return;
        };
        images.remove(index);
    }
}

fn matches_query(descriptor: &ImageDescriptor, request: &SearchImagesRequest) -> bool {
    if request
        .source_id()
        .is_some_and(|source_id| source_id != descriptor.source_id())
    {
        return false;
    }
    if request
        .range()
        .is_some_and(|range| !range.contains(descriptor.captured_at()))
    {
        return false;
    }
    request
        .text()
        .is_none_or(|text| caption_matches(descriptor.caption(), text))
}

fn caption_matches(caption: Option<&str>, query: &str) -> bool {
    caption.is_some_and(|caption| {
        let caption = caption.to_lowercase();
        let query = query.to_lowercase();
        caption.contains(&query)
    })
}

fn descriptor_order(left: &ImageDescriptor, right: &ImageDescriptor) -> Ordering {
    left.captured_at()
        .cmp(&right.captured_at())
        .then_with(|| left.id().as_str().cmp(right.id().as_str()))
}

#[cfg(test)]
mod history_tests {
    use std::sync::atomic::{AtomicUsize, Ordering as AtomicOrdering};

    use a2a_lab_dev_kit::{
        GetCurrentImageRequest, GetImageRequest, ListImagesRequest, PageRequest,
    };

    use super::*;

    struct Scripted {
        calls: AtomicUsize,
    }

    impl Scripted {
        fn new() -> Arc<Self> {
            Arc::new(Self {
                calls: AtomicUsize::new(0),
            })
        }
    }

    impl ImageCapture for Scripted {
        fn capture(
            &self,
        ) -> Pin<Box<dyn Future<Output = Result<CapturedFrame, A2aLabError>> + Send + '_>> {
            let call = self.calls.fetch_add(1, AtomicOrdering::SeqCst);
            Box::pin(std::future::ready(Ok(CapturedFrame::new(
                crate::time::now().unwrap(),
                "image/jpeg",
                2,
                2,
                Some(format!("frame-{call}")),
                JsonObject::empty(),
                vec![9, u8::try_from(call).unwrap_or(255)],
            ))))
        }
    }

    fn source() -> ImageSource {
        ImageSource {
            id: ImageSourceId::new("external-camera").unwrap(),
            name: "External camera".to_owned(),
            description: "Bench".to_owned(),
            asset_id: None,
            semantic_id: None,
        }
    }

    fn page() -> PageRequest {
        PageRequest::new(None, 10).unwrap()
    }

    #[tokio::test]
    async fn failed_write_does_not_publish_a_frame() {
        let dir =
            std::env::temp_dir().join(format!("a2a-lab-ot2-history-write-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let source_id = source().id;
        let capture = Scripted::new();
        let catalog = LiveImageCatalog::new(
            vec![(source(), Arc::clone(&capture) as Arc<dyn ImageCapture>)],
            ImageCatalogConfig::new(4, 1024).unwrap(),
        )
        .unwrap()
        .with_persisted_source(&source_id, &dir.join("images.sqlite"))
        .await
        .unwrap();
        catalog.fail_next_history_write();
        let error = catalog
            .get_current_image(GetCurrentImageRequest::new(source_id.clone()))
            .await
            .unwrap_err();
        assert_eq!(error.code(), "unavailable");
        assert!(error.to_string().contains("image history write failed"));
        let listed = catalog
            .list_images(ListImagesRequest::new(source_id.clone(), page()).unwrap())
            .await
            .unwrap();
        assert!(listed.items().is_empty());

        let image = catalog
            .get_current_image(GetCurrentImageRequest::new(source_id.clone()))
            .await
            .unwrap();
        let loaded = catalog
            .get_image(GetImageRequest::new(image.descriptor().id().clone()))
            .await
            .unwrap();
        assert_eq!(loaded.data(), image.data());
        assert_eq!(image.descriptor().id().as_str(), "img-00000000000000000001");
        std::fs::remove_dir_all(dir).ok();
    }
}
