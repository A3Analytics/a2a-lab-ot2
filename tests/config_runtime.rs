//! File loading through resolved video startup and the recording chat factory.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use a2a_lab_dev_kit::videos::{
    AdvertisedCaptureProfile, CaptureFormat, CaptureMode, CaptureRate, ColorCamera,
    RawCameraSample, SelectedCaptureProfile,
};
use a2a_lab_dev_kit::{
    FollowVideoRequest, GetVideoFrameAtRequest, GetVideoTimelineRequest, ListVideoSourcesRequest,
    PageRequest, VideoCodec, VideoProvider, VideoSourceId,
};
use a2a_lab_ot2::{
    ConversationStore, ExplicitSetting, LabConfigInputs, LiveCameraInputFormat, LiveVideoDelivery,
    LiveVideoInjections, ModelProvider, RecordingServeAgentFactory, ServeChatLegacyInputs,
    build_serve_lab_agent, open_resolved_video_catalog, resolve_lab_config,
    uses_resolved_chat_configuration, uses_resolved_video_configuration,
};

fn temp_root(name: &str) -> PathBuf {
    std::env::temp_dir().join(format!(
        "a2a-lab-ot2-config-runtime-{name}-{}",
        std::process::id()
    ))
}

fn write_config(dir: &Path, document: &str) -> PathBuf {
    fs::create_dir_all(dir).unwrap();
    let path = dir.join("a2a-lab.json");
    fs::write(&path, document).unwrap();
    path
}

fn cli_file(path: PathBuf) -> LabConfigInputs {
    LabConfigInputs {
        config_path: ExplicitSetting::cli(Some(path)),
        ..LabConfigInputs::default()
    }
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

fn profile(format: CaptureFormat, fps: u32) -> AdvertisedCaptureProfile {
    AdvertisedCaptureProfile::new(
        CaptureMode::new(format, 1280, 720).expect("mode"),
        CaptureRate::fps(fps).expect("rate"),
    )
}

fn yuy2_frame(fill: u8) -> RawCameraSample {
    let stride = 2560u32;
    let rows = 720u64 * u64::from(stride);
    RawCameraSample::new(
        CaptureFormat::Yuy2,
        1280,
        720,
        stride,
        vec![fill; usize::try_from(rows).unwrap()],
    )
}

struct RecordingCamera {
    advertised: Vec<AdvertisedCaptureProfile>,
    raw: RawCameraSample,
    negotiated: Option<SelectedCaptureProfile>,
    requested: Arc<Mutex<Option<CaptureFormat>>>,
}

impl ColorCamera for RecordingCamera {
    fn advertised(
        &mut self,
    ) -> Result<Vec<AdvertisedCaptureProfile>, a2a_lab_dev_kit::A2aLabError> {
        Ok(self.advertised.clone())
    }

    fn open(
        &mut self,
        profile: &SelectedCaptureProfile,
    ) -> Result<SelectedCaptureProfile, a2a_lab_dev_kit::A2aLabError> {
        *self.requested.lock().expect("requested format") = Some(profile.mode().format());
        Ok(self.negotiated.unwrap_or(*profile))
    }

    fn raw_frame(&mut self) -> Result<RawCameraSample, a2a_lab_dev_kit::A2aLabError> {
        Ok(self.raw.clone())
    }

    fn stop(&mut self) -> Result<(), a2a_lab_dev_kit::A2aLabError> {
        Ok(())
    }
}

fn camera(
    advertised: Vec<AdvertisedCaptureProfile>,
    raw: RawCameraSample,
    requested: Arc<Mutex<Option<CaptureFormat>>>,
) -> RecordingCamera {
    RecordingCamera {
        advertised,
        raw,
        negotiated: None,
        requested,
    }
}

fn both_formats(fps: u32) -> Vec<AdvertisedCaptureProfile> {
    vec![
        profile(CaptureFormat::Mjpeg, fps),
        profile(CaptureFormat::Yuy2, fps),
    ]
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

async fn assert_jpeg_frame(provider: &a2a_lab_ot2::VideoDvrCatalog, id: &str) {
    let source = VideoSourceId::new(id).unwrap();
    let follow = provider
        .follow_video(FollowVideoRequest::new(source.clone()))
        .await
        .unwrap_or_else(|error| panic!("{id} follow: {error}"));
    assert_eq!(follow.codec(), VideoCodec::Mjpeg, "{id}");
    let timeline = provider
        .get_video_timeline(GetVideoTimelineRequest::new(source.clone()))
        .await
        .unwrap();
    let at = timeline
        .newest_at()
        .unwrap_or_else(|| panic!("{id} has no frame"));
    let frame = provider
        .get_video_frame_at(GetVideoFrameAtRequest::new(source, at, Some(2_000_000)))
        .await
        .unwrap_or_else(|error| panic!("{id} frame: {error}"));
    assert!(frame.image().data().starts_with(&[0xFF, 0xD8]), "{id}");
    assert!(
        frame
            .image()
            .descriptor()
            .media_type()
            .starts_with("image/"),
        "{id}"
    );
}

fn three_source_injections(
    mjpeg_requested: Arc<Mutex<Option<CaptureFormat>>>,
    yuy2_requested: Arc<Mutex<Option<CaptureFormat>>>,
    alias_requested: Arc<Mutex<Option<CaptureFormat>>>,
) -> LiveVideoInjections {
    LiveVideoInjections::none()
        .with_color_camera(
            "deck.mjpeg",
            Box::new(camera(
                both_formats(5),
                RawCameraSample::new(CaptureFormat::Mjpeg, 1280, 720, 0, jpeg(720, 1280)),
                mjpeg_requested,
            )),
        )
        .with_color_camera(
            "deck.yuy2",
            Box::new(camera(both_formats(5), yuy2_frame(8), yuy2_requested)),
        )
        .with_color_camera(
            "deck.alias",
            Box::new(camera(both_formats(5), yuy2_frame(9), alias_requested)),
        )
}

async fn assert_factory_records_openai(inputs: &LabConfigInputs, dir: &Path) {
    let recorder = RecordingServeAgentFactory::new();
    let hook = recorder.clone();
    let store = ConversationStore::open(&dir.join("chat.db"), 1)
        .await
        .unwrap();
    let built = build_serve_lab_agent(
        inputs,
        &legacy_chat(),
        "http://127.0.0.1:9/mcp",
        store,
        move |provider, model, _url, _store| {
            let hook = hook.clone();
            async move {
                hook.record(provider, &model)?;
                Err::<(), _>(a2a_lab_dev_kit::A2aLabError::unavailable(
                    "stop before paid provider",
                ))
            }
        },
    )
    .await;
    assert!(built.is_err());
    assert_eq!(
        recorder.take_recorded().unwrap(),
        Some((ModelProvider::OpenAi, "gpt-test".to_owned()))
    );
}

fn legacy_chat() -> ServeChatLegacyInputs {
    ServeChatLegacyInputs {
        model_provider: ModelProvider::Bedrock,
        model: None,
        bedrock_model: None,
    }
}

#[test]
fn no_file_keeps_legacy_startup_selection() {
    let inputs = LabConfigInputs::default();
    assert!(!uses_resolved_video_configuration(&inputs).unwrap());
    assert!(!uses_resolved_chat_configuration(&inputs).unwrap());
}

#[tokio::test]
async fn three_file_sources_reach_injected_cameras_and_the_chat_factory() {
    let dir = temp_root("three");
    let path = write_config(
        &dir,
        r#"{"schema_version":1,"chat":{"provider":"openai","model":"gpt-test"},"videos":{"sources":[
            {"id":"deck.mjpeg","name":"Deck MJPEG","description":"Encoded input","format":"mjpeg","device_index":0},
            {"id":"deck.yuy2","name":"Deck YUY2","description":"Packed input","format":"yuy2","device_index":1},
            {"id":"deck.alias","name":"Deck alias","description":"YUYV alias","format":"yuyv","device_index":2}
        ],"fps":5,"live_retention":"5s","dvr":{"retention":"1d","storage_path":"./dvr"}}}"#,
    );
    let inputs = cli_file(path);
    let resolved = resolve_lab_config(&inputs).unwrap();
    assert_eq!(resolved.video.sources.len(), 3);
    assert_eq!(
        resolved.video.sources[0].input,
        LiveCameraInputFormat::Mjpeg
    );
    assert_eq!(resolved.video.sources[1].input, LiveCameraInputFormat::Yuy2);
    assert_eq!(resolved.video.sources[2].input, LiveCameraInputFormat::Yuy2);
    assert!(
        resolved
            .video
            .sources
            .iter()
            .all(|source| source.delivery == LiveVideoDelivery::H264)
    );
    assert_eq!(resolved.video.dvr_storage.path, dir.join("dvr"));
    assert_ne!(
        resolved.video.dvr_storage.path,
        std::env::current_dir().unwrap().join("dvr")
    );

    let mjpeg_requested = Arc::new(Mutex::new(None));
    let yuy2_requested = Arc::new(Mutex::new(None));
    let alias_requested = Arc::new(Mutex::new(None));
    let catalog = open_resolved_video_catalog(
        &resolved.video,
        three_source_injections(
            Arc::clone(&mjpeg_requested),
            Arc::clone(&yuy2_requested),
            Arc::clone(&alias_requested),
        ),
    )
    .await
    .unwrap()
    .expect("catalog");
    let provider = catalog.provider();
    wait_for_live(&provider, "deck.mjpeg").await;
    wait_for_live(&provider, "deck.yuy2").await;
    wait_for_live(&provider, "deck.alias").await;
    assert_eq!(*mjpeg_requested.lock().unwrap(), Some(CaptureFormat::Mjpeg));
    assert_eq!(*yuy2_requested.lock().unwrap(), Some(CaptureFormat::Yuy2));
    assert_eq!(*alias_requested.lock().unwrap(), Some(CaptureFormat::Yuy2));
    assert_jpeg_frame(&provider, "deck.yuy2").await;
    assert_jpeg_frame(&provider, "deck.alias").await;
    let listed = provider
        .list_video_sources(ListVideoSourcesRequest::new(page()).unwrap())
        .await
        .unwrap();
    assert_eq!(listed.items().len(), 3);
    catalog.shutdown();
    assert_factory_records_openai(&inputs, &dir).await;
    let _ = fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn precedence_retention_and_empty_sources_match_runtime() {
    let dir = temp_root("precedence");
    let path = write_config(
        &dir,
        r#"{"schema_version":1,"videos":{"sources":[
            {"id":"overview","format":"mjpeg","device_index":3}
        ],"fps":7,"live_retention":"5m","dvr":{"retention":"1h30m","storage_path":"./from-file"}}}"#,
    );
    let inputs = LabConfigInputs {
        video_retention_secs: ExplicitSetting::cli(12),
        video_fps: ExplicitSetting::default_value(5),
        ..cli_file(path)
    };
    let resolved = resolve_lab_config(&inputs).unwrap();
    assert_eq!(resolved.video.fps, 7);
    assert_eq!(resolved.video.live_retention_secs, 12);
    assert_eq!(resolved.video.disk_retention_secs, 5_400);
    let storage = dir.join("from-file");
    assert_eq!(resolved.video.dvr_storage.path, storage);
    assert_ne!(storage, std::env::current_dir().unwrap().join("from-file"));

    let requested = Arc::new(Mutex::new(None));
    let catalog = open_resolved_video_catalog(
        &resolved.video,
        LiveVideoInjections::none().with_color_camera(
            "overview",
            Box::new(camera(
                vec![profile(CaptureFormat::Mjpeg, 7)],
                RawCameraSample::new(CaptureFormat::Mjpeg, 1280, 720, 0, jpeg(720, 1280)),
                Arc::clone(&requested),
            )),
        ),
    )
    .await
    .unwrap()
    .expect("catalog");
    wait_for_live(&catalog.provider(), "overview").await;
    assert_eq!(*requested.lock().unwrap(), Some(CaptureFormat::Mjpeg));
    assert!(storage.is_dir());
    catalog.shutdown();

    let empty = write_config(
        &dir.join("empty"),
        r#"{"schema_version":1,"videos":{"sources":[],"dvr":{"storage_path":"./unused-dvr"}}}"#,
    );
    let empty_resolved = resolve_lab_config(&cli_file(empty)).unwrap();
    assert!(empty_resolved.video.sources.is_empty());
    assert!(
        open_resolved_video_catalog(&empty_resolved.video, LiveVideoInjections::none())
            .await
            .unwrap()
            .is_none()
    );
    assert!(!dir.join("empty").join("unused-dvr").exists());
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn legacy_source_ids_keep_output_delivery() {
    let dir = temp_root("delivery");
    let path = write_config(
        &dir,
        r#"{"schema_version":1,"videos":{"sources":[
            {"id":"external-camera","format":"yuy2","device_index":0},
            {"id":"camera","format":"mjpeg","device_index":1}
        ]}}"#,
    );
    let resolved = resolve_lab_config(&cli_file(path)).unwrap();
    assert_eq!(resolved.video.sources[0].input, LiveCameraInputFormat::Yuy2);
    assert_eq!(resolved.video.sources[0].delivery, LiveVideoDelivery::H264);
    assert_eq!(
        resolved.video.sources[1].input,
        LiveCameraInputFormat::Mjpeg
    );
    assert_eq!(resolved.video.sources[1].delivery, LiveVideoDelivery::H264);
    let _ = fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn mismatched_input_does_not_fall_back_and_isolates_failure() {
    let dir = temp_root("mismatch");
    let path = write_config(
        &dir,
        r#"{"schema_version":1,"videos":{"sources":[
            {"id":"good","format":"mjpeg","device_index":4},
            {"id":"wrong-mode","format":"yuy2","device_index":5},
            {"id":"short-yuy2","format":"yuy2","device_index":6}
        ],"dvr":{"storage_path":"./dvr"}}}"#,
    );
    let resolved = resolve_lab_config(&cli_file(path)).unwrap();
    let wrong_requested = Arc::new(Mutex::new(None));
    let catalog = open_resolved_video_catalog(
        &resolved.video,
        LiveVideoInjections::none()
            .with_color_camera(
                "good",
                Box::new(camera(
                    vec![profile(CaptureFormat::Mjpeg, 5)],
                    RawCameraSample::new(CaptureFormat::Mjpeg, 1280, 720, 0, jpeg(720, 1280)),
                    Arc::new(Mutex::new(None)),
                )),
            )
            .with_color_camera(
                "wrong-mode",
                Box::new(camera(
                    vec![profile(CaptureFormat::Mjpeg, 5)],
                    RawCameraSample::new(CaptureFormat::Mjpeg, 1280, 720, 0, jpeg(720, 1280)),
                    Arc::clone(&wrong_requested),
                )),
            )
            .with_color_camera(
                "short-yuy2",
                Box::new(camera(
                    vec![profile(CaptureFormat::Yuy2, 5)],
                    RawCameraSample::new(CaptureFormat::Yuy2, 1280, 720, 2560, vec![0; 3]),
                    Arc::new(Mutex::new(None)),
                )),
            ),
    )
    .await
    .unwrap()
    .expect("catalog");
    let provider = catalog.provider();
    wait_for_live(&provider, "good").await;
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert!(
        provider
            .follow_video(FollowVideoRequest::new(
                VideoSourceId::new("wrong-mode").unwrap()
            ))
            .await
            .is_err()
    );
    assert!(wrong_requested.lock().unwrap().is_none());
    assert!(
        provider
            .follow_video(FollowVideoRequest::new(
                VideoSourceId::new("short-yuy2").unwrap()
            ))
            .await
            .is_err()
    );
    catalog.shutdown();
    let _ = fs::remove_dir_all(&dir);
}

async fn open_persist(
    video: &a2a_lab_ot2::ResolvedVideoSettings,
    sample: RawCameraSample,
) -> a2a_lab_ot2::LiveVideoCatalog {
    open_resolved_video_catalog(
        video,
        LiveVideoInjections::none().with_color_camera(
            "persist",
            Box::new(camera(
                vec![profile(CaptureFormat::Mjpeg, 5)],
                sample,
                Arc::new(Mutex::new(None)),
            )),
        ),
    )
    .await
    .unwrap()
    .expect("catalog")
}

#[tokio::test]
async fn restart_reuses_dvr_for_the_same_source_id() {
    let dir = temp_root("restart");
    let path = write_config(
        &dir,
        r#"{"schema_version":1,"videos":{"sources":[
            {"id":"persist","name":"Persist","description":"Kept","format":"mjpeg","device_index":8}
        ],"dvr":{"storage_path":"./kept-dvr"}}}"#,
    );
    let inputs = cli_file(path);
    let resolved = resolve_lab_config(&inputs).unwrap();
    let sample = RawCameraSample::new(CaptureFormat::Mjpeg, 1280, 720, 0, jpeg(720, 1280));
    let first = open_persist(&resolved.video, sample.clone()).await;
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
    let second = open_persist(&resolved.video, sample).await;
    let recovered = second
        .provider()
        .get_video_timeline(GetVideoTimelineRequest::new(
            VideoSourceId::new("persist").unwrap(),
        ))
        .await
        .unwrap();
    assert_eq!(recovered.source_id().as_str(), "persist");
    assert!(recovered.newest_at().is_some());
    second.shutdown();
    let _ = fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn malformed_file_fails_before_chat_or_capture_side_effects() {
    let dir = temp_root("malformed");
    let path = write_config(
        &dir,
        r#"{"schema_version":1,"videos":{"sources":[],"live_retention":"1.5s","dvr":{"storage_path":"./should-not-exist"}}}"#,
    );
    let inputs = cli_file(path.clone());
    let resolved = resolve_lab_config(&inputs);
    assert!(resolved.is_err());
    let message = resolved.unwrap_err().to_string();
    assert!(message.contains("videos.live_retention"), "{message}");
    assert!(
        message.contains("must be a positive whole-second duration"),
        "{message}"
    );
    assert!(!message.contains("1.5s"), "{message}");
    assert!(!dir.join("should-not-exist").exists());

    let called = Arc::new(AtomicBool::new(false));
    let flag = Arc::clone(&called);
    let store = ConversationStore::open(&dir.join("unused.db"), 1)
        .await
        .unwrap();
    let built = build_serve_lab_agent(
        &inputs,
        &legacy_chat(),
        "http://127.0.0.1:9/mcp",
        store,
        move |_provider, _model, _url, _store| {
            let flag = Arc::clone(&flag);
            async move {
                flag.store(true, Ordering::SeqCst);
                Ok::<(), a2a_lab_dev_kit::A2aLabError>(())
            }
        },
    )
    .await;
    assert!(built.is_err());
    assert!(!called.load(Ordering::SeqCst));

    let output = Command::new(env!("CARGO_BIN_EXE_a2a-lab-ot2"))
        .env("A2ALAB_CONFIG", path.to_string_lossy().as_ref())
        .env("A2ALAB_OPENTRONS_URL", "http://127.0.0.1:9")
        .env("OPENAI_API_KEY", "sk-test-sentinel")
        .arg("list-tasks")
        .output()
        .unwrap();
    assert!(!output.status.success());
    let err = String::from_utf8_lossy(&output.stderr);
    let out = String::from_utf8_lossy(&output.stdout);
    assert!(err.contains("videos.live_retention"), "{err}");
    assert!(!err.contains("sk-test-sentinel"), "{err}");
    assert!(!out.contains("sk-test-sentinel"), "{out}");
    assert!(!err.to_ascii_lowercase().contains("connection refused"));
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn config_validate_succeeds_without_provider_credentials() {
    let dir = temp_root("validate");
    let path = write_config(
        &dir,
        r#"{"schema_version":1,"chat":{"provider":"anthropic","model":"claude-test"},"videos":{"sources":[
            {"id":"deck.alias","format":"yuyv","device_index":0}
        ],"live_retention":"5m","dvr":{"retention":"3d","storage_path":"./video-dvr"}}}"#,
    );
    let output = Command::new(env!("CARGO_BIN_EXE_a2a-lab-ot2"))
        .env_remove("OPENAI_API_KEY")
        .env_remove("ANTHROPIC_API_KEY")
        .env_remove("AWS_ACCESS_KEY_ID")
        .env_remove("AWS_SECRET_ACCESS_KEY")
        .env_remove("AWS_PROFILE")
        .env("A2ALAB_OPENTRONS_URL", "http://127.0.0.1:9")
        .args(["config", "validate", "--config"])
        .arg(&path)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let out = String::from_utf8_lossy(&output.stdout);
    assert!(out.contains("configuration valid"));
    assert!(!dir.join("video-dvr").exists());
    let _ = fs::remove_dir_all(&dir);
}
