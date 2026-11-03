//! Image-source contract shared by the OT-2, native, and Orbbec cameras.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use a2a_lab_dev_kit::{
    A2aClient, A2aLabApi, A2aLabCommand, A2aLabError, A2aLabResult, A2aLabService, A2aServer,
    GetCurrentImageRequest, GetImageRequest, Image, ImageId, ImageProvider, ImageSourceId,
    ListImageSourcesRequest, ListImagesRequest, McpLab, McpServer, PageRequest,
    SearchImagesRequest, bind_local,
};
use a2a_lab_ot2::camera::{
    CameraCapture, CameraCaptureError, EXTERNAL_CAMERA_SOURCE_ID, ExternalCamera,
    external_camera_source,
};
use a2a_lab_ot2::opentrons::{
    OPENTRONS_CAMERA_SOURCE_ID, OpentronsCamera, opentrons_camera_source,
};
use a2a_lab_ot2::orbbec::{
    ORBBEC_CAMERA_SOURCE_ID, OrbbecCamera, OrbbecCapture, OrbbecCaptureError, orbbec_camera_source,
};
use a2a_lab_ot2::{ImageCatalogConfig, LiveImageCatalog, OpentronsLab};
use axum::Router;
use axum::body::Body;
use axum::extract::State;
use axum::http::{HeaderValue, StatusCode, header};
use axum::response::Response;
use axum::routing::post;
use tokio::net::TcpListener;

const DIFFERENCES: &str = "
intentional image-source differences:
- opentrons-camera reads pinned robot-server POST /camera/picture. The operator enables that camera on the OT-2. Its description is the on-board camera sentence and its asset id is opentrons-ot2.
- external-camera reads a native camera index through the host camera backend. Its id is external-camera and its description is the configured still-frame sentence.
- orbbec-camera reads one MJPEG color frame through OrbbecSDK. It is absent until Orbbec color capture is enabled. A serial selects the device; omitting it uses the sole connected device and multiple devices are an ambiguity. Its id is orbbec-camera.
- diagnostic text names the transport that failed. The shared result is the error code class, JPEG descriptor fields, stored bytes, and history behavior.
- depth, infrared, stereo pairs, point clouds, and synchronization are outside this comparison.
";

struct Endpoints {
    a2a: A2aClient,
    mcp: McpLab,
}

#[derive(Clone)]
enum Shot {
    Jpeg(Vec<u8>),
    Empty,
    Garbage,
    Delay,
}

#[derive(Clone)]
struct Picture(Arc<Mutex<Shot>>);

async fn robot(shot: Shot) -> String {
    let picture = Picture(Arc::new(Mutex::new(shot)));
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let app = Router::new()
        .route("/camera/picture", post(picture_handler))
        .with_state(picture);
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    format!("http://{address}")
}

async fn picture_handler(State(picture): State<Picture>) -> Response {
    let shot = picture.0.lock().expect("shot").clone();
    match shot {
        Shot::Delay => {
            tokio::time::sleep(Duration::from_millis(300)).await;
            jpeg_response(jpeg(1, 1))
        }
        Shot::Jpeg(bytes) => jpeg_response(bytes),
        Shot::Empty => jpeg_response(Vec::new()),
        Shot::Garbage => jpeg_response(b"not-a-jpeg".to_vec()),
    }
}

fn jpeg_response(bytes: Vec<u8>) -> Response {
    let mut response = Response::builder()
        .status(StatusCode::OK)
        .body(Body::from(bytes))
        .unwrap();
    response
        .headers_mut()
        .insert(header::CONTENT_TYPE, HeaderValue::from_static("image/jpeg"));
    response
}

fn jpeg(width: u16, height: u16) -> Vec<u8> {
    let mut data = vec![0xFF, 0xD8, 0xFF, 0xC0, 0x00, 0x0B, 0x08];
    data.extend_from_slice(&height.to_be_bytes());
    data.extend_from_slice(&width.to_be_bytes());
    data.extend_from_slice(&[0x01, 0x01, 0x11, 0x00, 0xFF, 0xD9]);
    data
}

#[derive(Clone)]
struct Native(Arc<Mutex<Result<Vec<u8>, CameraCaptureError>>>);

impl CameraCapture for Native {
    fn capture(
        &self,
        _index: u32,
        _format: a2a_lab_ot2::camera::ExternalCameraFormat,
        _frames: u32,
    ) -> Result<Vec<u8>, CameraCaptureError> {
        self.0.lock().expect("native").clone()
    }
}

#[derive(Clone)]
struct Orb(Arc<Mutex<Result<Vec<u8>, OrbbecCaptureError>>>);

impl OrbbecCapture for Orb {
    fn capture_color(
        &self,
        _serial: Option<&str>,
        _timeout: Duration,
        _max_bytes: u64,
    ) -> Result<Vec<u8>, OrbbecCaptureError> {
        self.0.lock().expect("orbbec").clone()
    }
}

struct Slow;

impl OrbbecCapture for Slow {
    fn capture_color(
        &self,
        _serial: Option<&str>,
        _timeout: Duration,
        _max_bytes: u64,
    ) -> Result<Vec<u8>, OrbbecCaptureError> {
        std::thread::sleep(Duration::from_millis(200));
        Ok(jpeg(1, 1))
    }
}

struct SlowNative;

impl CameraCapture for SlowNative {
    fn capture(
        &self,
        _index: u32,
        _format: a2a_lab_ot2::camera::ExternalCameraFormat,
        _frames: u32,
    ) -> Result<Vec<u8>, CameraCaptureError> {
        std::thread::sleep(Duration::from_millis(200));
        Ok(jpeg(1, 1))
    }
}

fn native_ok(bytes: Vec<u8>) -> Native {
    Native(Arc::new(Mutex::new(Ok(bytes))))
}

fn native_err(error: CameraCaptureError) -> Native {
    Native(Arc::new(Mutex::new(Err(error))))
}

fn orb_ok(bytes: Vec<u8>) -> Orb {
    Orb(Arc::new(Mutex::new(Ok(bytes))))
}

fn orb_err(error: OrbbecCaptureError) -> Orb {
    Orb(Arc::new(Mutex::new(Err(error))))
}

fn catalog(
    url: &str,
    retention: u64,
    max_bytes: u64,
    timeout: Duration,
    native: impl CameraCapture + 'static,
    orbbec: impl OrbbecCapture + 'static,
) -> LiveImageCatalog {
    LiveImageCatalog::new(
        vec![
            (
                opentrons_camera_source().unwrap(),
                Arc::new(OpentronsCamera::new(url, timeout, max_bytes).unwrap()),
            ),
            (
                external_camera_source().unwrap(),
                Arc::new(ExternalCamera::with_capture(4, timeout, max_bytes, native).unwrap()),
            ),
            (
                orbbec_camera_source().unwrap(),
                Arc::new(
                    OrbbecCamera::with_capture(Some("CAM"), timeout, max_bytes, orbbec).unwrap(),
                ),
            ),
        ],
        ImageCatalogConfig::new(retention, max_bytes).unwrap(),
    )
    .unwrap()
}

async fn serve(images: LiveImageCatalog, max_bytes: u64, readonly: bool) -> Endpoints {
    let transport = a2a_lab_dev_kit::ImageTransportConfig::new(max_bytes).unwrap();
    let lab = OpentronsLab::new("http://127.0.0.1:9")
        .unwrap()
        .with_readonly(readonly);
    let service = A2aLabService::new(lab.clone(), lab.clone(), lab)
        .with_images(images)
        .with_image_transport(transport)
        .share();
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let mcp = McpServer::new(&service);
    tokio::spawn(async move {
        mcp.serve_http(listener).await.unwrap();
    });
    let mcp_lab = McpLab::connect_with(&format!("http://{address}/mcp"), transport)
        .await
        .unwrap();
    let api: Arc<dyn A2aLabApi> = Arc::new(mcp_lab);
    let (a2a_listener, a2a_address) = bind_local().await.unwrap();
    let server = A2aServer::new(&api);
    tokio::spawn(async move {
        server.listen(a2a_listener).await.unwrap();
    });
    tokio::time::sleep(Duration::from_millis(20)).await;
    Endpoints {
        a2a: A2aClient::new(&format!("http://{a2a_address}"))
            .unwrap()
            .with_image_transport(transport),
        mcp: McpLab::connect_with(&format!("http://{address}/mcp"), transport)
            .await
            .unwrap(),
    }
}

fn source(id: &str) -> ImageSourceId {
    ImageSourceId::new(id).unwrap()
}

fn page() -> PageRequest {
    PageRequest::new(None, 10).unwrap()
}

fn mismatch(source_id: &str, operation: &str, case: &str, left: &str, right: &str) -> ! {
    panic!(
        "image contract mismatch\nsource: {source_id}\noperation: {operation}\ncase: {case}\nA2A: {left}\nMCP: {right}"
    );
}

async fn current(endpoints: &Endpoints, id: &str, case: &str) -> Image {
    let request = GetCurrentImageRequest::new(source(id));
    let a2a = endpoints
        .a2a
        .get_current_image(request.clone())
        .await
        .map_err(|error| format!("{} {}", error.code(), error));
    let mcp = mcp_image(
        endpoints
            .mcp
            .execute(A2aLabCommand::GetCurrentImage(request))
            .await,
    );
    same_image(id, "get_current_image", case, a2a, mcp)
}

fn same_image(
    source_id: &str,
    operation: &str,
    case: &str,
    a2a: Result<Image, String>,
    mcp: Result<Image, String>,
) -> Image {
    match (a2a, mcp) {
        (Ok(left), Ok(right)) => {
            let same = left.descriptor().source_id() == right.descriptor().source_id()
                && left.descriptor().media_type() == right.descriptor().media_type()
                && left.descriptor().width() == right.descriptor().width()
                && left.descriptor().height() == right.descriptor().height()
                && left.data() == right.data();
            if !same {
                mismatch(
                    source_id,
                    operation,
                    case,
                    &format!("{left:?}"),
                    &format!("{right:?}"),
                );
            }
            left
        }
        (Err(left), Err(right)) => {
            let left_code = left.split_whitespace().next().unwrap_or("");
            let right_code = right.split_whitespace().next().unwrap_or("");
            if left_code != right_code {
                mismatch(source_id, operation, case, &left, &right);
            }
            panic!("expected an image for {source_id} {operation} {case}: {left}");
        }
        (left, right) => mismatch(
            source_id,
            operation,
            case,
            &format!("{left:?}"),
            &format!("{right:?}"),
        ),
    }
}

async fn current_error(endpoints: &Endpoints, id: &str, case: &str) -> String {
    let request = GetCurrentImageRequest::new(source(id));
    let a2a = endpoints
        .a2a
        .get_current_image(request.clone())
        .await
        .err()
        .map_or_else(
            || "ok".to_owned(),
            |error| format!("{} {error}", error.code()),
        );
    let mcp = match endpoints
        .mcp
        .execute(A2aLabCommand::GetCurrentImage(request))
        .await
    {
        Ok(outcome) => format!("ok {outcome:?}"),
        Err(error) => format!("{} {error}", error.code()),
    };
    let left = a2a.split_whitespace().next().unwrap_or("");
    let right = mcp.split_whitespace().next().unwrap_or("");
    if left != right || left == "ok" {
        mismatch(id, "get_current_image", case, &a2a, &mcp);
    }
    a2a
}

fn mcp_image(result: Result<a2a_lab_dev_kit::A2aLabOutcome, A2aLabError>) -> Result<Image, String> {
    match result {
        Ok(outcome) => match outcome.task.result {
            A2aLabResult::GetCurrentImage(image) | A2aLabResult::GetImage(image) => Ok(image),
            other => Err(format!("protocol {other:?}")),
        },
        Err(error) => Err(format!("{} {error}", error.code())),
    }
}

#[test]
fn intentional_differences_are_recorded() {
    for phrase in [
        "POST /camera/picture",
        "opentrons-ot2",
        "native camera index",
        "external-camera",
        "OrbbecSDK",
        "orbbec-camera",
        "serial",
        "operator enables",
        "diagnostic text",
        "depth, infrared, stereo",
    ] {
        assert!(DIFFERENCES.contains(phrase), "{phrase}");
    }
}

#[tokio::test]
async fn image_source_contract_matrix() {
    let frame = jpeg(6, 7);
    let url = robot(Shot::Jpeg(frame.clone())).await;
    let images = catalog(
        &url,
        4,
        1024,
        Duration::from_millis(500),
        native_ok(frame.clone()),
        orb_ok(frame.clone()),
    );
    let endpoints = serve(images, 1024, false).await;
    let card = endpoints.a2a.agent_card().await.unwrap();
    let skills: Vec<_> = card.skills.iter().map(|skill| skill.id.as_str()).collect();
    assert!(
        !skills.iter().any(|skill| skill.contains("orbbec")),
        "{skills:?}"
    );

    let a2a_sources = endpoints
        .a2a
        .list_image_sources(ListImageSourcesRequest::new(page()).unwrap())
        .await
        .unwrap();
    let mcp_sources = endpoints
        .mcp
        .execute(A2aLabCommand::ListImageSources(
            ListImageSourcesRequest::new(page()).unwrap(),
        ))
        .await
        .unwrap();
    let A2aLabResult::ListImageSources(mcp_sources) = mcp_sources.task.result else {
        panic!("list_image_sources");
    };
    if a2a_sources != mcp_sources {
        mismatch(
            "all",
            "list_image_sources",
            "discovery",
            &format!("{a2a_sources:?}"),
            &format!("{mcp_sources:?}"),
        );
    }
    let mut ids: Vec<_> = a2a_sources
        .items()
        .iter()
        .map(|item| item.id.as_str())
        .collect();
    ids.sort_unstable();
    assert_eq!(
        ids,
        vec![
            EXTERNAL_CAMERA_SOURCE_ID,
            OPENTRONS_CAMERA_SOURCE_ID,
            ORBBEC_CAMERA_SOURCE_ID
        ]
    );

    for id in [
        OPENTRONS_CAMERA_SOURCE_ID,
        EXTERNAL_CAMERA_SOURCE_ID,
        ORBBEC_CAMERA_SOURCE_ID,
    ] {
        assert_success(&endpoints, id).await;
    }

    let unknown = current_error(&endpoints, "missing-camera", "unknown source").await;
    assert!(unknown.starts_with("not_found"), "{unknown}");
    let missing = endpoints
        .a2a
        .get_image(GetImageRequest::new(ImageId::new("img-missing").unwrap()))
        .await
        .unwrap_err();
    let missing_mcp = endpoints
        .mcp
        .execute(A2aLabCommand::GetImage(GetImageRequest::new(
            ImageId::new("img-missing").unwrap(),
        )))
        .await
        .unwrap_err();
    if missing.code() != missing_mcp.code() {
        mismatch(
            "all",
            "get_image",
            "unknown image",
            &missing.to_string(),
            &missing_mcp.to_string(),
        );
    }
    assert_eq!(missing.code(), "not_found");
}

#[tokio::test]
async fn shared_failures_keep_the_same_code_and_no_partial_row() {
    let cases = [
        (
            "empty",
            Shot::Empty,
            native_ok(Vec::new()),
            orb_ok(Vec::new()),
            "invalid",
        ),
        (
            "malformed",
            Shot::Garbage,
            native_ok(b"not-a-jpeg".to_vec()),
            orb_ok(b"not-a-jpeg".to_vec()),
            "invalid",
        ),
        (
            "timeout",
            Shot::Delay,
            native_err(CameraCaptureError::Backend("timed out".to_owned())),
            orb_err(OrbbecCaptureError::Timeout),
            "unavailable",
        ),
    ];
    for (case, shot, native, orbbec, code) in cases {
        let url = robot(shot).await;
        let images = catalog(&url, 4, 1024, Duration::from_millis(40), native, orbbec);
        let endpoints = serve(images, 1024, false).await;
        let mut texts = Vec::new();
        for id in [
            OPENTRONS_CAMERA_SOURCE_ID,
            EXTERNAL_CAMERA_SOURCE_ID,
            ORBBEC_CAMERA_SOURCE_ID,
        ] {
            let error = current_error(&endpoints, id, case).await;
            assert!(error.starts_with(code), "{id} {case}: {error}");
            let listed = endpoints
                .a2a
                .list_images(ListImagesRequest::new(source(id), page()).unwrap())
                .await
                .unwrap();
            assert!(listed.items().is_empty(), "{id} {case}");
            texts.push(error);
        }
        assert!(
            texts
                .iter()
                .any(|text| text.contains("POST /camera/picture") || text.contains("OT-2"))
        );
        assert!(texts.iter().any(|text| text.contains("external camera")));
        assert!(texts.iter().any(|text| text.contains("orbbec-camera")));
    }

    let url = robot(Shot::Jpeg(jpeg(3, 3))).await;
    let images = catalog(
        &url,
        4,
        8,
        Duration::from_millis(500),
        native_ok(jpeg(3, 3)),
        orb_ok(jpeg(3, 3)),
    );
    let endpoints = serve(images, 8, false).await;
    for id in [
        OPENTRONS_CAMERA_SOURCE_ID,
        EXTERNAL_CAMERA_SOURCE_ID,
        ORBBEC_CAMERA_SOURCE_ID,
    ] {
        let error = current_error(&endpoints, id, "maximum payload").await;
        assert!(error.starts_with("invalid"), "{id}: {error}");
        let listed = endpoints
            .a2a
            .list_images(ListImagesRequest::new(source(id), page()).unwrap())
            .await
            .unwrap();
        assert!(listed.items().is_empty(), "{id}");
    }
}

#[tokio::test]
async fn retention_readonly_isolation_and_restart_match() {
    let frame = jpeg(2, 2);
    let url = robot(Shot::Jpeg(frame.clone())).await;
    let images = catalog(
        &url,
        1,
        1024,
        Duration::from_millis(500),
        native_ok(frame.clone()),
        orb_ok(frame.clone()),
    );
    let endpoints = serve(images, 1024, true).await;
    let first = endpoints
        .a2a
        .get_current_image(GetCurrentImageRequest::new(source(ORBBEC_CAMERA_SOURCE_ID)))
        .await
        .unwrap();
    let second = endpoints
        .a2a
        .get_current_image(GetCurrentImageRequest::new(source(ORBBEC_CAMERA_SOURCE_ID)))
        .await
        .unwrap();
    let gone = endpoints
        .a2a
        .get_image(GetImageRequest::new(first.descriptor().id().clone()))
        .await
        .unwrap_err();
    let gone_mcp = endpoints
        .mcp
        .execute(A2aLabCommand::GetImage(GetImageRequest::new(
            first.descriptor().id().clone(),
        )))
        .await
        .unwrap_err();
    assert_eq!(gone.code(), "not_found");
    assert_eq!(gone_mcp.code(), gone.code());
    let kept = endpoints
        .mcp
        .execute(A2aLabCommand::GetImage(GetImageRequest::new(
            second.descriptor().id().clone(),
        )))
        .await
        .unwrap();
    let A2aLabResult::GetImage(kept) = kept.task.result else {
        panic!("get_image");
    };
    assert_eq!(kept.data(), second.data());

    let tasks = endpoints
        .a2a
        .list_tasks(a2a_lab_dev_kit::ListTasksRequest {
            page: PageRequest::new(None, 200).unwrap(),
        })
        .await
        .unwrap();
    assert!(
        tasks
            .items()
            .iter()
            .any(|item| item.id.as_str() == "get_protocols")
    );
    assert!(
        !tasks
            .items()
            .iter()
            .any(|item| item.id.as_str() == "pause_run")
    );

    assert_restart(&url, &frame).await;
}

async fn assert_success(endpoints: &Endpoints, id: &str) {
    let image = current(endpoints, id, "success").await;
    assert_eq!(image.descriptor().media_type(), "image/jpeg");
    assert!(image.descriptor().width() > 0);
    assert!(image.descriptor().height() > 0);
    assert!(!image.data().is_empty());
    let stored = endpoints
        .a2a
        .get_image(GetImageRequest::new(image.descriptor().id().clone()))
        .await
        .unwrap();
    let mcp_stored = mcp_image(
        endpoints
            .mcp
            .execute(A2aLabCommand::GetImage(GetImageRequest::new(
                image.descriptor().id().clone(),
            )))
            .await,
    )
    .unwrap();
    assert_eq!(stored.data(), image.data());
    assert_eq!(mcp_stored.data(), image.data());
    assert_eq!(stored.descriptor(), mcp_stored.descriptor());
    let listed = endpoints
        .a2a
        .list_images(ListImagesRequest::new(source(id), page()).unwrap())
        .await
        .unwrap();
    let searched = endpoints
        .a2a
        .search_images(
            SearchImagesRequest::new(Some(source(id)), None, Some("camera".to_owned()), page())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(listed.items(), searched.items());
    assert!(
        listed
            .items()
            .iter()
            .any(|item| item.id() == image.descriptor().id())
    );
}

async fn assert_restart(url: &str, frame: &[u8]) {
    let isolated = catalog(
        url,
        4,
        1024,
        Duration::from_millis(500),
        native_err(CameraCaptureError::Backend("external down".to_owned())),
        orb_ok(frame.to_vec()),
    );
    let endpoints = serve(isolated, 1024, false).await;
    let error = current_error(&endpoints, EXTERNAL_CAMERA_SOURCE_ID, "isolation").await;
    assert!(error.starts_with("unavailable"), "{error}");
    let orbbec = current(&endpoints, ORBBEC_CAMERA_SOURCE_ID, "isolation orbbec").await;
    assert_eq!(orbbec.data(), frame);
    let robot_frame = current(&endpoints, OPENTRONS_CAMERA_SOURCE_ID, "isolation robot").await;
    assert_eq!(robot_frame.data(), frame);

    let dir =
        std::env::temp_dir().join(format!("a2a-lab-ot2-image-contract-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("images.sqlite");
    let sources = [
        source(OPENTRONS_CAMERA_SOURCE_ID),
        source(EXTERNAL_CAMERA_SOURCE_ID),
        source(ORBBEC_CAMERA_SOURCE_ID),
    ];
    let images = catalog(
        url,
        4,
        1024,
        Duration::from_millis(500),
        native_ok(frame.to_vec()),
        orb_ok(frame.to_vec()),
    )
    .with_persisted_sources(&sources, &path)
    .await
    .unwrap();
    let saved = images
        .get_current_image(GetCurrentImageRequest::new(source(ORBBEC_CAMERA_SOURCE_ID)))
        .await
        .unwrap();
    drop(images);
    let restored = catalog(
        url,
        4,
        1024,
        Duration::from_millis(500),
        native_err(CameraCaptureError::Empty),
        orb_err(OrbbecCaptureError::NoDevice),
    )
    .with_persisted_sources(&sources, &path)
    .await
    .unwrap();
    let loaded = restored
        .get_image(GetImageRequest::new(saved.descriptor().id().clone()))
        .await
        .unwrap();
    assert_eq!(loaded.data(), saved.data());
    std::fs::remove_dir_all(dir).ok();
}

#[tokio::test]
async fn live_timeouts_use_the_capture_budget() {
    let url = robot(Shot::Delay).await;
    let images = catalog(&url, 2, 1024, Duration::from_millis(30), SlowNative, Slow);
    let robot_error = images
        .get_current_image(GetCurrentImageRequest::new(source(
            OPENTRONS_CAMERA_SOURCE_ID,
        )))
        .await
        .unwrap_err();
    let external_error = images
        .get_current_image(GetCurrentImageRequest::new(source(
            EXTERNAL_CAMERA_SOURCE_ID,
        )))
        .await
        .unwrap_err();
    let orbbec_error = images
        .get_current_image(GetCurrentImageRequest::new(source(ORBBEC_CAMERA_SOURCE_ID)))
        .await
        .unwrap_err();
    for error in [robot_error, external_error, orbbec_error] {
        assert_eq!(error.code(), "unavailable", "{error}");
    }
}
