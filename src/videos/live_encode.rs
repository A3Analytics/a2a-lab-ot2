//! Dev-kit color conversion and `OpenH264` for live capture pumps.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use a2a_lab_dev_kit::videos::{
    CadenceDestinations, CadenceDispatch, CaptureFormat, CapturedColorFrame, ColorEncodePipeline,
    ColorEncodedFrame, LiveDvrCadenceSelector,
};
use a2a_lab_dev_kit::{
    A2aLabError, DvrPictureProperties, DvrTrackId, H264LiveAppendStats, H264LiveStore,
    H264LiveStoreLimits, UtcTimestamp, VideoSourceId,
};
use sha2::{Digest, Sha256};

const PIPELINE_QUEUE: usize = 8;
const JPEG_STILL_QUALITY: u8 = 80;

fn encode_bitrate_bps(fps: u32) -> u32 {
    2_000_000u32.saturating_mul(fps.max(1)) / 30
}

fn period_ns_from_fps(fps: u32) -> u64 {
    1_000_000_000u64 / u64::from(fps.max(1))
}

fn live_store_limits(
    retain_secs: u64,
    fps: u32,
    max_sample_bytes: u64,
) -> Result<H264LiveStoreLimits, A2aLabError> {
    let retention_us = retain_secs.saturating_mul(1_000_000);
    let max_samples = usize::try_from(
        retain_secs
            .saturating_mul(u64::from(fps.max(1)))
            .saturating_add(16)
            .max(32),
    )
    .unwrap_or(usize::MAX);
    let max_bytes = max_sample_bytes
        .saturating_mul(u64::try_from(max_samples).unwrap_or(u64::MAX))
        .max(max_sample_bytes)
        .min(512 * 1024 * 1024) as usize;
    H264LiveStoreLimits::new(max_samples, max_bytes, retention_us)
}

/// Per-source bounded live H.264 retention.
#[derive(Clone)]
pub struct SourceLiveH264Store {
    store: Arc<H264LiveStore>,
}

impl SourceLiveH264Store {
    pub fn open(retain_secs: u64, fps: u32, max_sample_bytes: u64) -> Result<Self, A2aLabError> {
        Ok(Self {
            store: Arc::new(H264LiveStore::new(live_store_limits(
                retain_secs,
                fps,
                max_sample_bytes,
            )?)),
        })
    }

    /// Shared store handle for readers and appenders.
    #[must_use]
    pub fn inner(&self) -> &Arc<H264LiveStore> {
        &self.store
    }

    pub fn reconnect(&self) -> Result<(), A2aLabError> {
        self.store.reconnect()
    }
}

/// Shared catalog map of live stores keyed by source id string.
#[derive(Clone, Default)]
pub struct LiveH264Catalog {
    stores: Arc<Mutex<HashMap<String, SourceLiveH264Store>>>,
}

impl LiveH264Catalog {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    pub fn register(
        &self,
        source_id: &VideoSourceId,
        retain_secs: u64,
        fps: u32,
        max_sample_bytes: u64,
    ) -> Result<SourceLiveH264Store, A2aLabError> {
        let entry = SourceLiveH264Store::open(retain_secs, fps, max_sample_bytes)?;
        self.stores
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .insert(source_id.as_str().to_owned(), entry.clone());
        Ok(entry)
    }

    pub fn get(&self, source_id: &VideoSourceId) -> Option<SourceLiveH264Store> {
        self.stores
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get(source_id.as_str())
            .cloned()
    }
}

struct EpochWriter {
    track: Option<DvrTrackId>,
    initialization: Vec<u8>,
    picture: Option<DvrPictureProperties>,
}

impl EpochWriter {
    const fn new() -> Self {
        Self {
            track: None,
            initialization: Vec::new(),
            picture: None,
        }
    }

    fn ensure(
        &mut self,
        source_id: &VideoSourceId,
        encoded: &ColorEncodedFrame,
    ) -> Result<(DvrTrackId, DvrPictureProperties), A2aLabError> {
        let init = encoded.initialization();
        if let (Some(track), Some(picture)) = (&self.track, self.picture)
            && self.initialization == init
        {
            return Ok((track.clone(), picture));
        }
        let picture =
            DvrPictureProperties::new(encoded.original().width(), encoded.original().height())?;
        let digest = Sha256::digest(init);
        let mut hex = String::with_capacity(16);
        for byte in &digest[..8] {
            use std::fmt::Write;
            let _ = write!(hex, "{byte:02x}");
        }
        let track = DvrTrackId::new(format!("h264-live-{}-{hex}", source_id.as_str()))?;
        self.track = Some(track.clone());
        self.initialization = init.to_vec();
        self.picture = Some(picture);
        Ok((track, picture))
    }
}

/// One MJPEG or YUY2 source encoding through `OpenH264` with independent live and DVR cadence.
pub struct LiveColorEncodePump {
    pipeline: ColorEncodePipeline,
    selector: LiveDvrCadenceSelector,
    epoch: EpochWriter,
    live: SourceLiveH264Store,
    source_id: VideoSourceId,
}

impl LiveColorEncodePump {
    pub fn open(
        source_id: VideoSourceId,
        live: SourceLiveH264Store,
        fps: u32,
        dvr_save_interval_nanos: Option<u64>,
    ) -> Result<Self, A2aLabError> {
        let live_period_ns = period_ns_from_fps(fps);
        let dvr_period_ns = dvr_save_interval_nanos.unwrap_or(live_period_ns);
        Ok(Self {
            pipeline: ColorEncodePipeline::at_5fps(
                encode_bitrate_bps(fps),
                PIPELINE_QUEUE,
                Some(JPEG_STILL_QUALITY),
            )?,
            selector: LiveDvrCadenceSelector::new(live_period_ns, dvr_period_ns, PIPELINE_QUEUE)?,
            epoch: EpochWriter::new(),
            live,
            source_id,
        })
    }

    pub fn source_lost(&mut self) {
        self.pipeline.source_lost();
    }

    pub fn reconnect(&mut self) {
        self.pipeline.reconnect();
        let _ = self.live.reconnect();
    }

    pub fn cancel(&mut self) {
        self.pipeline.cancel();
    }

    pub fn push_frame(
        &mut self,
        frame: CapturedColorFrame,
    ) -> Result<Vec<PublishedH264>, A2aLabError> {
        let dispatches = self.selector.push(frame)?;
        let mut out = Vec::with_capacity(dispatches.len());
        for dispatch in dispatches {
            if let Some(published) = self.publish_dispatch(dispatch)? {
                out.push(published);
            }
        }
        Ok(out)
    }

    fn publish_dispatch(
        &mut self,
        dispatch: CadenceDispatch,
    ) -> Result<Option<PublishedH264>, A2aLabError> {
        let destinations = dispatch.destinations();
        let Some(encoded) = self.pipeline.encode_frame(dispatch.into_frame())? else {
            return Ok(None);
        };
        self.publish_encoded(destinations, &encoded)
    }

    fn publish_encoded(
        &mut self,
        destinations: CadenceDestinations,
        encoded: &ColorEncodedFrame,
    ) -> Result<Option<PublishedH264>, A2aLabError> {
        if !destinations.live() && !destinations.dvr() {
            return Ok(None);
        }
        let captured_at = encoded.original().timing().captured_at();
        let (track, picture) = self.epoch.ensure(&self.source_id, encoded)?;
        if destinations.live() {
            if self
                .live
                .inner()
                .snapshot()?
                .epochs()
                .iter()
                .all(|epoch| epoch.track_id() != &track)
            {
                let init = encoded.initialization().to_vec();
                self.live.inner().open_epoch(track.clone(), init, picture)?;
            }
            let _stats: H264LiveAppendStats = self.live.inner().append_sample(
                &track,
                captured_at,
                encoded.sample().to_vec(),
                encoded.keyframe(),
            )?;
        }
        let jpeg = match encoded.original().format() {
            CaptureFormat::Mjpeg => encoded.original().bytes().to_vec(),
            CaptureFormat::Yuy2 => encoded.jpeg().map(std::vec::Vec::from).unwrap_or_default(),
        };
        Ok(Some(PublishedH264 {
            captured_at,
            destinations,
            initialization: encoded.initialization().to_vec(),
            sample: encoded.sample().to_vec(),
            keyframe: encoded.keyframe(),
            still_jpeg: jpeg,
        }))
    }
}

/// One encoded picture and where it should be delivered.
#[derive(Debug, Clone)]
pub struct PublishedH264 {
    pub captured_at: UtcTimestamp,
    pub destinations: CadenceDestinations,
    pub initialization: Vec<u8>,
    pub sample: Vec<u8>,
    pub keyframe: bool,
    pub still_jpeg: Vec<u8>,
}

impl PublishedH264 {
    pub const fn fans_to_disk(&self) -> bool {
        self.destinations.dvr()
    }
}

#[cfg(test)]
mod tests {
    use a2a_lab_dev_kit::videos::{
        CaptureClock, CaptureIdentity, CaptureTiming, CapturedColorFrame, ColorMatrix,
        ColorProperties, ColorRange, LiveDvrCadenceSelector,
    };
    use a2a_lab_dev_kit::{UtcTimestamp, VideoSourceId};

    fn sensor_frame(sequence: u64, monotonic_ns: u64) -> CapturedColorFrame {
        CapturedColorFrame::yuy2(
            CaptureIdentity::new(VideoSourceId::new("cam").unwrap(), sequence),
            CaptureTiming::new(
                UtcTimestamp::parse("2020-01-01T00:00:00Z").unwrap(),
                CaptureClock::monotonic(monotonic_ns),
            ),
            2,
            2,
            4,
            &[16, 128, 16, 128, 16, 128, 16, 128],
            ColorProperties::new(ColorMatrix::Bt709, ColorRange::Limited),
            None,
        )
        .unwrap()
    }

    #[test]
    fn selector_keeps_dvr_spread_on_thirty_fps_sensor_timeline() {
        const SENSOR_PERIOD: u64 = 33_333_333;
        let mut selector =
            LiveDvrCadenceSelector::new(200_000_000, 1_000_000_000, 16).expect("selector");
        let mut live = 0_u32;
        let mut dvr = 0_u32;
        for sequence in 0..120_u64 {
            let mono = sequence.saturating_mul(SENSOR_PERIOD);
            let dispatches = selector.push(sensor_frame(sequence, mono)).expect("push");
            for dispatch in dispatches {
                if dispatch.destinations().live() {
                    live += 1;
                }
                if dispatch.destinations().dvr() {
                    dvr += 1;
                }
            }
        }
        assert!(live > dvr, "live={live} dvr={dvr}");
        assert!(dvr <= 5, "live={live} dvr={dvr}");
    }

    #[test]
    fn selector_burst_of_thirty_two_sensor_frames() {
        const SENSOR_PERIOD: u64 = 33_333_333;
        let mut selector =
            LiveDvrCadenceSelector::new(200_000_000, 1_000_000_000, 16).expect("selector");
        let mut dvr = 0_u32;
        for sequence in 0..32_u64 {
            let dispatches = selector
                .push(sensor_frame(
                    sequence,
                    sequence.saturating_mul(SENSOR_PERIOD),
                ))
                .expect("push");
            for dispatch in dispatches {
                if dispatch.destinations().dvr() {
                    dvr += 1;
                }
            }
        }
        assert!(dvr <= 2, "dvr={dvr}");
    }
}
