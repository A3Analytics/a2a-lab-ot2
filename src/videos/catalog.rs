//! Memory video catalog plus cancellable capture pumps per camera.

use std::collections::{HashMap, VecDeque};
use std::path::Path;
use std::time::{Duration, Instant};

use a2a_lab_dev_kit::videos::{
    CaptureClock, CaptureFormat, CaptureFormatPreference, CaptureIdentity, CaptureSession,
    CaptureTiming, ColorCamera, ColorMatrix, ColorProperties, ColorRange,
};
use a2a_lab_dev_kit::{A2aLabError, MJPEG_MEDIA_TYPE, MemoryVideos, VideoSource, VideoSourceId};
use tokio::sync::mpsc::error::TrySendError;
use tokio::sync::{mpsc, watch};

use crate::camera::{remember_camera_frame, remember_external_frame};
use crate::images::jpeg_dimensions;
use crate::videos::dvr::{IndexedVideoDvr, VideoDvrCatalog, VideoDvrMaintenance};
use crate::videos::external::{LiveJpegSource, frame_budget};
use crate::videos::h264::H264Encoder;
use crate::videos::nokhwa_camera::NokhwaColorCamera;
use crate::videos::raw_jpeg::captured_frame_jpeg;
use crate::videos::source::{
    LiveCameraInputFormat, LiveVideoDelivery, LiveVideoSourceDescriptor, validate_live_sources,
};

/// Live video catalog and the capture threads that fill it.
pub struct LiveVideoCatalog {
    inner: VideoDvrCatalog,
    #[allow(dead_code)] // keeps periodic cleanup alive until catalog drop
    maintenance: Option<VideoDvrMaintenance>,
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
    /// Replaces legacy `external-camera` when set.
    pub external: Option<Box<dyn LiveJpegSource>>,
    /// Replaces legacy `camera` when set.
    pub webcam: Option<Box<dyn LiveJpegSource>>,
    /// Injected [`ColorCamera`] backends keyed by stable source id.
    pub cameras: HashMap<String, Box<dyn ColorCamera + Send>>,
}

impl LiveVideoInjections {
    /// No injected producers.
    #[must_use]
    pub fn none() -> Self {
        Self::default()
    }

    /// Legacy JPEG pump overrides for `external-camera` and `camera`.
    #[must_use]
    pub fn legacy_jpeg(
        external: Option<Box<dyn LiveJpegSource>>,
        webcam: Option<Box<dyn LiveJpegSource>>,
    ) -> Self {
        Self {
            external,
            webcam,
            cameras: HashMap::new(),
        }
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
    SessionNative(CaptureSession<NokhwaColorCamera>),
    SessionInjected(CaptureSession<DynColorCamera>),
}

struct DynColorCamera(Box<dyn ColorCamera + Send>);

impl ColorCamera for DynColorCamera {
    fn advertised(
        &mut self,
    ) -> Result<Vec<a2a_lab_dev_kit::videos::AdvertisedCaptureProfile>, A2aLabError> {
        self.0.advertised()
    }

    fn open(
        &mut self,
        profile: &a2a_lab_dev_kit::videos::SelectedCaptureProfile,
    ) -> Result<a2a_lab_dev_kit::videos::SelectedCaptureProfile, A2aLabError> {
        self.0.open(profile)
    }

    fn raw_frame(&mut self) -> Result<a2a_lab_dev_kit::videos::RawCameraSample, A2aLabError> {
        self.0.raw_frame()
    }

    fn stop(&mut self) -> Result<(), A2aLabError> {
        self.0.stop()
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
    mut injections: LiveVideoInjections,
) -> Result<LiveVideoCatalog, A2aLabError> {
    validate_live_sources(sources)?;
    let live = MemoryVideos::new();
    let dvr = IndexedVideoDvr::open(disk_root, disk_retention_secs).await?;
    let inner = VideoDvrCatalog::open(live, dvr, Duration::from_secs(60))?;
    for source in sources {
        let source_id = VideoSourceId::new(&source.id)?;
        insert_source(
            inner.live(),
            &source_id,
            &source.name,
            &source.description,
            source.width,
            source.height,
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
    let retain_us =
        i64::try_from(live_retention_secs.saturating_mul(1_000_000)).unwrap_or(i64::MAX);
    let frame_period_us = 1_000_000u64 / u64::from(fps.max(1));
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
            injection,
            rx.clone(),
            retain_us,
            frame_period_us,
            &runtime,
        );
    }
    Ok(LiveVideoCatalog {
        inner,
        maintenance,
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
    if source.remembers_external_still() {
        return injections.external.take().map(SourceInjection::Jpeg);
    }
    if source.remembers_camera_still() {
        return injections.webcam.take().map(SourceInjection::Jpeg);
    }
    None
}

#[allow(clippy::too_many_arguments)]
fn spawn_pump(
    catalog: VideoDvrCatalog,
    descriptor: LiveVideoSourceDescriptor,
    source_id: VideoSourceId,
    fps: u32,
    max_bytes: u64,
    injected: Option<SourceInjection>,
    stop: watch::Receiver<bool>,
    retain_us: i64,
    frame_period_us: u64,
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
            injected,
            stop,
            retain_us,
            frame_period_us,
            &runtime,
        );
    });
}

#[allow(clippy::too_many_arguments, clippy::cognitive_complexity)]
#[allow(clippy::too_many_lines)]
fn pump(
    catalog: &VideoDvrCatalog,
    descriptor: &LiveVideoSourceDescriptor,
    source_id: &VideoSourceId,
    fps: u32,
    max_bytes: u64,
    injected: Option<SourceInjection>,
    mut stop: watch::Receiver<bool>,
    retain_us: i64,
    frame_period_us: u64,
    runtime: &tokio::runtime::Handle,
) {
    let budget = frame_budget(fps);
    let encode_h264 = descriptor.delivery == LiveVideoDelivery::H264 && injected.is_none();
    let allow_native = injected.is_none();
    let mut driver = match injected {
        Some(SourceInjection::Jpeg(jpeg)) => Some(CaptureDriver::Jpeg(jpeg)),
        Some(SourceInjection::Camera(camera)) => {
            match open_injected_session(descriptor, fps, camera) {
                Ok(session) => Some(CaptureDriver::SessionInjected(session)),
                Err(error) => {
                    eprintln!("{}: {error}", descriptor.id);
                    let _ = runtime.block_on(catalog.live().set_live_available(source_id, false));
                    return;
                }
            }
        }
        None => None,
    };
    let mut encoder: Option<H264Encoder> = None;
    let mut encode_stamps = VecDeque::new();
    let mut codec_init = Vec::new();
    let (disk_tx, mut disk_rx) = mpsc::channel(64);
    let disk_catalog = catalog.clone();
    runtime.spawn(async move {
        while let Some(job) = disk_rx.recv().await {
            if let Err(error) = write_disk(&disk_catalog, job).await {
                eprintln!("video DVR: {error}");
            }
        }
    });
    let mut sequence = 0_u64;
    let mut disk_drops = DiskDropLog::new(source_id.as_str());
    while !*stop.borrow() {
        let started = Instant::now();
        if driver.is_none() && allow_native {
            match open_native_session(descriptor, fps) {
                Ok(session) => driver = Some(CaptureDriver::SessionNative(session)),
                Err(error) => {
                    eprintln!("{}: {error}", descriptor.id);
                    if mark_down(catalog, source_id, runtime, &mut stop) {
                        return;
                    }
                    continue;
                }
            }
        }
        let jpeg = match driver.as_mut() {
            Some(CaptureDriver::Jpeg(source)) => source.next_jpeg(),
            Some(CaptureDriver::SessionNative(session)) => {
                read_session_jpeg(session, source_id, &mut sequence, max_bytes, descriptor)
            }
            Some(CaptureDriver::SessionInjected(session)) => {
                read_session_jpeg(session, source_id, &mut sequence, max_bytes, descriptor)
            }
            None => Err(A2aLabError::unavailable("capture session is closed")),
        };
        let jpeg = match jpeg {
            Ok(jpeg) => jpeg,
            Err(error) => {
                eprintln!("{}: {error}", descriptor.id);
                if !matches!(driver.as_ref(), Some(CaptureDriver::Jpeg(_))) {
                    driver = None;
                }
                let _ = runtime.block_on(catalog.live().set_live_available(source_id, false));
                let delay = if matches!(driver.as_ref(), Some(CaptureDriver::Jpeg(_))) {
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
        if descriptor.remembers_camera_still() {
            remember_camera_frame(jpeg.clone());
        }
        if descriptor.remembers_external_still() {
            remember_external_frame(jpeg.clone());
        }
        let published = if encode_h264 {
            if encoder.is_none() {
                match H264Encoder::spawn() {
                    Ok(opened) => encoder = Some(opened),
                    Err(error) => {
                        eprintln!("{} h264: {error}", descriptor.id);
                        let _ =
                            runtime.block_on(catalog.live().set_live_available(source_id, false));
                        if wait(&mut stop, Duration::from_secs(2)) {
                            return;
                        }
                        continue;
                    }
                }
            }
            let Ok(stamp) = crate::time::now() else {
                continue;
            };
            encode_stamps.push_back(stamp);
            let pushed = encoder.as_mut().map(|encoder| encoder.push(&jpeg));
            match pushed {
                Some(Ok(None)) | None => false,
                Some(Ok(Some(unit))) => {
                    let captured_at = encode_stamps.pop_front().unwrap_or(stamp);
                    if !unit.parameter_sets.is_empty() {
                        codec_init.clone_from(&unit.parameter_sets);
                    }
                    if codec_init.is_empty() || unit.sample.is_empty() {
                        false
                    } else if runtime
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
                    } else {
                        queue_disk(
                            &disk_tx,
                            DiskJob::h264(
                                source_id.clone(),
                                captured_at,
                                frame_period_us,
                                unit.keyframe,
                                codec_init.clone(),
                                unit.sample,
                            ),
                            &mut disk_drops,
                        );
                        true
                    }
                }
                Some(Err(error)) => {
                    eprintln!("{} h264: {error}", descriptor.id);
                    encoder = None;
                    encode_stamps.clear();
                    codec_init.clear();
                    false
                }
            }
        } else {
            let Ok(captured_at) = crate::time::now() else {
                continue;
            };
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
            queue_disk(
                &disk_tx,
                DiskJob::mjpeg(source_id.clone(), captured_at, frame_period_us, jpeg),
                &mut disk_drops,
            );
            true
        };
        if !published {
            if let Some(rest) = budget.checked_sub(started.elapsed())
                && wait(&mut stop, rest)
            {
                return;
            }
            continue;
        }
        let _ = runtime.block_on(catalog.live().set_live_available(source_id, true));
        if let Some(rest) = budget.checked_sub(started.elapsed())
            && wait(&mut stop, rest)
        {
            return;
        }
    }
    match driver.as_mut() {
        Some(CaptureDriver::SessionNative(session)) => {
            let _ = session.stop();
        }
        Some(CaptureDriver::SessionInjected(session)) => {
            let _ = session.stop();
        }
        _ => {}
    }
}

fn open_injected_session(
    descriptor: &LiveVideoSourceDescriptor,
    fps: u32,
    camera: Box<dyn ColorCamera + Send>,
) -> Result<CaptureSession<DynColorCamera>, A2aLabError> {
    let request = descriptor.capture_request(fps)?;
    let mut session = CaptureSession::new(DynColorCamera(camera), request);
    session.open()?;
    Ok(session)
}

fn open_native_session(
    descriptor: &LiveVideoSourceDescriptor,
    fps: u32,
) -> Result<CaptureSession<NokhwaColorCamera>, A2aLabError> {
    let request = descriptor.capture_request(fps)?;
    let mut session = CaptureSession::new(
        NokhwaColorCamera::open_index(descriptor.device_index)?,
        request,
    );
    session.open()?;
    Ok(session)
}

fn read_session_jpeg(
    session: &mut CaptureSession<impl ColorCamera>,
    source_id: &VideoSourceId,
    sequence: &mut u64,
    max_bytes: u64,
    descriptor: &LiveVideoSourceDescriptor,
) -> Result<Vec<u8>, A2aLabError> {
    let captured_at = crate::time::now()?;
    let identity = CaptureIdentity::new(source_id.clone(), *sequence);
    *sequence = sequence.saturating_add(1);
    let timing = CaptureTiming::new(captured_at, CaptureClock::monotonic(0));
    let color = match descriptor.input {
        LiveCameraInputFormat::Yuy2 => {
            ColorProperties::new(ColorMatrix::Bt601, ColorRange::Limited)
        }
        LiveCameraInputFormat::Mjpeg => {
            ColorProperties::new(ColorMatrix::Unknown, ColorRange::Unknown)
        }
    };
    let frame = session.read(identity, timing, color)?;
    let selected = session.selected().ok_or_else(|| {
        A2aLabError::invalid("camera", "capture session has no negotiated profile")
    })?;
    let negotiated = selected.mode().format();
    let expected = descriptor.input.capture_preference();
    let matches = match expected {
        CaptureFormatPreference::Mjpeg => negotiated == CaptureFormat::Mjpeg,
        CaptureFormatPreference::Yuy2 => negotiated == CaptureFormat::Yuy2,
        CaptureFormatPreference::Auto => true,
    };
    if !matches {
        return Err(A2aLabError::invalid(
            "format",
            "negotiated raw layout does not match the configured input format",
        ));
    }
    if frame.format() != negotiated {
        return Err(A2aLabError::invalid(
            "format",
            "raw sample layout does not match the negotiated input format",
        ));
    }
    captured_frame_jpeg(&frame, max_bytes)
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

    use super::{DISK_DROP_REPORT_INTERVAL, DiskDropLog};

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
}
