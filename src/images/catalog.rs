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

use crate::page::slice_page;

/// Default number of recent frames retained for each source.
pub const DEFAULT_IMAGE_RETENTION_PER_SOURCE: u64 = 32;

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
/// `retention_per_source` is the maximum number of process-local frames kept
/// for each source. When a newly validated frame would exceed that bound, the
/// catalog removes the oldest other frames of that source until the bound
/// holds. The frame just stored stays readable through `get_image`. Oldest
/// means earliest `captured_at`, then lowest image ID. Eviction does not
/// remove frames from any other source. Evicted IDs then return `not_found`.
/// History lives only in this process.
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

/// Process-local [`ImageProvider`] over independently captured sources.
///
/// Current-image reads call only the named source. A capture error is returned
/// unchanged and stores nothing, so one source failing leaves every other
/// source usable.
#[derive(Clone)]
pub struct LiveImageCatalog {
    sources: Arc<[SourceSlot]>,
    config: ImageCatalogConfig,
    state: Arc<Mutex<CatalogState>>,
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
        })
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
    ImageId::new(format!("img-{id:020}"))
        .map_err(|error| A2aLabError::unavailable(error.to_string()))
}

fn default_retention() -> usize {
    usize::try_from(DEFAULT_IMAGE_RETENTION_PER_SOURCE).unwrap_or(32)
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
