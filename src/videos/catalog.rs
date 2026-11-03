//! Memory video catalog plus a cancellable capture pump.

use std::time::Instant;

use a2a_lab_dev_kit::{A2aLabError, MJPEG_MEDIA_TYPE, MemoryVideos, VideoSource, VideoSourceId};
use tokio::sync::watch;

use crate::camera::{EXTERNAL_CAMERA_HEIGHT, EXTERNAL_CAMERA_SOURCE_ID, EXTERNAL_CAMERA_WIDTH};
use crate::images::jpeg_dimensions;
use crate::videos::external::{LiveJpegSource, NativeLiveJpeg, frame_budget};

/// Live video catalog and the capture thread that fills it.
pub struct LiveVideoCatalog {
    inner: MemoryVideos,
    stop: watch::Sender<bool>,
}

impl LiveVideoCatalog {
    /// Catalog used by A2A/MCP.
    #[must_use]
    pub fn provider(&self) -> MemoryVideos {
        self.inner.clone()
    }

    /// Stops capture threads.
    pub fn shutdown(&self) {
        let _ = self.stop.send(true);
    }
}

impl Drop for LiveVideoCatalog {
    fn drop(&mut self) {
        self.shutdown();
    }
}

/// Starts `external-camera` live capture when an index is configured.
pub async fn start_external_video(
    index: u32,
    description: &str,
    fps: u32,
    retention_secs: u64,
    max_bytes: u64,
    source: Option<Box<dyn LiveJpegSource>>,
) -> Result<LiveVideoCatalog, A2aLabError> {
    let videos = MemoryVideos::new();
    let source_id = VideoSourceId::new(EXTERNAL_CAMERA_SOURCE_ID)?;
    videos
        .insert_source(VideoSource::new(
            source_id.clone(),
            "External camera",
            description,
            None,
            None,
            EXTERNAL_CAMERA_WIDTH,
            EXTERNAL_CAMERA_HEIGHT,
            fps,
            MJPEG_MEDIA_TYPE,
        )?)
        .await?;
    let (stop, rx) = watch::channel(false);
    let retain_us = i64::try_from(retention_secs.saturating_mul(1_000_000)).unwrap_or(i64::MAX);
    let pumped = videos.clone();
    let runtime = tokio::runtime::Handle::current();
    std::thread::spawn(move || {
        pump(
            &pumped, &source_id, index, fps, retain_us, max_bytes, source, rx, &runtime,
        );
    });
    Ok(LiveVideoCatalog {
        inner: videos,
        stop,
    })
}

#[allow(clippy::too_many_arguments)]
fn pump(
    videos: &MemoryVideos,
    source_id: &VideoSourceId,
    index: u32,
    fps: u32,
    retain_us: i64,
    max_bytes: u64,
    injected: Option<Box<dyn LiveJpegSource>>,
    mut stop: watch::Receiver<bool>,
    runtime: &tokio::runtime::Handle,
) {
    let budget = frame_budget(fps);
    let mut native: Option<NativeLiveJpeg> = None;
    let mut injected = injected;
    while !*stop.borrow() {
        let started = Instant::now();
        let next = if let Some(source) = injected.as_mut() {
            source.next_jpeg()
        } else {
            if native.is_none() {
                if let Ok(opened) = NativeLiveJpeg::open(index, max_bytes) {
                    native = Some(opened);
                } else {
                    let _ = runtime.block_on(videos.set_live_available(source_id, false));
                    if wait(&mut stop, std::time::Duration::from_secs(2)) {
                        return;
                    }
                    continue;
                }
            }
            native.as_mut().map_or_else(
                || Err(A2aLabError::unavailable("external camera is closed")),
                NativeLiveJpeg::next_frame,
            )
        };
        let Ok(jpeg) = next else {
            native = None;
            let _ = runtime.block_on(videos.set_live_available(source_id, false));
            if injected.is_some() && wait(&mut stop, std::time::Duration::from_millis(50)) {
                return;
            }
            continue;
        };
        if jpeg_dimensions(&jpeg).is_err() {
            continue;
        }
        if runtime
            .block_on(publish(videos, source_id, jpeg, retain_us))
            .is_err()
        {
            return;
        }
        let _ = runtime.block_on(videos.set_live_available(source_id, true));
        if let Some(rest) = budget.checked_sub(started.elapsed())
            && wait(&mut stop, rest)
        {
            return;
        }
    }
}

async fn publish(
    videos: &MemoryVideos,
    source_id: &VideoSourceId,
    jpeg: Vec<u8>,
    retain_us: i64,
) -> Result<(), A2aLabError> {
    let stamp = crate::time::now()?;
    videos.insert_frame(source_id.clone(), stamp, jpeg).await?;
    videos
        .forget_before(source_id, stamp.saturating_add_micros(-retain_us))
        .await
}

fn wait(stop: &mut watch::Receiver<bool>, wait: std::time::Duration) -> bool {
    if *stop.borrow() {
        return true;
    }
    std::thread::sleep(wait);
    *stop.borrow()
}
