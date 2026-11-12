//! [`ColorCaptureSource`] over one negotiated [`CaptureSession`].

use a2a_lab_dev_kit::A2aLabError;
use a2a_lab_dev_kit::VideoSourceId;
use a2a_lab_dev_kit::videos::{
    CaptureCapabilities, CaptureClock, CaptureFormat, CaptureIdentity, CaptureSession,
    CaptureTiming, CapturedColorFrame, ColorCamera, ColorCaptureSource, ColorMatrix,
    ColorProperties, ColorRange, SelectedCaptureProfile,
};

use crate::time;
use crate::videos::capture_open::open_configured_session;
use crate::videos::source::{LiveCameraInputFormat, LiveVideoSourceDescriptor};

/// Drains one open capture session into the dev-kit bounded runner.
pub struct SessionColorCapture<C: ColorCamera> {
    session: CaptureSession<C>,
    source_id: VideoSourceId,
    descriptor: LiveVideoSourceDescriptor,
    sequence: u64,
    capabilities: CaptureCapabilities,
}

impl<C: ColorCamera> SessionColorCapture<C> {
    /// Negotiates and opens `camera` for `descriptor`.
    pub fn open(
        camera: C,
        descriptor: &LiveVideoSourceDescriptor,
        source_id: VideoSourceId,
        output_fps: u32,
    ) -> Result<Self, A2aLabError> {
        let session = open_configured_session(camera, descriptor, output_fps)?;
        let selected = session.selected().ok_or_else(|| {
            A2aLabError::invalid("camera", "capture session has no negotiated profile")
        })?;
        let capabilities = capabilities_from_selected(selected)?;
        Ok(Self {
            session,
            source_id,
            descriptor: descriptor.clone(),
            sequence: 0,
            capabilities,
        })
    }

    /// Mutable access for reconnect and shutdown.
    pub fn session_mut(&mut self) -> &mut CaptureSession<C> {
        &mut self.session
    }
}

impl<C: ColorCamera> ColorCaptureSource for SessionColorCapture<C> {
    fn capabilities(&self) -> &CaptureCapabilities {
        &self.capabilities
    }

    fn pull_frame(&mut self) -> Result<CapturedColorFrame, A2aLabError> {
        let captured_at = time::now()?;
        let identity = CaptureIdentity::new(self.source_id.clone(), self.sequence);
        self.sequence = self.sequence.saturating_add(1);
        let selected = self.session.selected().ok_or_else(|| {
            A2aLabError::invalid("camera", "capture session has no negotiated profile")
        })?;
        let sensor_period_ns = fps_period_ns(selected.capture_rate());
        let monotonic_ns = self.sequence.saturating_mul(sensor_period_ns);
        let timing = CaptureTiming::new(captured_at, CaptureClock::monotonic(monotonic_ns));
        let negotiated = selected.mode().format();
        let color = color_properties(self.descriptor.input);
        let frame = self.session.read(identity, timing, color)?;
        let expected = self.descriptor.input.capture_preference();
        let matches = match expected {
            a2a_lab_dev_kit::videos::CaptureFormatPreference::Mjpeg => {
                negotiated == CaptureFormat::Mjpeg
            }
            a2a_lab_dev_kit::videos::CaptureFormatPreference::Yuy2 => {
                negotiated == CaptureFormat::Yuy2
            }
            a2a_lab_dev_kit::videos::CaptureFormatPreference::Auto => true,
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
        Ok(frame)
    }
}

fn capabilities_from_selected(
    selected: SelectedCaptureProfile,
) -> Result<CaptureCapabilities, A2aLabError> {
    CaptureCapabilities::new(vec![selected.mode()])
}

fn fps_period_ns(rate: a2a_lab_dev_kit::videos::CaptureRate) -> u64 {
    let fps = u64::from(rate.numerator());
    let denom = u64::from(rate.denominator()).max(1);
    1_000_000_000u64
        .saturating_mul(denom)
        .saturating_div(fps.max(1))
}

fn color_properties(input: LiveCameraInputFormat) -> ColorProperties {
    match input {
        LiveCameraInputFormat::Yuy2 => {
            ColorProperties::new(ColorMatrix::Bt601, ColorRange::Limited)
        }
        LiveCameraInputFormat::Auto | LiveCameraInputFormat::Mjpeg => {
            ColorProperties::new(ColorMatrix::Unknown, ColorRange::Unknown)
        }
    }
}
