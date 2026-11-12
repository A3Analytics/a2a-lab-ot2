//! Helpers for DVR save cadence on paths without a color frame.

use a2a_lab_dev_kit::videos::{
    CadenceDispatch, CaptureClock, CaptureIdentity, CaptureTiming, CapturedColorFrame, ColorMatrix,
    ColorProperties, ColorRange, LiveDvrCadenceSelector,
};
use a2a_lab_dev_kit::{A2aLabError, UtcTimestamp, VideoSourceId};

const TICK_W: u32 = 2;
const TICK_H: u32 = 2;
const TICK_BYTES: &[u8] = &[16, 128, 16, 128, 16, 128, 16, 128];

/// Independent live and DVR gates for MJPEG pumps that lack a color pipeline.
#[derive(Debug)]
pub struct JpegCadenceSelector {
    inner: LiveDvrCadenceSelector,
    source_id: VideoSourceId,
}

impl JpegCadenceSelector {
    pub fn new(
        source_id: VideoSourceId,
        live_period_ns: u64,
        dvr_period_ns: u64,
        capacity: usize,
    ) -> Result<Self, A2aLabError> {
        Ok(Self {
            inner: LiveDvrCadenceSelector::new(live_period_ns, dvr_period_ns, capacity)?,
            source_id,
        })
    }

    /// One pump tick at `captured_at` with `monotonic_ns`.
    pub fn push_tick(
        &mut self,
        sequence: u64,
        captured_at: UtcTimestamp,
        monotonic_ns: u64,
    ) -> Result<Vec<CadenceDispatch>, A2aLabError> {
        let frame = cadence_tick_frame(&self.source_id, sequence, captured_at, monotonic_ns)?;
        self.inner.push(frame)
    }
}

fn cadence_tick_frame(
    source_id: &VideoSourceId,
    sequence: u64,
    captured_at: UtcTimestamp,
    monotonic_ns: u64,
) -> Result<CapturedColorFrame, A2aLabError> {
    CapturedColorFrame::yuy2(
        CaptureIdentity::new(source_id.clone(), sequence),
        CaptureTiming::new(captured_at, CaptureClock::monotonic(monotonic_ns)),
        TICK_W,
        TICK_H,
        4,
        TICK_BYTES,
        ColorProperties::new(ColorMatrix::Bt709, ColorRange::Limited),
        None,
    )
}
