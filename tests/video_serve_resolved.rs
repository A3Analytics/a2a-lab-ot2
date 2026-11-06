//! Resolved configuration wired into serve-mode video catalog startup.

use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;

use a2a_lab_dev_kit::LoadedLabConfig;
use a2a_lab_dev_kit::videos::{
    AdvertisedCaptureProfile, CaptureFormat, CaptureMode, CaptureRate, ColorCamera,
    RawCameraSample, SelectedCaptureProfile,
};
use a2a_lab_dev_kit::{
    FollowVideoRequest, GetVideoTimelineRequest, ListVideoSourcesRequest, PageRequest,
    VideoProvider, VideoSourceId,
};
use a2a_lab_ot2::{
    ClapValueSource, ExplicitSetting, LabConfigInputs, LiveCameraInputFormat, LiveVideoInjections,
    open_resolved_video_catalog, resolve_lab_config, uses_resolved_video_configuration,
};

fn temp_root(name: &str) -> PathBuf {
    std::env::temp_dir().join(format!(
        "a2a-lab-ot2-serve-resolved-{name}-{}",
        std::process::id()
    ))
}

fn load_config(document: &str, path: &Path) -> LoadedLabConfig {
    fs::write(path, document).unwrap();
    LoadedLabConfig::load(path).unwrap()
}

fn page() -> PageRequest {
    PageRequest::new(None, 20).unwrap()
}

fn jpeg(width: u16, height: u16) -> Vec<u8> {
    let mut data = vec![0xFF, 0xD8, 0xFF, 0xC0, 0x00, 0x0B, 0x08];
    data.extend_from_slice(&height.to_be_bytes());
    data.extend_from_slice(&width.to_be_bytes());
    data.extend_from_slice(&[0x01, 0x01, 0x11, 0x00, 0xFF, 0xD9]);
    data
}

struct ScriptCamera {
    formats: Vec<AdvertisedCaptureProfile>,
    raw: RawCameraSample,
    negotiated: SelectedCaptureProfile,
}

impl ColorCamera for ScriptCamera {
    fn advertised(
        &mut self,
    ) -> Result<Vec<AdvertisedCaptureProfile>, a2a_lab_dev_kit::A2aLabError> {
        Ok(self.formats.clone())
    }

    fn open(
        &mut self,
        profile: &SelectedCaptureProfile,
    ) -> Result<SelectedCaptureProfile, a2a_lab_dev_kit::A2aLabError> {
        assert_eq!(profile.mode().format(), self.negotiated.mode().format());
        Ok(self.negotiated)
    }

    fn raw_frame(&mut self) -> Result<RawCameraSample, a2a_lab_dev_kit::A2aLabError> {
        Ok(self.raw.clone())
    }

    fn stop(&mut self) -> Result<(), a2a_lab_dev_kit::A2aLabError> {
        Ok(())
    }
}

fn profile(format: CaptureFormat, width: u32, height: u32, fps: u32) -> AdvertisedCaptureProfile {
    AdvertisedCaptureProfile::new(
        CaptureMode::new(format, width, height).expect("mode"),
        CaptureRate::fps(fps).expect("rate"),
    )
}

async fn wait_for_live(provider: &a2a_lab_ot2::VideoDvrCatalog, id: &str) {
    let source = VideoSourceId::new(id).unwrap();
    for _ in 0..50 {
        if provider
            .follow_video(FollowVideoRequest::new(source.clone()))
            .await
            .is_ok()
        {
            return;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    panic!("live follow never became available for {id}");
}

#[test]
fn legacy_defaults_skip_resolved_video_path() {
    let inputs = LabConfigInputs::default();
    assert!(!uses_resolved_video_configuration(&inputs).unwrap());
}

#[tokio::test]
async fn videos_only_file_controls_sources_and_timing() {
    let dir = temp_root("file-only");
    fs::create_dir_all(&dir).unwrap();
    let path = dir.join("a2a-lab.json");
    let loaded = load_config(
        r#"{"schema_version":1,"videos":{"sources":[
            {"id":"bench","name":"Bench","description":"Overview","format":"yuyv","device_index":4},
            {"id":"detail","name":"Detail","description":"Close-up","format":"mjpeg","device_index":6}
        ],"fps":7,"live_retention":"11s","dvr":{"retention":"2d","storage_path":"./dvr-store"}}}"#,
        &path,
    );
    let inputs = LabConfigInputs {
        config_path: ExplicitSetting {
            value: Some(path),
            source: Some(ClapValueSource::CommandLine),
        },
        ..LabConfigInputs::default()
    };
    let resolved = resolve_lab_config(&inputs).unwrap();
    assert_eq!(resolved.video.fps, 7);
    assert_eq!(resolved.video.live_retention_secs, 11);
    assert_eq!(resolved.video.disk_retention_secs, 172_800);
    assert_eq!(resolved.video.sources.len(), 2);
    assert_eq!(resolved.video.sources[0].input, LiveCameraInputFormat::Yuy2);
    assert_eq!(resolved.video.dvr_storage.path, dir.join("dvr-store"));

    let frame = jpeg(720, 1280);
    let negotiated_mjpeg = SelectedCaptureProfile::negotiated(
        CaptureMode::new(CaptureFormat::Mjpeg, 1280, 720).expect("mode"),
        CaptureRate::fps(7).expect("rate"),
        CaptureRate::fps(7).expect("out"),
    );
    let negotiated_yuy2 = SelectedCaptureProfile::negotiated(
        CaptureMode::new(CaptureFormat::Yuy2, 1280, 720).expect("mode"),
        CaptureRate::fps(7).expect("rate"),
        CaptureRate::fps(7).expect("out"),
    );
    let catalog = open_resolved_video_catalog(
        &resolved.video,
        LiveVideoInjections::none()
            .with_color_camera(
                "bench",
                Box::new(ScriptCamera {
                    formats: vec![profile(CaptureFormat::Yuy2, 1280, 720, 7)],
                    raw: RawCameraSample::new(
                        CaptureFormat::Yuy2,
                        1280,
                        720,
                        2560,
                        vec![9; 2560 * 720],
                    ),
                    negotiated: negotiated_yuy2,
                }),
            )
            .with_color_camera(
                "detail",
                Box::new(ScriptCamera {
                    formats: vec![profile(CaptureFormat::Mjpeg, 1280, 720, 7)],
                    raw: RawCameraSample::new(CaptureFormat::Mjpeg, 1280, 720, 0, frame),
                    negotiated: negotiated_mjpeg,
                }),
            ),
    )
    .await
    .unwrap()
    .expect("catalog");
    let provider = catalog.provider();
    wait_for_live(&provider, "bench").await;
    wait_for_live(&provider, "detail").await;
    let page = provider
        .list_video_sources(ListVideoSourcesRequest::new(page()).unwrap())
        .await
        .unwrap();
    assert_eq!(page.items().len(), 2);
    let bench = page
        .items()
        .iter()
        .find(|item| item.id().as_str() == "bench")
        .unwrap();
    assert_eq!(bench.name(), "Bench");
    assert_eq!(bench.description(), "Overview");
    catalog.shutdown();
    let _ = loaded;
}

#[tokio::test]
async fn empty_file_sources_stay_off_without_index_override() {
    let dir = temp_root("empty");
    fs::create_dir_all(&dir).unwrap();
    let path = dir.join("a2a-lab.json");
    load_config(r#"{"schema_version":1,"videos":{"sources":[]}}"#, &path);
    let inputs = LabConfigInputs {
        config_path: ExplicitSetting::cli(Some(path)),
        ..LabConfigInputs::default()
    };
    let resolved = resolve_lab_config(&inputs).unwrap();
    assert!(resolved.video.sources.is_empty());
    assert!(
        open_resolved_video_catalog(&resolved.video, LiveVideoInjections::none())
            .await
            .unwrap()
            .is_none()
    );
}

#[tokio::test]
async fn explicit_camera_index_enables_one_source_from_empty_file_list() {
    let dir = temp_root("enable");
    fs::create_dir_all(&dir).unwrap();
    let path = dir.join("a2a-lab.json");
    load_config(
        r#"{"schema_version":1,"videos":{"sources":[
            {"id":"camera","format":"yuy2","device_index":1}
        ]}}"#,
        &path,
    );
    let inputs = LabConfigInputs {
        config_path: ExplicitSetting::cli(Some(path)),
        camera_index: ExplicitSetting::cli(Some(9)),
        ..LabConfigInputs::default()
    };
    let resolved = resolve_lab_config(&inputs).unwrap();
    assert_eq!(resolved.video.sources.len(), 1);
    assert_eq!(resolved.video.sources[0].device_index, 9);
    assert_eq!(resolved.video.sources[0].input, LiveCameraInputFormat::Yuy2);
}

#[tokio::test]
async fn restart_keeps_timeline_for_same_ids_and_dvr_root() {
    let dir = temp_root("restart");
    let dvr = dir.join("dvr");
    fs::create_dir_all(&dir).unwrap();
    let path = dir.join("a2a-lab.json");
    let storage = serde_json::to_string(&dvr.to_string_lossy()).unwrap();
    load_config(
        &format!(
            r#"{{"schema_version":1,"videos":{{"sources":[
                {{"id":"persist","name":"Persist","description":"DVR","format":"mjpeg","device_index":2}}
            ],"dvr":{{"storage_path":{storage}}}}}}}"#
        ),
        &path,
    );
    let inputs = LabConfigInputs {
        config_path: ExplicitSetting::cli(Some(path)),
        ..LabConfigInputs::default()
    };
    let resolved = resolve_lab_config(&inputs).unwrap();
    let frame = jpeg(720, 1280);
    let negotiated = SelectedCaptureProfile::negotiated(
        CaptureMode::new(CaptureFormat::Mjpeg, 1280, 720).expect("mode"),
        CaptureRate::fps(5).expect("rate"),
        CaptureRate::fps(5).expect("out"),
    );
    let camera = || ScriptCamera {
        formats: vec![profile(CaptureFormat::Mjpeg, 1280, 720, 5)],
        raw: RawCameraSample::new(CaptureFormat::Mjpeg, 1280, 720, 0, frame.clone()),
        negotiated,
    };
    let injections = LiveVideoInjections::none().with_color_camera("persist", Box::new(camera()));
    let first = open_resolved_video_catalog(&resolved.video, injections)
        .await
        .unwrap()
        .expect("first catalog");
    wait_for_live(&first.provider(), "persist").await;
    let timeline = first
        .provider()
        .get_video_timeline(GetVideoTimelineRequest::new(
            VideoSourceId::new("persist").unwrap(),
        ))
        .await
        .unwrap();
    assert!(timeline.newest_at().is_some());
    first.shutdown();
    tokio::time::sleep(Duration::from_millis(50)).await;

    let second = open_resolved_video_catalog(
        &resolved.video,
        LiveVideoInjections::none().with_color_camera("persist", Box::new(camera())),
    )
    .await
    .unwrap()
    .expect("second catalog");
    let recovered = second
        .provider()
        .get_video_timeline(GetVideoTimelineRequest::new(
            VideoSourceId::new("persist").unwrap(),
        ))
        .await
        .unwrap();
    assert!(recovered.newest_at().is_some());
    second.shutdown();
}
