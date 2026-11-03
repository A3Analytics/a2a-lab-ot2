//! Frame-set tasks, media URLs, and two fake camera backends.

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use a2a_lab_dev_kit::{
    A2aLabError, GetImageRequest, ImageId, ImageProvider, ImageSource, ImageSourceId, JsonObject,
    ListImagesRequest, ListTasksRequest, PageRequest, StartTaskRequest, TaskId, TaskProvider,
    TaskState,
};
use a2a_lab_ot2::frame_set_service::FrameSetApi;
use a2a_lab_ot2::frame_sets::{
    CaptureRequest, CapturedFrameSet, ChannelCalibration, ChannelProfile, ChannelRole, DepthScale,
    DepthUnit, FrameChannelId, FrameChannelParts, FrameSetCapture, FrameSetError, FrameSetSource,
    FrameSetSourceId, frame_channel,
};
use a2a_lab_ot2::images::{CapturedFrame, ImageCapture, ImageCatalogConfig, LiveImageCatalog};
use serde_json::{Value, json};

const CAMERA_SOURCE_ID: &str = "camera";
use tokio::net::TcpListener;

struct Fake {
    source: FrameSetSource,
    captures: Arc<AtomicUsize>,
    class: Option<&'static str>,
}

impl FrameSetCapture for Fake {
    fn sources(&self) -> Result<Vec<FrameSetSource>, FrameSetError> {
        Ok(vec![self.source.clone()])
    }

    fn capture(&self, request: &CaptureRequest) -> Result<CapturedFrameSet, FrameSetError> {
        self.captures.fetch_add(1, Ordering::SeqCst);
        if let Some(class) = self.class {
            return Err(error_class(class));
        }
        if request.channel_ids().iter().any(|id| {
            !self
                .source
                .channels()
                .iter()
                .any(|profile| profile.id() == id)
        }) {
            return Err(FrameSetError::UnsupportedProfile {
                device: self.source.serial().to_owned(),
                detail: "channel is not advertised".to_owned(),
            });
        }
        let channels = if request.channel_ids().is_empty() {
            self.source.channels().iter().map(channel).collect()
        } else {
            request
                .channel_ids()
                .iter()
                .map(|id| {
                    channel(
                        self.source
                            .channels()
                            .iter()
                            .find(|profile| profile.id() == id)
                            .unwrap(),
                    )
                })
                .collect()
        };
        CapturedFrameSet::new(
            &self.source,
            "2026-10-30T16:00:00Z",
            channels,
            request.channel_ids(),
            serde_json::Map::new(),
            request.max_channel_bytes(),
        )
    }
}

struct IdleCamera;

impl ImageCapture for IdleCamera {
    fn capture(
        &self,
    ) -> Pin<Box<dyn Future<Output = Result<CapturedFrame, A2aLabError>> + Send + '_>> {
        Box::pin(async { Err(A2aLabError::unavailable("image capture was not requested")) })
    }
}

fn error_class(class: &str) -> FrameSetError {
    match class {
        "timeout" => FrameSetError::Timeout {
            device: "camera".to_owned(),
            detail: "timed out".to_owned(),
        },
        "busy" => FrameSetError::Busy {
            device: "camera".to_owned(),
            detail: "device is busy".to_owned(),
        },
        "permission" => FrameSetError::Permission {
            device: "camera".to_owned(),
            detail: "permission denied".to_owned(),
        },
        "unsynchronized" => FrameSetError::Unsynchronized("timestamps diverged".to_owned()),
        _ => FrameSetError::Unavailable(class.to_owned()),
    }
}

fn source(id: &str, backend: &str) -> FrameSetSource {
    FrameSetSource::new(
        FrameSetSourceId::new(id).unwrap(),
        backend,
        format!("{backend}-serial"),
        "stereo",
        vec![
            ChannelProfile::new(
                FrameChannelId::new("color").unwrap(),
                ChannelRole::color(),
                "image/jpeg",
                1,
                1,
                8,
                30.0,
            )
            .unwrap(),
            ChannelProfile::new(
                FrameChannelId::new("depth").unwrap(),
                ChannelRole::depth(),
                "image/png",
                1,
                1,
                16,
                30.0,
            )
            .unwrap(),
            ChannelProfile::new(
                FrameChannelId::new("infrared-left").unwrap(),
                ChannelRole::infrared_left(),
                "image/png",
                1,
                1,
                8,
                30.0,
            )
            .unwrap(),
        ],
        format!("device:{backend}-serial"),
        1_000,
    )
    .unwrap()
}

fn channel(profile: &ChannelProfile) -> a2a_lab_ot2::frame_sets::FrameChannel {
    let depth = profile
        .role()
        .is_depth()
        .then(|| DepthScale::new(0.1, DepthUnit::Millimeter).unwrap());
    let data = if profile.role().is_color() {
        vec![0xFF, 0xD8, 0xFF, 0xD9]
    } else {
        png(profile.width(), profile.height(), profile.bit_depth())
    };
    frame_channel(FrameChannelParts {
        id: profile.id().clone(),
        role: profile.role().clone(),
        media_type: profile.media_type().to_owned(),
        width: profile.width(),
        height: profile.height(),
        bit_depth: profile.bit_depth(),
        device_timestamp_us: 50,
        frame_number: 3,
        depth,
        calibration: ChannelCalibration::default(),
        attributes: serde_json::Map::new(),
        data,
    })
}

fn png(width: u32, height: u32, bit_depth: u8) -> Vec<u8> {
    let mut data = b"\x89PNG\r\n\x1a\n".to_vec();
    data.extend_from_slice(&13u32.to_be_bytes());
    data.extend_from_slice(b"IHDR");
    data.extend_from_slice(&width.to_be_bytes());
    data.extend_from_slice(&height.to_be_bytes());
    data.extend_from_slice(&[bit_depth, 0, 0, 0, 0]);
    data.extend_from_slice(&[0, 0, 0, 0]);
    data
}

fn shape(value: &Value) -> Vec<(String, String, u8, Option<String>)> {
    value["channels"]
        .as_array()
        .unwrap()
        .iter()
        .map(|channel| {
            (
                channel["id"].as_str().unwrap().to_owned(),
                channel["media_type"].as_str().unwrap().to_owned(),
                u8::try_from(channel["bit_depth"].as_u64().unwrap()).unwrap(),
                channel["depth"]["units"].as_str().map(str::to_owned),
            )
        })
        .collect()
}

async fn api(fake: Fake, images: Option<LiveImageCatalog>, origin: &str) -> FrameSetApi {
    static NEXT_DB: AtomicUsize = AtomicUsize::new(0);
    let path = std::env::temp_dir().join(format!(
        "frame-service-{}-{}",
        std::process::id(),
        NEXT_DB.fetch_add(1, Ordering::SeqCst)
    ));
    let _ = std::fs::remove_dir_all(&path);
    std::fs::create_dir_all(&path).unwrap();
    FrameSetApi::open(
        &path.join("frames.sqlite"),
        Arc::new(fake),
        4,
        4096,
        origin.to_owned(),
        images,
    )
    .await
    .unwrap()
}

#[tokio::test]
async fn tasks_and_http_share_metadata_and_do_not_embed_pixels() {
    let capture_count = Arc::new(AtomicUsize::new(0));
    let images = LiveImageCatalog::new(
        vec![(
            ImageSource {
                id: ImageSourceId::new(CAMERA_SOURCE_ID).unwrap(),
                name: "Camera".to_owned(),
                description: "Still frame from the configured camera.".to_owned(),
                asset_id: None,
                semantic_id: None,
            },
            Arc::new(IdleCamera),
        )],
        ImageCatalogConfig::new(4, 4096).unwrap(),
    )
    .unwrap();
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let origin = format!("http://{address}");
    let service = Arc::new(
        api(
            Fake {
                source: source(CAMERA_SOURCE_ID, "webcam"),
                captures: Arc::clone(&capture_count),
                class: None,
            },
            Some(images.clone()),
            &origin,
        )
        .await,
    );
    let router = a2a_lab_ot2::frame_set_service::router(Arc::clone(&service));
    tokio::spawn(async move {
        axum::serve(listener, router).await.unwrap();
    });

    let captured = service
        .execute("capture-current", &json!({ "source_id": CAMERA_SOURCE_ID }))
        .await
        .unwrap();
    assert!(captured.get("data").is_none());
    assert_eq!(captured["channel_urls"].as_array().unwrap().len(), 3);
    let color_url = captured["channel_urls"][0]["url"].as_str().unwrap();
    assert!(color_url.starts_with(&origin));
    let bytes = reqwest::get(color_url)
        .await
        .unwrap()
        .bytes()
        .await
        .unwrap();
    assert_eq!(bytes.as_ref(), [0xFF, 0xD8, 0xFF, 0xD9]);
    assert_eq!(capture_count.load(Ordering::SeqCst), 1);

    let listed = service
        .execute(
            "list",
            &json!({ "source_id": CAMERA_SOURCE_ID, "limit": 10 }),
        )
        .await
        .unwrap();
    assert_eq!(listed["frame_sets"].as_array().unwrap().len(), 1);
    assert_eq!(capture_count.load(Ordering::SeqCst), 1);
    assert_task_round_trip(&service, &captured).await;
    assert_color_reused(&images).await;
}

async fn assert_task_round_trip(service: &Arc<FrameSetApi>, captured: &Value) {
    let lab = a2a_lab_ot2::OpentronsLab::new("http://127.0.0.1:9")
        .unwrap()
        .with_frame_sets(Arc::clone(service));
    let tasks = lab
        .list_tasks(ListTasksRequest {
            page: PageRequest::new(None, 1_000).unwrap(),
        })
        .await
        .unwrap();
    assert!(
        tasks
            .items()
            .iter()
            .any(|task| task.id.as_str() == "ot2.frame-sets.capture-current")
    );
    let run = lab
        .start(StartTaskRequest::new(
            TaskId::new("ot2.frame-sets.get").unwrap(),
            JsonObject::try_from_value(json!({ "id": captured["id"] })).unwrap(),
        ))
        .await
        .unwrap();
    assert_eq!(run.state, TaskState::Completed);
    let task_json = serde_json::to_value(run.result.unwrap()).unwrap();
    assert_eq!(task_json["id"], captured["id"]);
    assert_eq!(task_json["channel_urls"], captured["channel_urls"]);
    let hidden = a2a_lab_ot2::OpentronsLab::new("http://127.0.0.1:9").unwrap();
    let hidden_tasks = hidden
        .list_tasks(ListTasksRequest {
            page: PageRequest::new(None, 1_000).unwrap(),
        })
        .await
        .unwrap();
    assert!(
        !hidden_tasks
            .items()
            .iter()
            .any(|task| task.id.as_str().starts_with("ot2.frame-sets."))
    );
}

async fn assert_color_reused(images: &LiveImageCatalog) {
    let images_page = images
        .list_images(
            ListImagesRequest::new(
                ImageSourceId::new(CAMERA_SOURCE_ID).unwrap(),
                PageRequest::new(None, 10).unwrap(),
            )
            .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(images_page.items().len(), 1);
    let image = images
        .get_image(GetImageRequest::new(
            ImageId::new(images_page.items()[0].id().as_str()).unwrap(),
        ))
        .await
        .unwrap();
    assert_eq!(image.data(), [0xFF, 0xD8, 0xFF, 0xD9]);
}

#[tokio::test]
async fn two_fake_backends_share_the_contract_without_realsense() {
    let manifest =
        std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/Cargo.toml")).unwrap();
    let lower = manifest.to_ascii_lowercase();
    assert!(!lower.contains("realsense"));
    assert!(!lower.contains("librealsense"));

    let webcam = api(
        Fake {
            source: source(CAMERA_SOURCE_ID, "webcam"),
            captures: Arc::new(AtomicUsize::new(0)),
            class: None,
        },
        None,
        "http://127.0.0.1:31003",
    )
    .await;
    let realsense = api(
        Fake {
            source: source("realsense-camera", "realsense"),
            captures: Arc::new(AtomicUsize::new(0)),
            class: None,
        },
        None,
        "http://127.0.0.1:31003",
    )
    .await;
    let left = webcam
        .execute("capture-current", &json!({ "source_id": CAMERA_SOURCE_ID }))
        .await
        .unwrap();
    let right = realsense
        .execute(
            "capture-current",
            &json!({ "source_id": "realsense-camera" }),
        )
        .await
        .unwrap();
    assert_eq!(shape(&left), shape(&right));
    assert_ne!(left["source_id"], right["source_id"]);

    for (service, source_id) in [
        (&webcam, CAMERA_SOURCE_ID),
        (&realsense, "realsense-camera"),
    ] {
        let missing = service
            .execute(
                "capture-current",
                &json!({ "source_id": source_id, "channels": ["missing"] }),
            )
            .await
            .unwrap_err();
        assert_eq!(missing.class(), "unsupported_profile");
    }

    let timed_out = api(
        Fake {
            source: source("realsense-camera", "realsense"),
            captures: Arc::new(AtomicUsize::new(0)),
            class: Some("timeout"),
        },
        None,
        "http://127.0.0.1:9",
    )
    .await;
    let error = timed_out
        .execute(
            "capture-current",
            &json!({ "source_id": "realsense-camera" }),
        )
        .await
        .unwrap_err();
    assert_eq!(error.class(), "timeout");
    let lab = a2a_lab_ot2::OpentronsLab::new("http://127.0.0.1:9")
        .unwrap()
        .with_frame_sets(Arc::new(timed_out));
    let run = lab
        .start(StartTaskRequest::new(
            TaskId::new("ot2.frame-sets.capture-current").unwrap(),
            JsonObject::try_from_value(json!({ "source_id": "realsense-camera" })).unwrap(),
        ))
        .await
        .unwrap();
    assert_eq!(run.state, TaskState::Failed);
    assert_eq!(run.error_kind.as_deref(), Some("timeout"));
}
