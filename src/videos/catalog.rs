//! Memory video catalog plus cancellable capture pumps per camera.

use std::collections::VecDeque;
use std::path::Path;
use std::time::{Duration, Instant};

use a2a_lab_dev_kit::{A2aLabError, MJPEG_MEDIA_TYPE, MemoryVideos, VideoSource, VideoSourceId};
use tokio::sync::watch;

use crate::camera::{
    CAMERA_SOURCE_ID, EXTERNAL_CAMERA_HEIGHT, EXTERNAL_CAMERA_SOURCE_ID, EXTERNAL_CAMERA_WIDTH,
    remember_camera_frame,
};
use crate::images::jpeg_dimensions;
use crate::videos::dvr::{IndexedVideoDvr, VideoDvrCatalog, VideoDvrMaintenance};
use crate::videos::external::{LiveJpegSource, NativeLiveJpeg, frame_budget};
use crate::videos::h264::H264Encoder;

const CAMERA_VIDEO_WIDTH: u32 = 1280;
const CAMERA_VIDEO_HEIGHT: u32 = 720;
const CAMERA_VIDEO_DESCRIPTION: &str = "Live MJPEG from the configured camera.";

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
    /// Replaces the external Nokhwa session when set.
    pub external: Option<Box<dyn LiveJpegSource>>,
    /// Replaces the second webcam session when set.
    pub webcam: Option<Box<dyn LiveJpegSource>>,
}

impl LiveVideoInjections {
    /// No injected producers.
    #[must_use]
    pub fn none() -> Self {
        Self {
            external: None,
            webcam: None,
        }
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

enum PumpKind {
    External { index: u32 },
    Webcam { index: u32 },
}

/// Starts live capture for every configured source and records frames in the indexed DVR.
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
    if external.is_none() && webcam.is_none() {
        return Err(A2aLabError::invalid(
            "video",
            "at least one live video source must be configured",
        ));
    }
    let live = MemoryVideos::new();
    let dvr = IndexedVideoDvr::open(disk_root, disk_retention_secs).await?;
    let inner = VideoDvrCatalog::open(live, dvr, Duration::from_secs(60))?;
    if let Some(config) = &external {
        insert_source(
            inner.live(),
            EXTERNAL_CAMERA_SOURCE_ID,
            "External camera",
            config.description,
            EXTERNAL_CAMERA_WIDTH,
            EXTERNAL_CAMERA_HEIGHT,
            fps,
        )
        .await?;
    }
    if webcam.is_some() {
        insert_source(
            inner.live(),
            CAMERA_SOURCE_ID,
            "Camera",
            CAMERA_VIDEO_DESCRIPTION,
            CAMERA_VIDEO_WIDTH,
            CAMERA_VIDEO_HEIGHT,
            fps,
        )
        .await?;
    }
    let maintenance = Some(inner.maintain_disk());
    let (stop, rx) = watch::channel(false);
    let retain_us =
        i64::try_from(live_retention_secs.saturating_mul(1_000_000)).unwrap_or(i64::MAX);
    let frame_period_us = 1_000_000u64 / u64::from(fps.max(1));
    let runtime = tokio::runtime::Handle::current();
    if let Some(config) = external {
        spawn_pump(
            inner.clone(),
            VideoSourceId::new(EXTERNAL_CAMERA_SOURCE_ID)?,
            PumpKind::External {
                index: config.index,
            },
            fps,
            max_bytes,
            injections.external,
            rx.clone(),
            retain_us,
            frame_period_us,
            &runtime,
        );
    }
    if let Some(config) = webcam {
        spawn_pump(
            inner.clone(),
            VideoSourceId::new(CAMERA_SOURCE_ID)?,
            PumpKind::Webcam {
                index: config.index,
            },
            fps,
            max_bytes,
            injections.webcam,
            rx,
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
        LiveVideoInjections {
            external: source,
            webcam: None,
        },
    )
    .await
}

#[allow(clippy::too_many_arguments)]
fn spawn_pump(
    catalog: VideoDvrCatalog,
    source_id: VideoSourceId,
    kind: PumpKind,
    fps: u32,
    max_bytes: u64,
    injected: Option<Box<dyn LiveJpegSource>>,
    stop: watch::Receiver<bool>,
    retain_us: i64,
    frame_period_us: u64,
    runtime: &tokio::runtime::Handle,
) {
    let runtime = runtime.clone();
    std::thread::spawn(move || {
        pump(
            &catalog,
            &source_id,
            &kind,
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
    source_id: &VideoSourceId,
    kind: &PumpKind,
    fps: u32,
    max_bytes: u64,
    mut injected: Option<Box<dyn LiveJpegSource>>,
    mut stop: watch::Receiver<bool>,
    retain_us: i64,
    frame_period_us: u64,
    runtime: &tokio::runtime::Handle,
) {
    let budget = frame_budget(fps);
    let mut native_external: Option<NativeLiveJpeg> = None;
    let mut native_webcam: Option<NativeLiveJpeg> = None;
    let encode_native = matches!(kind, PumpKind::External { .. }) && injected.is_none();
    let mut encoder: Option<H264Encoder> = None;
    let mut encode_stamps = VecDeque::new();
    let mut codec_init = Vec::new();
    while !*stop.borrow() {
        let started = Instant::now();
        let next = match kind {
            PumpKind::External { index: _ } if injected.is_some() => injected.as_mut().map_or_else(
                || Err(A2aLabError::unavailable("external camera is closed")),
                |source| source.next_jpeg(),
            ),
            PumpKind::External { index } => {
                if native_external.is_none() {
                    if let Ok(opened) = NativeLiveJpeg::open(*index, max_bytes) {
                        native_external = Some(opened);
                    } else if mark_down(catalog, source_id, runtime, &mut stop) {
                        return;
                    } else {
                        continue;
                    }
                }
                native_external.as_mut().map_or_else(
                    || Err(A2aLabError::unavailable("external camera is closed")),
                    NativeLiveJpeg::next_frame,
                )
            }
            PumpKind::Webcam { .. } if injected.is_some() => injected.as_mut().map_or_else(
                || Err(A2aLabError::unavailable("camera is closed")),
                |source| source.next_jpeg(),
            ),
            PumpKind::Webcam { index } => {
                if native_webcam.is_none() {
                    match NativeLiveJpeg::open_mjpeg(*index, max_bytes) {
                        Ok(opened) => native_webcam = Some(opened),
                        Err(error) => {
                            eprintln!("camera video: {error}");
                            if mark_down(catalog, source_id, runtime, &mut stop) {
                                return;
                            }
                            continue;
                        }
                    }
                }
                native_webcam.as_mut().map_or_else(
                    || Err(A2aLabError::unavailable("camera is closed")),
                    NativeLiveJpeg::read_jpeg,
                )
            }
        };
        let jpeg = match next {
            Ok(jpeg) => jpeg,
            Err(error) => {
                if matches!(kind, PumpKind::Webcam { .. }) {
                    eprintln!("camera video: {error}");
                    native_webcam = None;
                    let _ = runtime.block_on(catalog.live().set_live_available(source_id, false));
                    if wait(&mut stop, std::time::Duration::from_millis(200)) {
                        return;
                    }
                    continue;
                }
                native_external = None;
                let _ = runtime.block_on(catalog.live().set_live_available(source_id, false));
                if injected.is_some() && wait(&mut stop, std::time::Duration::from_millis(50)) {
                    return;
                }
                continue;
            }
        };
        if jpeg_dimensions(&jpeg).is_err() {
            continue;
        }
        if matches!(kind, PumpKind::Webcam { .. }) {
            remember_camera_frame(jpeg.clone());
        }
        let published = if encode_native {
            if encoder.is_none() {
                match H264Encoder::spawn() {
                    Ok(opened) => encoder = Some(opened),
                    Err(error) => {
                        eprintln!("external camera h264: {error}");
                        native_external = None;
                        let _ =
                            runtime.block_on(catalog.live().set_live_available(source_id, false));
                        if wait(&mut stop, std::time::Duration::from_secs(2)) {
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
                        .block_on(publish_h264(
                            catalog,
                            source_id,
                            jpeg,
                            captured_at,
                            retain_us,
                            frame_period_us,
                            unit.keyframe,
                            &codec_init,
                            &unit.sample,
                        ))
                        .is_err()
                    {
                        return;
                    } else {
                        true
                    }
                }
                Some(Err(error)) => {
                    eprintln!("external camera h264: {error}");
                    encoder = None;
                    encode_stamps.clear();
                    codec_init.clear();
                    false
                }
            }
        } else if runtime
            .block_on(publish(
                catalog,
                source_id,
                jpeg,
                retain_us,
                frame_period_us,
                false,
            ))
            .is_err()
        {
            return;
        } else {
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
}

async fn insert_source(
    live: &MemoryVideos,
    id: &str,
    name: &str,
    description: &str,
    width: u32,
    height: u32,
    fps: u32,
) -> Result<(), A2aLabError> {
    live.insert_source(VideoSource::new(
        VideoSourceId::new(id)?,
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

async fn publish(
    catalog: &VideoDvrCatalog,
    source_id: &VideoSourceId,
    jpeg: Vec<u8>,
    live_retain_us: i64,
    frame_period_us: u64,
    skip_dvr_append: bool,
) -> Result<(), A2aLabError> {
    let stamp = crate::time::now()?;
    catalog
        .live()
        .insert_frame(source_id.clone(), stamp, jpeg.clone())
        .await?;
    catalog
        .live()
        .forget_before(source_id, stamp.saturating_add_micros(-live_retain_us))
        .await?;
    if !skip_dvr_append {
        catalog
            .dvr()
            .append_mjpeg_frame(source_id, stamp, frame_period_us, &jpeg)
            .await?;
    }
    catalog.dvr().cleanup_source(source_id, stamp).await
}

async fn publish_h264(
    catalog: &VideoDvrCatalog,
    source_id: &VideoSourceId,
    jpeg: Vec<u8>,
    captured_at: a2a_lab_dev_kit::UtcTimestamp,
    live_retain_us: i64,
    frame_period_us: u64,
    keyframe: bool,
    codec_init: &[u8],
    sample: &[u8],
) -> Result<(), A2aLabError> {
    catalog
        .live()
        .insert_frame(source_id.clone(), captured_at, jpeg)
        .await?;
    catalog
        .live()
        .forget_before(
            source_id,
            captured_at.saturating_add_micros(-live_retain_us),
        )
        .await?;
    catalog
        .dvr()
        .append_h264_sample(
            source_id,
            captured_at,
            frame_period_us,
            keyframe,
            codec_init,
            sample,
        )
        .await?;
    catalog.dvr().cleanup_source(source_id, captured_at).await
}

fn mark_down(
    catalog: &VideoDvrCatalog,
    source_id: &VideoSourceId,
    runtime: &tokio::runtime::Handle,
    stop: &mut watch::Receiver<bool>,
) -> bool {
    let _ = runtime.block_on(catalog.live().set_live_available(source_id, false));
    wait(stop, std::time::Duration::from_secs(2))
}

fn wait(stop: &mut watch::Receiver<bool>, wait: std::time::Duration) -> bool {
    if *stop.borrow() {
        return true;
    }
    std::thread::sleep(wait);
    *stop.borrow()
}
