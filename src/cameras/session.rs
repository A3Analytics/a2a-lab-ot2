//! One device pipeline shared by stills, live color, discovery, and frame sets.

use std::collections::VecDeque;
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

use crate::frame_sets::{
    CaptureRequest, CapturedFrameSet, FrameSetCapture, FrameSetError, FrameSetSource,
};

/// Device-advertised capture rate for synchronized streams.
pub const TARGET_FPS: u32 = 5;

/// Bounded recent frames retained for concurrent consumers.
const FRAME_LIMIT: usize = 8;

/// Opens one streaming device and yields synchronized sets from that open.
pub trait StreamingDevice: Send + Sync {
    /// Advertised source. The first call may open the device.
    fn describe(&self) -> Result<FrameSetSource, FrameSetError>;

    /// Next synchronized set from the already opened pipeline.
    fn next(&self, request: &CaptureRequest) -> Result<CapturedFrameSet, FrameSetError>;
}

/// Interval where a stereo-capable source delivered color only.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StereoGap {
    /// Device timestamp when color-only frames began.
    pub started_us: u64,
    /// Device timestamp when stereo resumed.
    pub ended_us: Option<u64>,
}

struct Inner {
    source: Option<FrameSetSource>,
    frames: VecDeque<Arc<CapturedFrameSet>>,
    gaps: Vec<StereoGap>,
    pipeline_starts: u64,
    shutdown: bool,
}

/// Shared capture owner. Stills, live follow, discovery, and frame sets wait on it.
pub struct SharedSession {
    device: Arc<dyn StreamingDevice>,
    inner: Mutex<Inner>,
    ready: Condvar,
}

impl SharedSession {
    /// Does not open the device until the first describe or capture.
    #[must_use]
    pub fn new(device: Arc<dyn StreamingDevice>) -> Arc<Self> {
        Arc::new(Self {
            device,
            inner: Mutex::new(Inner {
                source: None,
                frames: VecDeque::new(),
                gaps: Vec::new(),
                pipeline_starts: 0,
                shutdown: false,
            }),
            ready: Condvar::new(),
        })
    }

    /// How many times this session opened a device pipeline.
    #[must_use]
    pub fn pipeline_starts(&self) -> u64 {
        self.lock().pipeline_starts
    }

    /// Recorded color-only intervals on a stereo-capable source.
    #[must_use]
    pub fn stereo_gaps(&self) -> Vec<StereoGap> {
        self.lock().gaps.clone()
    }

    /// Stops waiters.
    pub fn shutdown(&self) {
        self.lock().shutdown = true;
        self.ready.notify_all();
    }

    /// Pulls one set from the device and publishes it to every consumer.
    pub fn pump_once(
        &self,
        request: &CaptureRequest,
    ) -> Result<Arc<CapturedFrameSet>, FrameSetError> {
        let source = self.ensure_source()?;
        let captured = self.device.next(request)?;
        Ok(self.publish(&source, captured))
    }

    /// Latest color JPEG, waiting up to `timeout`.
    pub fn color_jpeg(&self, timeout: Duration) -> Result<Vec<u8>, FrameSetError> {
        self.wait(timeout, |inner| {
            inner
                .frames
                .back()
                .and_then(|set| set.color_jpeg().map(Vec::from))
        })
    }

    /// Latest synchronized set, waiting up to `timeout`.
    pub fn wait_set(&self, timeout: Duration) -> Result<Arc<CapturedFrameSet>, FrameSetError> {
        self.wait(timeout, |inner| inner.frames.back().cloned())
    }

    fn ensure_source(&self) -> Result<FrameSetSource, FrameSetError> {
        if let Some(source) = self.lock().source.clone() {
            return Ok(source);
        }
        let source = self.device.describe()?;
        let mut inner = self.lock();
        if inner.source.is_none() {
            inner.pipeline_starts = inner.pipeline_starts.saturating_add(1);
            inner.source = Some(source.clone());
        }
        inner
            .source
            .clone()
            .ok_or_else(|| FrameSetError::Unavailable("shared session lost its source".to_owned()))
    }

    fn publish(
        &self,
        source: &FrameSetSource,
        captured: CapturedFrameSet,
    ) -> Arc<CapturedFrameSet> {
        let timestamp = captured.timestamp_us();
        let stereo = captured.has_stereo();
        let advertised_stereo = source_has_stereo(source);
        let mut inner = self.lock();
        if advertised_stereo && !stereo {
            if inner.gaps.last().is_none_or(|gap| gap.ended_us.is_some()) {
                inner.gaps.push(StereoGap {
                    started_us: timestamp,
                    ended_us: None,
                });
            }
        } else if advertised_stereo
            && stereo
            && let Some(gap) = inner.gaps.last_mut()
            && gap.ended_us.is_none()
        {
            gap.ended_us = Some(timestamp);
        }
        let frame = Arc::new(captured);
        if inner.frames.len() >= FRAME_LIMIT {
            inner.frames.pop_front();
        }
        inner.frames.push_back(Arc::clone(&frame));
        self.ready.notify_all();
        frame
    }

    fn wait<T>(
        &self,
        timeout: Duration,
        mut select: impl FnMut(&Inner) -> Option<T>,
    ) -> Result<T, FrameSetError> {
        let deadline = Instant::now() + timeout;
        let mut inner = self.lock();
        loop {
            if let Some(value) = select(&inner) {
                return Ok(value);
            }
            if inner.shutdown {
                return Err(FrameSetError::Unavailable(
                    "shared capture session stopped".to_owned(),
                ));
            }
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                return Err(self.timeout());
            }
            let (guard, timed_out) = self
                .ready
                .wait_timeout(inner, remaining)
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            inner = guard;
            if timed_out.timed_out() && select(&inner).is_none() {
                return Err(self.timeout());
            }
        }
    }

    fn timeout(&self) -> FrameSetError {
        FrameSetError::Timeout {
            device: self
                .lock()
                .source
                .as_ref()
                .map_or("camera", FrameSetSource::serial)
                .to_owned(),
            detail: "timed out".to_owned(),
        }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Inner> {
        self.inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

/// Background pump that keeps the shared sequence moving.
pub struct SessionPump {
    /// Session used by stills, live color, and frame sets.
    pub session: Arc<SharedSession>,
}

impl SessionPump {
    /// Opens the device from a worker thread at [`TARGET_FPS`].
    #[must_use]
    pub fn start(session: Arc<SharedSession>, max_channel_bytes: u64) -> Self {
        let worker = Arc::clone(&session);
        std::thread::spawn(move || {
            let period = Duration::from_millis(1_000 / u64::from(TARGET_FPS.max(1)));
            loop {
                if worker.lock().shutdown {
                    break;
                }
                match worker.ensure_source() {
                    Ok(source) => {
                        if let Ok(request) = CaptureRequest::new(
                            source.id().clone(),
                            Vec::new(),
                            2_000,
                            max_channel_bytes.max(1),
                        ) {
                            let _ = worker.pump_once(&request);
                        }
                    }
                    Err(_) => std::thread::sleep(period),
                }
                if worker.lock().shutdown {
                    break;
                }
                std::thread::sleep(period);
            }
        });
        Self { session }
    }
}

impl Drop for SessionPump {
    fn drop(&mut self) {
        self.session.shutdown();
    }
}

impl FrameSetCapture for SharedSession {
    fn sources(&self) -> Result<Vec<FrameSetSource>, FrameSetError> {
        Ok(vec![self.ensure_source()?])
    }

    fn capture(&self, request: &CaptureRequest) -> Result<CapturedFrameSet, FrameSetError> {
        Ok((*self.pump_once(request)?).clone())
    }
}

fn source_has_stereo(source: &FrameSetSource) -> bool {
    let roles: Vec<_> = source
        .channels()
        .iter()
        .map(|channel| channel.role().as_str())
        .collect();
    roles.contains(&"depth")
        && roles.contains(&"infrared-left")
        && roles.contains(&"infrared-right")
}

#[cfg(test)]
mod tests {
    use super::super::png::encode_gray;
    use super::*;
    use crate::frame_sets::{
        ChannelCalibration, ChannelProfile, ChannelRole, DepthScale, DepthUnit, FrameChannelId,
        FrameChannelParts, FrameSetSourceId, frame_channel,
    };
    use serde_json::Map;
    use std::sync::atomic::{AtomicU64, Ordering};

    struct Script {
        source: FrameSetSource,
        opens: AtomicU64,
        frames: Mutex<VecDeque<CapturedFrameSet>>,
    }

    impl Script {
        fn new(source: FrameSetSource, frames: Vec<CapturedFrameSet>) -> Arc<Self> {
            Arc::new(Self {
                source,
                opens: AtomicU64::new(0),
                frames: Mutex::new(frames.into()),
            })
        }

        fn opens(&self) -> u64 {
            self.opens.load(Ordering::SeqCst)
        }

        fn open(&self) {
            self.opens.fetch_add(1, Ordering::SeqCst);
        }
    }

    impl StreamingDevice for Script {
        fn describe(&self) -> Result<FrameSetSource, FrameSetError> {
            if self.opens() == 0 {
                self.open();
            }
            Ok(self.source.clone())
        }

        fn next(&self, _request: &CaptureRequest) -> Result<CapturedFrameSet, FrameSetError> {
            if self.opens() == 0 {
                self.open();
            }
            self.frames
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .pop_front()
                .ok_or_else(|| FrameSetError::Timeout {
                    device: self.source.serial().to_owned(),
                    detail: "no scripted frame".to_owned(),
                })
        }
    }

    fn jpeg() -> Vec<u8> {
        vec![0xFF, 0xD8, 0xFF, 0xD9]
    }

    fn png8() -> Vec<u8> {
        encode_gray(&[0], 1, 1, 8).unwrap()
    }

    fn png16() -> Vec<u8> {
        encode_gray(&[0, 1], 1, 1, 16).unwrap()
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

    fn stereo_source() -> FrameSetSource {
        FrameSetSource::new(
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
        .unwrap()
    }

    fn channel(
        id: &str,
        role: ChannelRole,
        media: &str,
        depth: u8,
        data: Vec<u8>,
        timestamp: u64,
        scale: Option<DepthScale>,
    ) -> crate::frame_sets::FrameChannel {
        frame_channel(FrameChannelParts {
            id: FrameChannelId::new(id).unwrap(),
            role,
            media_type: media.to_owned(),
            width: 1,
            height: 1,
            bit_depth: depth,
            device_timestamp_us: timestamp,
            frame_number: 1,
            depth: scale,
            calibration: ChannelCalibration::default(),
            attributes: Map::new(),
            data,
        })
    }

    fn color_only(source: &FrameSetSource, timestamp: u64) -> CapturedFrameSet {
        CapturedFrameSet::new(
            source,
            "2026-11-05T00:00:00Z",
            vec![channel(
                "color",
                ChannelRole::color(),
                "image/jpeg",
                8,
                jpeg(),
                timestamp,
                None,
            )],
            &[FrameChannelId::new("color").unwrap()],
            Map::new(),
            1024,
        )
        .unwrap()
    }

    fn stereo_set(source: &FrameSetSource, timestamp: u64) -> CapturedFrameSet {
        let scale = DepthScale::new(1.0, DepthUnit::Millimeter).unwrap();
        CapturedFrameSet::new(
            source,
            "2026-11-05T00:00:00Z",
            vec![
                channel(
                    "color",
                    ChannelRole::color(),
                    "image/jpeg",
                    8,
                    jpeg(),
                    timestamp,
                    None,
                ),
                channel(
                    "depth",
                    ChannelRole::depth(),
                    "image/png",
                    16,
                    png16(),
                    timestamp,
                    Some(scale),
                ),
                channel(
                    "infrared-left",
                    ChannelRole::infrared_left(),
                    "image/png",
                    8,
                    png8(),
                    timestamp,
                    None,
                ),
                channel(
                    "infrared-right",
                    ChannelRole::infrared_right(),
                    "image/png",
                    8,
                    png8(),
                    timestamp,
                    None,
                ),
            ],
            &[],
            Map::new(),
            4096,
        )
        .unwrap()
    }

    fn request(source: &FrameSetSource) -> CaptureRequest {
        CaptureRequest::new(source.id().clone(), Vec::new(), 1_000, 4096).unwrap()
    }

    #[test]
    fn shared_consumers_use_one_pipeline() {
        let source = stereo_source();
        let frame = stereo_set(&source, 1_000);
        let device = Script::new(source.clone(), vec![frame.clone(), frame]);
        let session = SharedSession::new(device.clone());
        let req = request(&source);
        assert_eq!(session.sources().unwrap()[0].serial(), "ABC");
        assert_eq!(
            session.capture(&req).unwrap().color_jpeg(),
            Some(jpeg().as_slice())
        );
        assert_eq!(
            session.color_jpeg(Duration::from_millis(1)).unwrap(),
            jpeg()
        );
        assert_eq!(session.pipeline_starts(), 1);
        assert_eq!(device.opens(), 1);
    }

    #[test]
    fn rgb_fallback_records_stereo_gap() {
        let source = stereo_source();
        let device = Script::new(
            source.clone(),
            vec![color_only(&source, 1_000), stereo_set(&source, 2_000)],
        );
        let session = SharedSession::new(device);
        let color_req = CaptureRequest::new(
            source.id().clone(),
            vec![FrameChannelId::new("color").unwrap()],
            1_000,
            4096,
        )
        .unwrap();
        session.pump_once(&color_req).unwrap();
        let gaps = session.stereo_gaps();
        assert_eq!(gaps.len(), 1);
        assert_eq!(gaps[0].started_us, 1_000);
        assert_eq!(gaps[0].ended_us, None);
        session.pump_once(&request(&source)).unwrap();
        let gaps = session.stereo_gaps();
        assert_eq!(gaps[0].ended_us, Some(2_000));
    }
}
