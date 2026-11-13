//! Declarative runtime on the serve boundary: still cache and capture settings.

use std::time::Duration;

use a2a_lab_dev_kit::videos::WarmStillCache;
use a2a_lab_dev_kit::videos::{
    AdvertisedCaptureProfile, CaptureFormat, CaptureMode, CaptureRate, ColorCamera,
    RawCameraSample, SelectedCaptureProfile,
};
use a2a_lab_dev_kit::{
    A2aLabService, A2aServer, CAPTURE_SETTINGS_PATH, DvrVideoProvider, MemoryLogs, MemoryMetrics,
    MemoryTasks, VideoSourceId,
};
use a2a_lab_ot2::camera::PumpLinkedStill;
use a2a_lab_ot2::{
    ExplicitSetting, ImageCapture, LabConfigInputs, LiveVideoInjections,
    open_resolved_video_catalog, resolve_lab_config,
};

fn jpeg() -> Vec<u8> {
    use image::codecs::jpeg::JpegEncoder;
    use image::{ExtendedColorType, ImageBuffer, ImageEncoder, Rgb};
    let image: ImageBuffer<Rgb<u8>, Vec<u8>> = ImageBuffer::from_fn(4, 4, |_, _| Rgb([55, 55, 55]));
    let mut out = Vec::new();
    JpegEncoder::new_with_quality(&mut out, 80)
        .write_image(image.as_raw(), 4, 4, ExtendedColorType::Rgb8)
        .expect("jpeg");
    out
}

struct StillCamera;

impl ColorCamera for StillCamera {
    fn advertised(
        &mut self,
    ) -> Result<Vec<AdvertisedCaptureProfile>, a2a_lab_dev_kit::A2aLabError> {
        Ok(vec![AdvertisedCaptureProfile::new(
            CaptureMode::new(CaptureFormat::Mjpeg, 4, 4).expect("mode"),
            CaptureRate::fps(5).expect("rate"),
        )])
    }

    fn open(
        &mut self,
        selected: &SelectedCaptureProfile,
    ) -> Result<SelectedCaptureProfile, a2a_lab_dev_kit::A2aLabError> {
        Ok(*selected)
    }

    fn raw_frame(&mut self) -> Result<RawCameraSample, a2a_lab_dev_kit::A2aLabError> {
        Ok(RawCameraSample::new(CaptureFormat::Mjpeg, 4, 4, 0, jpeg()))
    }

    fn stop(&mut self) -> Result<(), a2a_lab_dev_kit::A2aLabError> {
        Ok(())
    }
}

async fn wait_for_still(provider: &DvrVideoProvider, still_cache: &WarmStillCache, id: &str) {
    let source = VideoSourceId::new(id).unwrap();
    for _ in 0..200 {
        if provider.live_h264().get(&source).is_some() && still_cache.latest(id).is_some() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    panic!("live store and still cache never became available for {id}");
}

#[tokio::test]
async fn declarative_still_cache_serves_configured_source() {
    let dir = std::env::temp_dir().join(format!(
        "a2a-lab-ot2-declarative-still-{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("a2a-lab.json");
    std::fs::write(
        &path,
        r#"{"schema_version":1,"videos":{"sources":[
            {"id":"bench","format":"mjpeg","width":4,"height":4,"capture_rate":5,"device_index":0}
        ],"dvr":{"storage_path":"./dvr"}}}"#,
    )
    .unwrap();
    let inputs = LabConfigInputs {
        config_path: ExplicitSetting::cli(Some(path)),
        ..LabConfigInputs::default()
    };
    let resolved = resolve_lab_config(&inputs).unwrap();
    let catalog = open_resolved_video_catalog(
        &resolved.video,
        LiveVideoInjections::none().with_color_camera("bench", Box::new(StillCamera)),
    )
    .await
    .unwrap()
    .expect("runtime");
    let provider = catalog.provider();
    wait_for_still(&provider, catalog.still_cache(), "bench").await;
    let still =
        PumpLinkedStill::new("bench", "Bench", 1024, catalog.still_cache().clone()).unwrap();
    let frame = still.capture().await.unwrap();
    assert!(frame.data().starts_with(&[0xFF, 0xD8]));
    catalog.shutdown();
    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn declarative_runtime_exposes_capture_settings_on_a2a() {
    let dir = std::env::temp_dir().join(format!(
        "a2a-lab-ot2-declarative-settings-{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("a2a-lab.json");
    std::fs::write(
        &path,
        r#"{"schema_version":1,"videos":{"sources":[
            {"id":"bench","format":"mjpeg","width":4,"height":4,"capture_rate":5,"device_index":0}
        ],"fps":5,"dvr":{"storage_path":"./dvr"}}}"#,
    )
    .unwrap();
    let inputs = LabConfigInputs {
        config_path: ExplicitSetting::cli(Some(path)),
        ..LabConfigInputs::default()
    };
    let resolved = resolve_lab_config(&inputs).unwrap();
    let runtime = open_resolved_video_catalog(
        &resolved.video,
        LiveVideoInjections::none().with_color_camera("bench", Box::new(StillCamera)),
    )
    .await
    .unwrap()
    .expect("runtime");
    let service = A2aLabService::new(MemoryLogs::new(), MemoryMetrics::new(), MemoryTasks::new())
        .with_videos(runtime.provider())
        .share();
    let (listener, addr) = a2a_lab_dev_kit::bind_local().await.unwrap();
    let server = A2aServer::new(&service)
        .with_web_ui()
        .with_capture_settings(runtime.settings_handle());
    tokio::spawn(async move {
        server.listen(listener).await.unwrap();
    });
    tokio::time::sleep(Duration::from_millis(30)).await;
    let client = reqwest::Client::new();
    let response = client
        .get(format!("http://{addr}{CAPTURE_SETTINGS_PATH}"))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 200);
    runtime.shutdown();
    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn absent_source_still_reports_unavailable() {
    let still = PumpLinkedStill::new(
        "missing",
        "Missing",
        1024,
        a2a_lab_dev_kit::videos::WarmStillCache::new(),
    )
    .unwrap();
    let error = still.capture().await.unwrap_err().to_string();
    assert!(error.contains("missing"));
    assert!(error.contains("no frame yet"));
}
