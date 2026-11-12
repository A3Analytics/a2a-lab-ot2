//! Capacity-one runner polled from the live capture thread.

use std::time::{Duration, Instant};

use a2a_lab_dev_kit::A2aLabError;
use a2a_lab_dev_kit::videos::{
    CaptureDrainRunner, CaptureRunnerPhase, CapturedColorFrame, ColorCamera,
};

use crate::camera::remember_live_still;
use crate::images::jpeg_dimensions;
use crate::videos::raw_jpeg::captured_frame_jpeg;
use crate::videos::session_capture::SessionColorCapture;

/// One opened session drained through the dev-kit latest-frame store.
pub struct NativeCaptureDrain<C: ColorCamera> {
    runner: CaptureDrainRunner<SessionColorCapture<C>>,
    source_id: String,
    max_bytes: u64,
}

impl<C: ColorCamera> NativeCaptureDrain<C> {
    /// Wraps an already-open [`SessionColorCapture`].
    #[must_use]
    pub fn new(source: SessionColorCapture<C>, source_id: &str, max_bytes: u64) -> Self {
        Self {
            runner: CaptureDrainRunner::new(source),
            source_id: source_id.to_owned(),
            max_bytes,
        }
    }

    /// Every frame offered during up to `max_pulls` drain attempts.
    pub fn drain_handoffs(&mut self, max_pulls: u32) -> Vec<CapturedColorFrame> {
        let mut out = Vec::new();
        for _ in 0..max_pulls {
            match self.runner.drain_once_handoff() {
                Some(frame) => {
                    if let Ok(jpeg) = captured_frame_jpeg(&frame, self.max_bytes)
                        && jpeg_dimensions(&jpeg).is_ok()
                    {
                        remember_live_still(&self.source_id, jpeg);
                    }
                    out.push(frame);
                }
                None => break,
            }
        }
        out
    }

    /// Pulls frames until the source has no frame ready or `max_pulls` is reached.
    pub fn drain_pending(&mut self, max_pulls: u32) {
        for _ in 0..max_pulls {
            if !self.runner.drain_once() {
                break;
            }
            self.refresh_still();
        }
    }

    /// Newest retained color frame after one drain pass.
    #[expect(dead_code, reason = "used by legacy pumps and diagnostics")]
    pub fn latest_captured(&mut self) -> Result<CapturedColorFrame, A2aLabError> {
        self.drain_pending(8);
        self.runner
            .peek_latest()
            .cloned()
            .ok_or_else(|| A2aLabError::unavailable("capture has no frame yet"))
    }

    /// Newest retained JPEG for the output-rate pump.
    pub fn latest_jpeg(&mut self) -> Result<Vec<u8>, A2aLabError> {
        self.drain_pending(1);
        let frame = self
            .runner
            .peek_latest()
            .ok_or_else(|| A2aLabError::unavailable("capture has no frame yet"))?;
        captured_frame_jpeg(frame, self.max_bytes)
    }

    /// Runner phase after the latest drain.
    #[must_use]
    pub fn phase(&self) -> CaptureRunnerPhase {
        self.runner.phase()
    }

    /// Frames waiting for a slow consumer. Never more than one.
    #[must_use]
    pub fn queue_depth(&self) -> usize {
        self.runner.queue_depth()
    }

    /// Clears stale retention and reopens the underlying session after a source fault.
    pub fn reconnect(&mut self) -> Result<(), A2aLabError> {
        self.runner.reconnect();
        self.runner.source_mut().session_mut().reconnect()?;
        Ok(())
    }

    /// Stops draining and closes the device stream.
    pub fn shutdown(&mut self) {
        let _ = self.runner.source_mut().session_mut().stop();
        let _ = self.runner.shutdown();
    }

    /// Drains while waiting so the sensor stays current.
    pub fn wait_polling(&mut self, duration: Duration) {
        let deadline = Instant::now() + duration;
        while Instant::now() < deadline {
            self.drain_pending(8);
            std::thread::sleep(Duration::from_millis(1));
        }
    }

    fn refresh_still(&mut self) {
        let Some(frame) = self.runner.peek_latest() else {
            return;
        };
        if let Ok(jpeg) = captured_frame_jpeg(frame, self.max_bytes)
            && jpeg_dimensions(&jpeg).is_ok()
        {
            remember_live_still(&self.source_id, jpeg);
        }
    }
}
