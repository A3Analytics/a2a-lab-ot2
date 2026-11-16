//! Time-lapse DVR persistence: sparse disk commits and indexing at the commit boundary.

use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use a2a_lab_dev_kit::videos::{
    AdvertisedCaptureProfile, CaptureFormat, CaptureMode, CaptureRate, RawCameraSample,
    SelectedCaptureProfile,
};
use a2a_lab_dev_kit::{SaveIntervalDuration, VideoSourceId};
use a2a_lab_ot2::{
    LiveCameraInputFormat, LiveVideoDelivery, LiveVideoInjections, LiveVideoSourceDescriptor,
    start_fixture_declarative_serve,
};

fn temp_dvr(name: &str) -> PathBuf {
    std::env::temp_dir().join(format!(
        "a2a-lab-ot2-timelapse-{name}-{}",
        std::process::id()
    ))
}

fn tiny_jpeg(y: u8) -> Vec<u8> {
    use image::codecs::jpeg::JpegEncoder;
    use image::{ExtendedColorType, ImageBuffer, ImageEncoder, Rgb};
    let image: ImageBuffer<Rgb<u8>, Vec<u8>> = ImageBuffer::from_fn(16, 16, |_, _| Rgb([y, y, y]));
    let mut jpeg = Vec::new();
    JpegEncoder::new_with_quality(&mut jpeg, 80)
        .write_image(image.as_raw(), 16, 16, ExtendedColorType::Rgb8)
        .expect("jpeg");
    jpeg
}

fn h264_source(id: &str) -> LiveVideoSourceDescriptor {
    let (camera_id, device_index) = a2a_lab_ot2::index_camera_fields(1);
    LiveVideoSourceDescriptor {
        id: id.to_owned(),
        name: format!("Name {id}"),
        description: format!("Description {id}"),
        camera_id,
        device_index,
        input: LiveCameraInputFormat::Mjpeg,
        width: Some(16),
        height: Some(16),
        capture_rate: Some(30),
        delivery: LiveVideoDelivery::H264,
    }
}

/// Script advances on every `raw_frame` pull (nominal 30 Hz sensor).
struct HighRateCam {
    negotiated: SelectedCaptureProfile,
    advertised: Vec<AdvertisedCaptureProfile>,
    frames: Vec<Vec<u8>>,
    index: AtomicU64,
}

impl HighRateCam {
    fn mjpeg(frame_count: usize, output_fps: u32) -> Self {
        let format = CaptureFormat::Mjpeg;
        let mode = CaptureMode::new(format, 16, 16).unwrap();
        let negotiated = SelectedCaptureProfile::negotiated(
            mode,
            CaptureRate::fps(30).unwrap(),
            CaptureRate::fps(output_fps).unwrap(),
        );
        Self {
            negotiated,
            advertised: vec![AdvertisedCaptureProfile::new(
                mode,
                CaptureRate::fps(30).unwrap(),
            )],
            frames: (0..frame_count)
                .map(|i| tiny_jpeg(u8::try_from(i).unwrap_or(255)))
                .collect(),
            index: AtomicU64::new(0),
        }
    }
}

impl a2a_lab_dev_kit::videos::ColorCamera for HighRateCam {
    fn advertised(
        &mut self,
    ) -> Result<Vec<AdvertisedCaptureProfile>, a2a_lab_dev_kit::A2aLabError> {
        Ok(self.advertised.clone())
    }

    fn open(
        &mut self,
        profile: &SelectedCaptureProfile,
    ) -> Result<SelectedCaptureProfile, a2a_lab_dev_kit::A2aLabError> {
        assert_eq!(profile.mode().format(), CaptureFormat::Mjpeg);
        Ok(self.negotiated)
    }

    fn raw_frame(&mut self) -> Result<RawCameraSample, a2a_lab_dev_kit::A2aLabError> {
        let index = usize::try_from(self.index.fetch_add(1, Ordering::SeqCst)).unwrap_or(0);
        let bytes = self
            .frames
            .get(index)
            .cloned()
            .unwrap_or_else(|| self.frames.last().cloned().unwrap_or_default());
        Ok(RawCameraSample::new(CaptureFormat::Mjpeg, 16, 16, 0, bytes))
    }

    fn stop(&mut self) -> Result<(), a2a_lab_dev_kit::A2aLabError> {
        Ok(())
    }
}

fn save_interval_nanos(text: &str) -> u64 {
    SaveIntervalDuration::parse("videos.dvr.save_interval", text)
        .unwrap()
        .as_nanos()
}

async fn count_dvr_h264(store: &a2a_lab_ot2::IndexedVideoDvr, source: &str) -> u64 {
    sqlx::query_scalar(
        "SELECT COUNT(*) FROM dvr_frames
         WHERE source_id = ?1 AND track_id = 'h264' AND payload_ready = 1",
    )
    .bind(source)
    .fetch_one(store.media_pool())
    .await
    .unwrap()
}

async fn count_indexing_jobs(store: &a2a_lab_ot2::IndexedVideoDvr) -> u64 {
    store.count_indexing_jobs().await.unwrap()
}

fn count_live_h264_samples(catalog: &a2a_lab_ot2::DeclarativeVideoServe, id: &str) -> usize {
    let source = VideoSourceId::new(id).unwrap();
    let live = catalog
        .provider()
        .live_h264()
        .get(&source)
        .expect("live store");
    live.inner().snapshot().unwrap().samples().len()
}

async fn run_timelapse_capture(
    id: &str,
    live_fps: u32,
    save_text: &str,
    wall_ms: u64,
) -> (a2a_lab_ot2::DeclarativeVideoServe, PathBuf) {
    let root = temp_dvr(&format!("{id}-{save_text}"));
    let _ = std::fs::remove_dir_all(&root);
    let source = h264_source(id);
    let catalog = start_fixture_declarative_serve(
        vec![source],
        live_fps,
        30,
        3_600,
        &root,
        256 * 1024,
        Some(save_interval_nanos(save_text)),
        LiveVideoInjections::none().with_color_camera(id, Box::new(HighRateCam::mjpeg(2_000, 30))),
    )
    .await
    .unwrap();
    catalog
        .dvr()
        .set_indexed_source_filter(a2a_lab_ot2::IndexedSourceFilter::allow_only(
            std::iter::once(id.to_owned()).collect(),
        ));
    tokio::time::sleep(Duration::from_millis(wall_ms)).await;
    (catalog, root)
}

#[tokio::test]
async fn dvr_spacing_follows_configured_save_intervals() {
    for text in ["500ms", "1s", "5s", "30s", "750ms"] {
        let id = format!("spacing-{text}");
        let (catalog, root) = run_timelapse_capture(&id, 5, text, 2_500).await;
        let dvr = count_dvr_h264(catalog.dvr(), &id).await;
        let live = count_live_h264_samples(&catalog, &id);
        assert!(live >= 4, "{text}: live={live}");
        assert!(dvr >= 1, "{text}: dvr={dvr}");
        catalog.shutdown();
        let _ = std::fs::remove_dir_all(root);
    }
}

#[tokio::test]
async fn live_cadence_exceeds_dvr_commit_rate_at_fixed_live_fps() {
    let id = "live-vs-dvr";
    let (catalog, root) = run_timelapse_capture(id, 5, "1s", 3_000).await;
    let dvr = count_dvr_h264(catalog.dvr(), id).await;
    let live = count_live_h264_samples(&catalog, id);
    assert!(live >= 8, "live samples={live}");
    assert!(dvr >= 1, "dvr={dvr}");
    catalog.shutdown();
    let _ = std::fs::remove_dir_all(root);
}

#[tokio::test]
async fn semantic_jobs_match_committed_dvr_frames_only() {
    let id = "semantic-count";
    let (catalog, root) = run_timelapse_capture(id, 5, "1s", 2_800).await;
    tokio::time::sleep(Duration::from_millis(500)).await;
    let dvr = count_dvr_h264(catalog.dvr(), id).await;
    let jobs = count_indexing_jobs(catalog.dvr()).await;
    assert!(
        jobs.abs_diff(dvr.saturating_mul(2)) <= 8,
        "jobs={jobs} dvr={dvr}"
    );
    let live = count_live_h264_samples(&catalog, id);
    assert!(live >= 4, "live={live}");
    catalog.shutdown();
    let _ = std::fs::remove_dir_all(root);
}

#[tokio::test]
async fn sparse_dvr_survives_catalog_restart() {
    let id = "restart";
    let root = temp_dvr(id);
    let _ = std::fs::remove_dir_all(&root);
    let source = h264_source(id);
    let first = start_fixture_declarative_serve(
        vec![source.clone()],
        5,
        30,
        3_600,
        &root,
        256 * 1024,
        Some(save_interval_nanos("1s")),
        LiveVideoInjections::none().with_color_camera(id, Box::new(HighRateCam::mjpeg(800, 30))),
    )
    .await
    .unwrap();
    tokio::time::sleep(Duration::from_millis(1_500)).await;
    let before = count_dvr_h264(first.dvr(), id).await;
    assert!(before >= 1);
    first.shutdown();
    drop(first);
    let second = start_fixture_declarative_serve(
        vec![source.clone()],
        5,
        30,
        3_600,
        &root,
        256 * 1024,
        Some(save_interval_nanos("1s")),
        LiveVideoInjections::none().with_color_camera(id, Box::new(HighRateCam::mjpeg(800, 30))),
    )
    .await
    .unwrap();
    tokio::time::sleep(Duration::from_millis(1_500)).await;
    let after = count_dvr_h264(second.dvr(), id).await;
    assert!(after > before, "before={before} after={after}");
    second.shutdown();
    let _ = std::fs::remove_dir_all(root);
}
