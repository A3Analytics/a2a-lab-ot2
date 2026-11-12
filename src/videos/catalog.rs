//! Memory video catalog plus cancellable capture pumps per camera.

use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, Instant};

use a2a_lab_dev_kit::videos::{CaptureRunnerPhase, ColorCamera};
use a2a_lab_dev_kit::{
    A2aLabError, FrameDecisionLedgerProvider, MJPEG_MEDIA_TYPE, MemoryVideos, VideoSource,
    VideoSourceId,
};
use tokio::sync::mpsc::error::TrySendError;
use tokio::sync::{mpsc, watch};

use crate::camera::remember_live_still;
use crate::images::jpeg_dimensions;
use crate::lab_config::ResolvedSemanticIndexing;
use crate::videos::capture_open::open_native_session_capture;
use crate::videos::dvr::{
    DvrFrameDecisionLedger, IndexedVideoDvr, VideoDvrAnnotationQueue, VideoDvrCatalog,
    VideoDvrDecisionEvaluationQueue, VideoDvrMaintenance,
};
use crate::videos::dvr_cadence::JpegCadenceSelector;
use crate::videos::dyn_camera::DynColorCamera;
use crate::videos::external::{LiveJpegSource, frame_budget};
use crate::videos::live_encode::LiveColorEncodePump;
use crate::videos::native_drain::NativeCaptureDrain;
use crate::videos::nokhwa_camera::NokhwaColorCamera;
use crate::videos::raw_jpeg::{captured_frame_jpeg, normalize_mjpeg_frame};
use crate::videos::semantic_indexing::{
    SemanticServeBundle, SemanticServeInjections, prepare_semantic_serve_bundle,
};
use crate::videos::session_capture::SessionColorCapture;
use crate::videos::source::{LiveVideoDelivery, LiveVideoSourceDescriptor, validate_live_sources};

/// Live video catalog and the capture threads that fill it.
pub struct LiveVideoCatalog {
    inner: VideoDvrCatalog,
    #[allow(dead_code)] // keeps periodic cleanup alive until catalog drop
    maintenance: Option<VideoDvrMaintenance>,
    #[allow(dead_code)] // keeps background reconcile alive until catalog drop
    annotation: VideoDvrAnnotationQueue,
    #[allow(dead_code)]
    decision_evaluation: VideoDvrDecisionEvaluationQueue,
    semantic: SemanticServeBundle,
    stop: watch::Sender<bool>,
}

impl LiveVideoCatalog {
    /// Catalog used by A2A/MCP.
    #[must_use]
    pub fn provider(&self) -> VideoDvrCatalog {
        self.inner.clone()
    }

    #[must_use]
    pub fn dvr(&self) -> &IndexedVideoDvr {
        self.inner.dvr()
    }

    /// SQLite-backed Jev decision ledger when semantic indexing is enabled.
    pub fn frame_decision_ledger(
        &self,
    ) -> Result<Option<Arc<dyn FrameDecisionLedgerProvider>>, A2aLabError> {
        if self.semantic.search.is_none() {
            return Ok(None);
        }
        DvrFrameDecisionLedger::pilot_authorized_producer(Arc::new(self.inner.dvr().clone()))
            .map(|ledger| Some(ledger as Arc<dyn FrameDecisionLedgerProvider>))
    }

    #[must_use]
    pub fn semantic_bundle(&self) -> &SemanticServeBundle {
        &self.semantic
    }

    /// Stops capture threads and releases native sessions on drop.
    pub fn shutdown(&self) {
        let _ = self.stop.send(true);
    }
}

impl Drop for LiveVideoCatalog {
    fn drop(&mut self) {
        self.shutdown();
    }
}

/// Injectable producers for hardware-free tests.
#[derive(Default)]
pub struct LiveVideoInjections {
    /// Injected MJPEG pumps keyed by configured source id.
    pub jpeg_sources: HashMap<String, Box<dyn LiveJpegSource>>,
    /// Injected [`ColorCamera`] backends keyed by stable source id.
    pub cameras: HashMap<String, Box<dyn ColorCamera + Send>>,
}

impl LiveVideoInjections {
    /// No injected producers.
    #[must_use]
    pub fn none() -> Self {
        Self::default()
    }

    /// Schema-default JPEG pump overrides for legacy serve configuration only.
    #[must_use]
    pub fn legacy_jpeg(
        external: Option<Box<dyn LiveJpegSource>>,
        webcam: Option<Box<dyn LiveJpegSource>>,
    ) -> Self {
        use crate::camera::{CAMERA_SOURCE_ID, EXTERNAL_CAMERA_SOURCE_ID};
        let mut jpeg_sources = HashMap::new();
        if let Some(source) = external {
            jpeg_sources.insert(EXTERNAL_CAMERA_SOURCE_ID.to_owned(), source);
        }
        if let Some(source) = webcam {
            jpeg_sources.insert(CAMERA_SOURCE_ID.to_owned(), source);
        }
        Self {
            jpeg_sources,
            cameras: HashMap::new(),
        }
    }

    /// Registers an injected MJPEG producer for `id`.
    #[must_use]
    pub fn with_jpeg_source(
        mut self,
        id: impl Into<String>,
        source: Box<dyn LiveJpegSource>,
    ) -> Self {
        self.jpeg_sources.insert(id.into(), source);
        self
    }

    /// Registers an injected camera for `id`.
    #[must_use]
    pub fn with_color_camera(
        mut self,
        id: impl Into<String>,
        camera: Box<dyn ColorCamera + Send>,
    ) -> Self {
        self.cameras.insert(id.into(), camera);
        self
    }
}

/// External camera index and description when that source is enabled.
pub struct ExternalVideoConfig<'a> {
    /// Native camera index.
    pub index: u32,
    /// Advertised description.
    pub description: &'a str,
}

/// Second webcam index when source `camera` is enabled.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WebcamVideoConfig {
    /// V4L2 index opened with the closest 1280×720 MJPEG mode.
    pub index: u32,
}

enum SourceInjection {
    Jpeg(Box<dyn LiveJpegSource>),
    Camera(Box<dyn ColorCamera + Send>),
}

enum CaptureDriver {
    Jpeg(Box<dyn LiveJpegSource>),
    Native(NativeCaptureDrain<NokhwaColorCamera>),
    Injected(NativeCaptureDrain<DynColorCamera>),
}

impl CaptureDriver {
    fn next_jpeg(&mut self) -> Result<Vec<u8>, A2aLabError> {
        match self {
            Self::Jpeg(source) => source.next_jpeg(),
            Self::Native(drain) => drain.latest_jpeg(),
            Self::Injected(drain) => drain.latest_jpeg(),
        }
    }

    #[expect(dead_code, reason = "diagnostics and legacy pump paths")]
    fn latest_captured(
        &mut self,
    ) -> Result<a2a_lab_dev_kit::videos::CapturedColorFrame, A2aLabError> {
        match self {
            Self::Jpeg(_) => Err(A2aLabError::invalid(
                "source",
                "JPEG injection does not expose captured color frames",
            )),
            Self::Native(drain) => drain.latest_captured(),
            Self::Injected(drain) => drain.latest_captured(),
        }
    }

    fn is_jpeg_injection(&self) -> bool {
        matches!(self, Self::Jpeg(_))
    }

    fn queue_depth(&self) -> usize {
        match self {
            Self::Jpeg(_) => 0,
            Self::Native(drain) => drain.queue_depth(),
            Self::Injected(drain) => drain.queue_depth(),
        }
    }

    fn phase(&self) -> Option<CaptureRunnerPhase> {
        match self {
            Self::Jpeg(_) => None,
            Self::Native(drain) => Some(drain.phase()),
            Self::Injected(drain) => Some(drain.phase()),
        }
    }

    fn poll_during(&mut self, duration: Duration) {
        match self {
            Self::Jpeg(_) => std::thread::sleep(duration),
            Self::Native(drain) => drain.wait_polling(duration),
            Self::Injected(drain) => drain.wait_polling(duration),
        }
    }

    fn reconnect_drain(&mut self) -> Result<(), A2aLabError> {
        match self {
            Self::Jpeg(_) => Ok(()),
            Self::Native(drain) => drain.reconnect(),
            Self::Injected(drain) => drain.reconnect(),
        }
    }

    fn shutdown_drain(&mut self) {
        match self {
            Self::Jpeg(_) => {}
            Self::Native(drain) => drain.shutdown(),
            Self::Injected(drain) => drain.shutdown(),
        }
    }

    fn drain_handoffs(
        &mut self,
        max_pulls: u32,
    ) -> Vec<a2a_lab_dev_kit::videos::CapturedColorFrame> {
        match self {
            Self::Jpeg(_) => Vec::new(),
            Self::Native(drain) => drain.drain_handoffs(max_pulls),
            Self::Injected(drain) => drain.drain_handoffs(max_pulls),
        }
    }
}

/// Starts live capture for configured sources and records frames in the indexed DVR.
#[allow(clippy::too_many_arguments)]
pub async fn start_live_video_catalog(
    sources: &[LiveVideoSourceDescriptor],
    fps: u32,
    live_retention_secs: u64,
    disk_retention_secs: u64,
    disk_root: &Path,
    max_bytes: u64,
    dvr_save_interval_nanos: Option<u64>,
    injections: LiveVideoInjections,
) -> Result<LiveVideoCatalog, A2aLabError> {
    start_live_video_catalog_with_semantic(
        sources,
        fps,
        live_retention_secs,
        disk_retention_secs,
        disk_root,
        max_bytes,
        dvr_save_interval_nanos,
        injections,
        None,
        SemanticServeInjections::default(),
    )
    .await
}

/// Starts live capture and optionally wires semantic workers before pumps run.
pub async fn start_live_video_catalog_with_semantic(
    sources: &[LiveVideoSourceDescriptor],
    fps: u32,
    live_retention_secs: u64,
    disk_retention_secs: u64,
    disk_root: &Path,
    max_bytes: u64,
    dvr_save_interval_nanos: Option<u64>,
    mut injections: LiveVideoInjections,
    semantic: Option<&ResolvedSemanticIndexing>,
    semantic_injections: SemanticServeInjections,
) -> Result<LiveVideoCatalog, A2aLabError> {
    validate_live_sources(sources)?;
    let live = MemoryVideos::new();
    let dvr = IndexedVideoDvr::open(disk_root, disk_retention_secs).await?;
    let semantic_bundle = match semantic {
        Some(settings) => {
            prepare_semantic_serve_bundle(settings, &dvr, &semantic_injections).await?
        }
        None => SemanticServeBundle::empty(),
    };
    let annotation_workers = semantic_bundle.workers.clone();
    let decision_worker = semantic_bundle.decision_worker.clone();
    let inner = VideoDvrCatalog::open(live, dvr, Duration::from_secs(60))?;
    for source in sources {
        let source_id = VideoSourceId::new(&source.id)?;
        insert_source(
            inner.live(),
            &source_id,
            &source.name,
            &source.description,
            source.width.unwrap_or(640),
            source.height.unwrap_or(480),
            fps,
        )
        .await?;
    }
    let maintenance = if sources.is_empty() {
        None
    } else {
        Some(inner.maintain_disk())
    };
    let (stop, rx) = watch::channel(false);
    let annotation = inner.maintain_annotation_jobs(rx.clone(), annotation_workers);
    if let Some((coverage, reason)) = semantic_bundle.decision_unavailable {
        let now_us = crate::time::now()?.as_micros();
        let closed = inner
            .dvr()
            .mark_decision_evaluation_jobs_unavailable(coverage, reason, now_us)
            .await?;
        if closed > 0 {
            eprintln!("video DVR: Jev unavailable; closed {closed} decision jobs as {reason}");
        }
    }
    let decision_evaluation = inner.maintain_decision_evaluation_jobs(rx.clone(), decision_worker);
    let retain_us =
        i64::try_from(live_retention_secs.saturating_mul(1_000_000)).unwrap_or(i64::MAX);
    let frame_period_us = 1_000_000u64 / u64::from(fps.max(1));
    let dvr_frame_period_us = dvr_save_interval_nanos
        .map_or(frame_period_us, |nanos| nanos / 1_000)
        .max(1);
    let runtime = tokio::runtime::Handle::current();
    for source in sources {
        let source_id = VideoSourceId::new(&source.id)?;
        let injection = take_injection(source, &mut injections);
        spawn_pump(
            inner.clone(),
            source.clone(),
            source_id,
            fps,
            max_bytes,
            dvr_save_interval_nanos,
            injection,
            rx.clone(),
            retain_us,
            frame_period_us,
            dvr_frame_period_us,
            &runtime,
        );
    }
    Ok(LiveVideoCatalog {
        inner,
        maintenance,
        annotation,
        decision_evaluation,
        semantic: semantic_bundle,
        stop,
    })
}

/// Legacy two-slot entry point built from optional external and `camera` sources.
#[allow(clippy::too_many_arguments)]
pub async fn start_live_videos(
    external: Option<ExternalVideoConfig<'_>>,
    webcam: Option<WebcamVideoConfig>,
    fps: u32,
    live_retention_secs: u64,
    disk_retention_secs: u64,
    disk_root: &Path,
    max_bytes: u64,
    injections: LiveVideoInjections,
) -> Result<LiveVideoCatalog, A2aLabError> {
    let mut sources = Vec::new();
    if let Some(config) = external {
        sources.push(LiveVideoSourceDescriptor::legacy_external(
            config.index,
            config.description,
        ));
    }
    if let Some(config) = webcam {
        sources.push(LiveVideoSourceDescriptor::legacy_camera(config.index));
    }
    start_live_video_catalog(
        &sources,
        fps,
        live_retention_secs,
        disk_retention_secs,
        disk_root,
        max_bytes,
        None,
        injections,
    )
    .await
}

/// Starts `external-camera` live capture when an index is configured.
#[expect(dead_code, reason = "public API for external-only callers")]
#[allow(clippy::too_many_arguments)]
pub async fn start_external_video(
    index: u32,
    description: &str,
    fps: u32,
    live_retention_secs: u64,
    disk_retention_secs: u64,
    disk_root: &Path,
    max_bytes: u64,
    source: Option<Box<dyn LiveJpegSource>>,
) -> Result<LiveVideoCatalog, A2aLabError> {
    start_live_videos(
        Some(ExternalVideoConfig { index, description }),
        None,
        fps,
        live_retention_secs,
        disk_retention_secs,
        disk_root,
        max_bytes,
        LiveVideoInjections::legacy_jpeg(source, None),
    )
    .await
}

fn take_injection(
    source: &LiveVideoSourceDescriptor,
    injections: &mut LiveVideoInjections,
) -> Option<SourceInjection> {
    if let Some(camera) = injections.cameras.remove(&source.id) {
        return Some(SourceInjection::Camera(camera));
    }
    injections
        .jpeg_sources
        .remove(&source.id)
        .map(SourceInjection::Jpeg)
}

#[allow(clippy::too_many_arguments)]
fn spawn_pump(
    catalog: VideoDvrCatalog,
    descriptor: LiveVideoSourceDescriptor,
    source_id: VideoSourceId,
    fps: u32,
    max_bytes: u64,
    dvr_save_interval_nanos: Option<u64>,
    injected: Option<SourceInjection>,
    stop: watch::Receiver<bool>,
    retain_us: i64,
    frame_period_us: u64,
    dvr_frame_period_us: u64,
    runtime: &tokio::runtime::Handle,
) {
    let runtime = runtime.clone();
    std::thread::spawn(move || {
        pump(
            &catalog,
            &descriptor,
            &source_id,
            fps,
            max_bytes,
            dvr_save_interval_nanos,
            injected,
            stop,
            retain_us,
            frame_period_us,
            dvr_frame_period_us,
            &runtime,
        );
    });
}

#[allow(
    clippy::too_many_arguments,
    clippy::cognitive_complexity,
    clippy::needless_late_init
)]
#[allow(clippy::too_many_lines)]
fn pump(
    catalog: &VideoDvrCatalog,
    descriptor: &LiveVideoSourceDescriptor,
    source_id: &VideoSourceId,
    fps: u32,
    max_bytes: u64,
    dvr_save_interval_nanos: Option<u64>,
    injected: Option<SourceInjection>,
    mut stop: watch::Receiver<bool>,
    retain_us: i64,
    _frame_period_us: u64,
    dvr_frame_period_us: u64,
    runtime: &tokio::runtime::Handle,
) {
    let budget = frame_budget(fps);
    let deliver_h264 = descriptor.delivery == LiveVideoDelivery::H264;
    let allow_native = injected.is_none();
    let retain_secs = u64::try_from(retain_us.max(1) / 1_000_000).unwrap_or(1);
    let live_period_ns = 1_000_000_000u64 / u64::from(fps.max(1));
    let dvr_period_ns = dvr_save_interval_nanos.unwrap_or(live_period_ns);
    let mut jpeg_cadence =
        JpegCadenceSelector::new(source_id.clone(), live_period_ns, dvr_period_ns, 8).ok();
    let mut cadence_sequence = 0_u64;
    let mut cadence_monotonic = 0_u64;
    let mut color_pump = if deliver_h264 {
        match catalog
            .live_h264()
            .register(source_id, retain_secs, fps, max_bytes)
        {
            Ok(live_store) => {
                match LiveColorEncodePump::open(
                    source_id.clone(),
                    live_store,
                    fps,
                    dvr_save_interval_nanos,
                ) {
                    Ok(pump) => Some(pump),
                    Err(error) => {
                        eprintln!("{} h264 pipeline: {error}", descriptor.id);
                        None
                    }
                }
            }
            Err(error) => {
                eprintln!("{} live h264 store: {error}", descriptor.id);
                None
            }
        }
    } else {
        None
    };
    let mut driver = match injected {
        Some(SourceInjection::Jpeg(jpeg)) => Some(CaptureDriver::Jpeg(jpeg)),
        Some(SourceInjection::Camera(camera)) => {
            match open_injected_drain(descriptor, source_id, fps, max_bytes, camera) {
                Ok(drain) => Some(CaptureDriver::Injected(drain)),
                Err(error) => {
                    eprintln!("{}: {error}", descriptor.id);
                    let _ = runtime.block_on(catalog.live().set_live_available(source_id, false));
                    return;
                }
            }
        }
        None => None,
    };
    let (disk_tx, mut disk_rx) = mpsc::channel(64);
    let disk_catalog = catalog.clone();
    runtime.spawn(async move {
        while let Some(job) = disk_rx.recv().await {
            if let Err(error) = write_disk(&disk_catalog, job).await {
                eprintln!("video DVR: {error}");
            }
        }
    });
    let mut disk_drops = DiskDropLog::new(source_id.as_str());
    while !*stop.borrow() {
        let started = Instant::now();
        if driver.is_none() && allow_native {
            match open_native_session_capture(descriptor, source_id.clone(), fps) {
                Ok(source) => {
                    driver = Some(CaptureDriver::Native(NativeCaptureDrain::new(
                        source,
                        &descriptor.id,
                        max_bytes,
                    )));
                }
                Err(error) => {
                    eprintln!("{}: {error}", descriptor.id);
                    if mark_down(catalog, source_id, runtime, &mut stop) {
                        return;
                    }
                    continue;
                }
            }
        }
        let use_color_pipeline = color_pump.is_some()
            && driver
                .as_ref()
                .is_some_and(|active| !active.is_jpeg_injection());
        let mut live_published = false;
        let published;
        if use_color_pipeline {
            let handoff_budget = if dvr_save_interval_nanos.is_some() {
                32
            } else {
                1
            };
            let pulled = driver.as_mut().unwrap().drain_handoffs(handoff_budget);
            if pulled.is_empty() {
                if let Some(active) = driver.as_mut() {
                    if let Some(CaptureRunnerPhase::SourceFaulted) = active.phase() {
                        if active.reconnect_drain().is_ok() {
                            if let Some(pump) = color_pump.as_mut() {
                                pump.reconnect();
                            }
                        } else {
                            active.shutdown_drain();
                            driver = None;
                        }
                    } else if active.queue_depth() == 0 {
                        // Startup warmup: keep draining until the first frame lands.
                    } else if !active.is_jpeg_injection() {
                        active.shutdown_drain();
                        driver = None;
                        if let Some(pump) = color_pump.as_mut() {
                            pump.source_lost();
                        }
                    }
                }
                let _ = runtime.block_on(catalog.live().set_live_available(source_id, false));
                let delay = Duration::from_millis(200);
                if wait(&mut stop, delay) {
                    return;
                }
                continue;
            }
            for frame in pulled {
                let frame = match normalize_mjpeg_frame(frame, max_bytes) {
                    Ok(frame) => frame,
                    Err(error) => {
                        eprintln!("{}: {error}", descriptor.id);
                        continue;
                    }
                };
                let captured_at = frame.timing().captured_at();
                let warmup_jpeg = captured_frame_jpeg(&frame, max_bytes).ok();
                match color_pump.as_mut().unwrap().push_frame(frame) {
                    Ok(units) => {
                        if units.is_empty()
                            && let Some(jpeg) = warmup_jpeg.as_ref()
                            && jpeg_dimensions(jpeg).is_ok()
                            && runtime
                                .block_on(publish_live(
                                    catalog,
                                    source_id,
                                    jpeg,
                                    captured_at,
                                    retain_us,
                                ))
                                .is_ok()
                        {
                            if let Some(jpeg) = warmup_jpeg {
                                remember_live_still(&descriptor.id, jpeg);
                            }
                            live_published = true;
                        }
                        for unit in units {
                            if !unit.still_jpeg.is_empty()
                                && jpeg_dimensions(&unit.still_jpeg).is_ok()
                            {
                                remember_live_still(&descriptor.id, unit.still_jpeg.clone());
                            }
                            if unit.destinations.live() && !unit.still_jpeg.is_empty() {
                                if runtime
                                    .block_on(publish_live(
                                        catalog,
                                        source_id,
                                        &unit.still_jpeg,
                                        unit.captured_at,
                                        retain_us,
                                    ))
                                    .is_err()
                                {
                                    return;
                                }
                                live_published = true;
                            }
                            if unit.fans_to_disk()
                                && !unit.initialization.is_empty()
                                && !unit.sample.is_empty()
                            {
                                queue_disk(
                                    &disk_tx,
                                    DiskJob::h264(
                                        source_id.clone(),
                                        unit.captured_at,
                                        dvr_frame_period_us,
                                        unit.keyframe,
                                        unit.initialization.clone(),
                                        unit.sample.clone(),
                                    ),
                                    &mut disk_drops,
                                );
                            }
                        }
                    }
                    Err(error) => {
                        eprintln!("{} h264 pipeline: {error}", descriptor.id);
                        if let Some(pump) = color_pump.as_mut() {
                            pump.reconnect();
                        }
                    }
                }
            }
            published = live_published;
        } else {
            let jpeg = match driver.as_mut() {
                Some(driver) => driver.next_jpeg(),
                None => Err(A2aLabError::unavailable("capture session is closed")),
            };
            let jpeg = match jpeg {
                Ok(jpeg) => jpeg,
                Err(error) => {
                    eprintln!("{}: {error}", descriptor.id);
                    if let Some(active) = driver.as_mut() {
                        if let Some(CaptureRunnerPhase::SourceFaulted) = active.phase() {
                            if active.reconnect_drain().is_err() {
                                active.shutdown_drain();
                                driver = None;
                            }
                        } else if matches!(error, A2aLabError::Unavailable { .. })
                            && active.queue_depth() == 0
                        {
                            // Startup warmup: keep draining until the first frame lands.
                        } else if !active.is_jpeg_injection() {
                            active.shutdown_drain();
                            driver = None;
                        }
                    }
                    let _ = runtime.block_on(catalog.live().set_live_available(source_id, false));
                    let delay = if driver
                        .as_ref()
                        .is_some_and(CaptureDriver::is_jpeg_injection)
                    {
                        Duration::from_millis(50)
                    } else {
                        Duration::from_millis(200)
                    };
                    if wait(&mut stop, delay) {
                        return;
                    }
                    continue;
                }
            };
            if jpeg_dimensions(&jpeg).is_err() {
                continue;
            }
            remember_live_still(&descriptor.id, jpeg.clone());
            let Ok(captured_at) = crate::time::now() else {
                continue;
            };
            let mut to_live = true;
            let mut to_disk = true;
            if let Some(selector) = jpeg_cadence.as_mut() {
                let dispatches = selector
                    .push_tick(cadence_sequence, captured_at, cadence_monotonic)
                    .unwrap_or_default();
                cadence_sequence = cadence_sequence.saturating_add(1);
                cadence_monotonic = cadence_monotonic.saturating_add(live_period_ns);
                to_live = dispatches
                    .iter()
                    .any(|dispatch| dispatch.destinations().live());
                to_disk = dispatches
                    .iter()
                    .any(|dispatch| dispatch.destinations().dvr());
            }
            if to_live {
                if runtime
                    .block_on(publish_live(
                        catalog,
                        source_id,
                        &jpeg,
                        captured_at,
                        retain_us,
                    ))
                    .is_err()
                {
                    return;
                }
                live_published = true;
            }
            if to_disk {
                queue_disk(
                    &disk_tx,
                    DiskJob::mjpeg(source_id.clone(), captured_at, dvr_frame_period_us, jpeg),
                    &mut disk_drops,
                );
            }
            published = live_published;
        }
        if !published {
            if let Some(rest) = budget.checked_sub(started.elapsed()) {
                if let Some(active) = driver.as_mut() {
                    active.poll_during(rest);
                    if *stop.borrow() {
                        return;
                    }
                } else if wait(&mut stop, rest) {
                    return;
                }
            }
            continue;
        }
        let _ = runtime.block_on(catalog.live().set_live_available(source_id, true));
        if let Some(rest) = budget.checked_sub(started.elapsed()) {
            if let Some(active) = driver.as_mut() {
                active.poll_during(rest);
                if *stop.borrow() {
                    return;
                }
            } else if wait(&mut stop, rest) {
                return;
            }
        }
    }
    if let Some(active) = driver.as_mut() {
        active.shutdown_drain();
    }
    if let Some(pump) = color_pump.as_mut() {
        pump.cancel();
    }
}

fn open_injected_drain(
    descriptor: &LiveVideoSourceDescriptor,
    source_id: &VideoSourceId,
    fps: u32,
    max_bytes: u64,
    camera: Box<dyn ColorCamera + Send>,
) -> Result<NativeCaptureDrain<DynColorCamera>, A2aLabError> {
    let source =
        SessionColorCapture::open(DynColorCamera(camera), descriptor, source_id.clone(), fps)?;
    Ok(NativeCaptureDrain::new(source, &descriptor.id, max_bytes))
}

async fn insert_source(
    live: &MemoryVideos,
    id: &VideoSourceId,
    name: &str,
    description: &str,
    width: u32,
    height: u32,
    fps: u32,
) -> Result<(), A2aLabError> {
    live.insert_source(VideoSource::new(
        id.clone(),
        name,
        description,
        None,
        None,
        width,
        height,
        fps,
        MJPEG_MEDIA_TYPE,
    )?)
    .await
}

enum DiskJob {
    Mjpeg {
        source_id: VideoSourceId,
        captured_at: a2a_lab_dev_kit::UtcTimestamp,
        frame_period_us: u64,
        jpeg: Vec<u8>,
    },
    H264 {
        source_id: VideoSourceId,
        captured_at: a2a_lab_dev_kit::UtcTimestamp,
        frame_period_us: u64,
        keyframe: bool,
        codec_init: Vec<u8>,
        sample: Vec<u8>,
    },
}

impl DiskJob {
    fn mjpeg(
        source_id: VideoSourceId,
        captured_at: a2a_lab_dev_kit::UtcTimestamp,
        frame_period_us: u64,
        jpeg: Vec<u8>,
    ) -> Self {
        Self::Mjpeg {
            source_id,
            captured_at,
            frame_period_us,
            jpeg,
        }
    }

    fn h264(
        source_id: VideoSourceId,
        captured_at: a2a_lab_dev_kit::UtcTimestamp,
        frame_period_us: u64,
        keyframe: bool,
        codec_init: Vec<u8>,
        sample: Vec<u8>,
    ) -> Self {
        Self::H264 {
            source_id,
            captured_at,
            frame_period_us,
            keyframe,
            codec_init,
            sample,
        }
    }
}

const DISK_DROP_REPORT_INTERVAL: Duration = Duration::from_secs(5);

struct DiskDropLog {
    source: String,
    dropped: u64,
    last_report: Option<Instant>,
    closed_reported: bool,
}

impl DiskDropLog {
    fn new(source: &str) -> Self {
        Self {
            source: source.to_owned(),
            dropped: 0,
            last_report: None,
            closed_reported: false,
        }
    }

    fn observe(&mut self, now: Instant, closed: bool) -> Option<String> {
        if closed {
            if self.closed_reported {
                return None;
            }
            self.closed_reported = true;
            return Some(format!(
                "video DVR: {} disk writer closed; dropped frames are no longer queued",
                self.source
            ));
        }
        self.dropped = self.dropped.saturating_add(1);
        let due = self
            .last_report
            .is_none_or(|last| now.saturating_duration_since(last) >= DISK_DROP_REPORT_INTERVAL);
        if !due {
            return None;
        }
        self.last_report = Some(now);
        Some(format!(
            "video DVR: {} disk writer is behind; dropped {} frames",
            self.source, self.dropped
        ))
    }
}

fn queue_disk(tx: &mpsc::Sender<DiskJob>, job: DiskJob, drops: &mut DiskDropLog) {
    let closed = match tx.try_send(job) {
        Ok(()) => return,
        Err(TrySendError::Closed(_)) => true,
        Err(TrySendError::Full(_)) => false,
    };
    if let Some(message) = drops.observe(Instant::now(), closed) {
        eprintln!("{message}");
    }
}

async fn write_disk(catalog: &VideoDvrCatalog, job: DiskJob) -> Result<(), A2aLabError> {
    match job {
        DiskJob::Mjpeg {
            source_id,
            captured_at,
            frame_period_us,
            jpeg,
        } => {
            catalog
                .dvr()
                .append_mjpeg_frame(&source_id, captured_at, frame_period_us, &jpeg)
                .await
        }
        DiskJob::H264 {
            source_id,
            captured_at,
            frame_period_us,
            keyframe,
            codec_init,
            sample,
        } => {
            catalog
                .dvr()
                .append_h264_sample(
                    &source_id,
                    captured_at,
                    frame_period_us,
                    keyframe,
                    &codec_init,
                    &sample,
                )
                .await
        }
    }
}

async fn publish_live(
    catalog: &VideoDvrCatalog,
    source_id: &VideoSourceId,
    jpeg: &[u8],
    captured_at: a2a_lab_dev_kit::UtcTimestamp,
    live_retain_us: i64,
) -> Result<(), A2aLabError> {
    catalog
        .live()
        .insert_frame(source_id.clone(), captured_at, jpeg.to_vec())
        .await?;
    catalog
        .live()
        .forget_before(
            source_id,
            captured_at.saturating_add_micros(-live_retain_us),
        )
        .await
}

fn mark_down(
    catalog: &VideoDvrCatalog,
    source_id: &VideoSourceId,
    runtime: &tokio::runtime::Handle,
    stop: &mut watch::Receiver<bool>,
) -> bool {
    let _ = runtime.block_on(catalog.live().set_live_available(source_id, false));
    wait(stop, Duration::from_secs(2))
}

fn wait(stop: &mut watch::Receiver<bool>, wait: Duration) -> bool {
    if *stop.borrow() {
        return true;
    }
    std::thread::sleep(wait);
    *stop.borrow()
}

#[cfg(test)]
mod tests {
    use std::time::Instant;

    use a2a_lab_dev_kit::VideoSourceId;
    use tokio::sync::mpsc;

    use super::{DISK_DROP_REPORT_INTERVAL, DiskDropLog, DiskJob, queue_disk};

    #[test]
    fn disk_drop_reports_are_source_scoped_and_rate_limited() {
        let start = Instant::now();
        let mut camera = DiskDropLog::new("camera");
        let mut external = DiskDropLog::new("external-camera");
        let first = camera.observe(start, false).unwrap();
        assert_eq!(
            first,
            "video DVR: camera disk writer is behind; dropped 1 frames"
        );
        assert!(camera.observe(start, false).is_none());
        let other = external.observe(start, false).unwrap();
        assert!(other.contains("external-camera"));
        assert!(other.contains("dropped 1 frames"));
        let later = start + DISK_DROP_REPORT_INTERVAL;
        let next = camera.observe(later, false).unwrap();
        assert_eq!(
            next,
            "video DVR: camera disk writer is behind; dropped 3 frames"
        );
        let closed = camera.observe(later, true).unwrap();
        assert_eq!(
            closed,
            "video DVR: camera disk writer closed; dropped frames are no longer queued"
        );
        assert!(camera.observe(later, true).is_none());
        let external_again = external
            .observe(later + DISK_DROP_REPORT_INTERVAL, false)
            .unwrap();
        assert!(external_again.contains("dropped 2 frames"));
    }

    #[tokio::test]
    async fn dropped_disk_queue_jobs_never_reach_dvr_append() {
        let (tx, mut rx) = mpsc::channel(1);
        let source = VideoSourceId::new("camera").unwrap();
        let capture = crate::time::now().unwrap();
        let first = DiskJob::mjpeg(
            source.clone(),
            capture,
            1_000_000,
            vec![0xFF, 0xD8, 0x01, 0xD9],
        );
        let second = DiskJob::mjpeg(source, capture, 1_000_000, vec![0xFF, 0xD8, 0x02, 0xD9]);
        let mut drops = DiskDropLog::new("camera");
        queue_disk(&tx, first, &mut drops);
        queue_disk(&tx, second, &mut drops);
        let received = rx.recv().await.unwrap();
        assert!(matches!(received, DiskJob::Mjpeg { .. }));
        assert!(rx.try_recv().is_err() || drops.dropped >= 1);
    }
}
