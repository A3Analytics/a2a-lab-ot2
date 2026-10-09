//! Serve-mode image configuration and both camera sources.

use std::process::Command;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use a2a_lab_dev_kit::{
    A2aClient, A2aLabApi, A2aLabCommand, A2aLabError, A2aLabResult, A2aLabService, A2aServer,
    AgentMessageFuture, AgentMessageHandler, AgentMessageReply, AgentMessageRequest,
    GetCurrentImageRequest, Image, ImageProvider, ImageSourceId, JsonObject,
    ListImageSourcesRequest, ListTasksRequest, McpLab, McpServer, Page, PageRequest,
    StartTaskRequest, TaskId, bind_local,
};
use a2a_lab_ot2::camera::{
    CameraCapture, CameraCaptureError, DEFAULT_EXTERNAL_CAMERA_DESCRIPTION,
    EXTERNAL_CAMERA_SOURCE_ID, ExternalCameraFormat,
};
use a2a_lab_ot2::opentrons::OPENTRONS_CAMERA_SOURCE_ID;
use a2a_lab_ot2::{ImageServeConfig, OpentronsLab, image_catalog};
use axum::Router;
use axum::body::Body;
use axum::extract::State;
use axum::http::{HeaderValue, StatusCode, header};
use axum::response::Response;
use axum::routing::post;
use tokio::net::TcpListener;

fn bin() -> Command {
    Command::new(env!("CARGO_BIN_EXE_a2a-lab-ot2"))
}

fn jpeg(width: u16, height: u16) -> Vec<u8> {
    let mut data = vec![0xFF, 0xD8, 0xFF, 0xC0, 0x00, 0x0B, 0x08];
    data.extend_from_slice(&height.to_be_bytes());
    data.extend_from_slice(&width.to_be_bytes());
    data.extend_from_slice(&[0x01, 0x01, 0x11, 0x00, 0xFF, 0xD9]);
    data
}

fn page() -> PageRequest {
    PageRequest::new(None, 50).unwrap()
}

#[test]
fn help_lists_image_defaults() {
    let output = bin().arg("--help").output().unwrap();
    assert!(output.status.success());
    let help = String::from_utf8(output.stdout).unwrap();
    assert!(help.contains("--external-camera-index"));
    assert!(help.contains("--external-camera-description"));
    assert!(help.contains("Still frame from the configured external camera."));
    assert!(!help.contains("v4l2"));
    assert!(help.contains("--max-image-bytes"));
    assert!(help.contains("67108864"));
    assert!(help.contains("--capture-timeout-ms"));
    assert!(help.contains("10000"));
    assert!(help.contains("--image-retention"));
    assert!(help.contains("32"));
}

#[test]
fn invalid_limits_fail_before_robot_contact() {
    for args in [
        vec!["--max-image-bytes", "0"],
        vec!["--capture-timeout-ms", "0"],
        vec!["--image-retention", "0"],
        vec!["--max-image-bytes", "18446744073709551615"],
    ] {
        let output = bin().args(args.clone()).output().unwrap();
        assert!(!output.status.success(), "{args:?}");
        let err = String::from_utf8_lossy(&output.stderr);
        assert!(
            err.contains("greater than zero") || err.contains("overflow-prone"),
            "{args:?}: {err}"
        );
        assert!(!err.contains("unreachable"), "{err}");
    }
}

#[test]
fn startup_line_enables_the_extra_camera_only_when_an_index_is_set() {
    let off = bin()
        .env_remove("A2ALAB_EXTERNAL_CAMERA_INDEX")
        .env("A2ALAB_OPENTRONS_URL", "http://127.0.0.1:9")
        .output()
        .unwrap();
    let off_out = String::from_utf8_lossy(&off.stdout);
    assert!(off_out.contains("opentrons-camera asset opentrons-ot2"));
    assert!(off_out.contains("external-camera off"));
    assert!(!off_out.contains("external-camera index"));

    let overridden = bin()
        .env("A2ALAB_EXTERNAL_CAMERA_INDEX", "2")
        .env("A2ALAB_OPENTRONS_URL", "http://127.0.0.1:9")
        .args(["--external-camera-index", "9"])
        .output()
        .unwrap();
    let out = String::from_utf8_lossy(&overridden.stdout);
    assert!(out.contains("external-camera index 9"));
    assert!(!out.contains("external-camera index 2"));

    let from_env = bin()
        .env("A2ALAB_EXTERNAL_CAMERA_INDEX", "2")
        .env("A2ALAB_OPENTRONS_URL", "http://127.0.0.1:9")
        .output()
        .unwrap();
    let env_out = String::from_utf8_lossy(&from_env.stdout);
    assert!(env_out.contains("external-camera index 2"));

    let invalid = bin()
        .env_remove("A2ALAB_EXTERNAL_CAMERA_INDEX")
        .args(["--external-camera-index", "camera-zero"])
        .output()
        .unwrap();
    assert!(!invalid.status.success());
    assert!(String::from_utf8_lossy(&invalid.stderr).contains("invalid value"));

    let described = bin()
        .env_remove("A2ALAB_EXTERNAL_CAMERA_INDEX")
        .env("A2ALAB_EXTERNAL_CAMERA_DESCRIPTION", "From env")
        .env("A2ALAB_OPENTRONS_URL", "http://127.0.0.1:9")
        .args([
            "--external-camera-index",
            "9",
            "--external-camera-description",
            "From flag",
        ])
        .output()
        .unwrap();
    let described_out = String::from_utf8_lossy(&described.stdout);
    assert!(described_out.contains("description From flag"));
    assert!(!described_out.contains("From env"));

    let from_env = bin()
        .env("A2ALAB_EXTERNAL_CAMERA_INDEX", "2")
        .env("A2ALAB_EXTERNAL_CAMERA_DESCRIPTION", "From env")
        .env("A2ALAB_OPENTRONS_URL", "http://127.0.0.1:9")
        .output()
        .unwrap();
    assert!(String::from_utf8_lossy(&from_env.stdout).contains("description From env"));

    let empty_description = bin()
        .env_remove("A2ALAB_EXTERNAL_CAMERA_DESCRIPTION")
        .args(["--external-camera-description", ""])
        .output()
        .unwrap();
    assert!(!empty_description.status.success());
    assert!(String::from_utf8_lossy(&empty_description.stderr).contains("must not be empty"));
}

#[test]
fn config_rejects_zero_and_overflow() {
    assert_eq!(
        ImageServeConfig::new(Some(4), None, 0, 1_000, 4)
            .unwrap_err()
            .code(),
        "invalid"
    );
    assert_eq!(
        ImageServeConfig::new(Some(4), None, 1024, 0, 4)
            .unwrap_err()
            .code(),
        "invalid"
    );
    assert_eq!(
        ImageServeConfig::new(Some(4), None, 1024, 1_000, 0)
            .unwrap_err()
            .code(),
        "invalid"
    );
    assert!(
        ImageServeConfig::new(Some(4), None, u64::MAX, 1_000, 4)
            .unwrap_err()
            .to_string()
            .contains("overflow-prone")
    );
    let off = ImageServeConfig::new(None, None, 1024, 1_000, 4).unwrap();
    assert_eq!(off.camera_index(), None);
    assert!(off.summary().contains("external-camera off"));
    let config = ImageServeConfig::new(Some(4), None, 1024, 1_000, 4).unwrap();
    assert_eq!(config.camera_index(), Some(4));
    assert_eq!(config.max_image_bytes(), 1024);
    assert_eq!(config.capture_timeout(), Duration::from_millis(1_000));
    assert_eq!(config.retention_per_source(), 4);
    assert!(config.summary().contains("external-camera index 4"));
}

#[test]
fn external_camera_description_defaults_and_rejects_empty() {
    assert_eq!(
        ImageServeConfig::new(Some(4), Some(""), 1024, 1_000, 4)
            .unwrap_err()
            .code(),
        "invalid"
    );
    let config = ImageServeConfig::new(Some(4), None, 1024, 1_000, 4).unwrap();
    assert_eq!(config.description(), DEFAULT_EXTERNAL_CAMERA_DESCRIPTION);
    let named =
        ImageServeConfig::new(Some(4), Some("Bench camera over the deck"), 1024, 1_000, 4).unwrap();
    assert_eq!(named.description(), "Bench camera over the deck");
    assert!(
        named
            .summary()
            .contains("description Bench camera over the deck")
    );
}

#[derive(Clone)]
struct Script {
    calls: Arc<Mutex<Vec<u32>>>,
    result: Arc<Mutex<Result<Vec<u8>, CameraCaptureError>>>,
}

impl Script {
    fn ok(bytes: Vec<u8>) -> Self {
        Self::set(Ok(bytes))
    }

    fn fail(error: CameraCaptureError) -> Self {
        Self::set(Err(error))
    }

    fn set(result: Result<Vec<u8>, CameraCaptureError>) -> Self {
        Self {
            calls: Arc::new(Mutex::new(Vec::new())),
            result: Arc::new(Mutex::new(result)),
        }
    }
}

impl CameraCapture for Script {
    fn capture(
        &self,
        index: u32,
        _format: ExternalCameraFormat,
        _frames: u32,
    ) -> Result<Vec<u8>, CameraCaptureError> {
        self.calls.lock().expect("calls").push(index);
        self.result.lock().expect("result").clone()
    }
}

#[tokio::test]
async fn configured_description_is_advertised_for_the_external_camera() {
    let config =
        ImageServeConfig::new(Some(9), Some("Bench camera over the deck"), 1024, 1_000, 4).unwrap();
    let images = image_catalog(
        "http://127.0.0.1:9",
        &config,
        Some(Arc::new(Script::ok(jpeg(1, 1)))),
    )
    .unwrap();
    let page = images
        .list_image_sources(ListImageSourcesRequest::new(page()).unwrap())
        .await
        .unwrap();
    let external = page
        .items()
        .iter()
        .find(|source| source.id.as_str() == EXTERNAL_CAMERA_SOURCE_ID)
        .unwrap();
    assert_eq!(external.name, "External camera");
    assert_eq!(external.description, "Bench camera over the deck");
}

#[derive(Clone)]
struct Picture(Arc<Mutex<PictureBody>>);

#[derive(Clone)]
enum PictureBody {
    Jpeg(Vec<u8>),
    Disabled,
}

async fn picture_server(body: PictureBody) -> (String, Picture) {
    let picture = Picture(Arc::new(Mutex::new(body)));
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let app = Router::new()
        .route("/camera/picture", post(picture_handler))
        .route("/health", axum::routing::get(|| async { "ok" }))
        .with_state(picture.clone());
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    (format!("http://{address}"), picture)
}

async fn picture_handler(State(picture): State<Picture>) -> Response {
    let body = picture.0.lock().expect("picture").clone();
    match body {
        PictureBody::Jpeg(bytes) => {
            let mut response = Response::builder()
                .status(StatusCode::OK)
                .body(Body::from(bytes))
                .unwrap();
            response
                .headers_mut()
                .insert(header::CONTENT_TYPE, HeaderValue::from_static("image/jpeg"));
            response
        }
        PictureBody::Disabled => Response::builder()
            .status(StatusCode::UNPROCESSABLE_ENTITY)
            .body(Body::from(
                r#"{"detail":"Cannot take photo, camera is disabled."}"#,
            ))
            .unwrap(),
    }
}

struct Repeat;

impl AgentMessageHandler for Repeat {
    fn handle(
        &self,
        request: AgentMessageRequest,
    ) -> AgentMessageFuture<'_, Result<AgentMessageReply, A2aLabError>> {
        Box::pin(async move { Ok(AgentMessageReply { text: request.text }) })
    }
}

struct Endpoints {
    a2a: A2aClient,
    mcp: McpLab,
    mcp_url: String,
}

async fn serve(url: &str, config: &ImageServeConfig, grab: Script, readonly: bool) -> Endpoints {
    let catalog = image_catalog(url, config, Some(Arc::new(grab))).unwrap();
    let lab = OpentronsLab::new(url).unwrap().with_readonly(readonly);
    let service = A2aLabService::new(lab.clone(), lab.clone(), lab)
        .with_images(catalog)
        .with_image_transport(config.transport())
        .share();
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let mcp = McpServer::new(&service);
    tokio::spawn(async move {
        mcp.serve_http(listener).await.unwrap();
    });
    let mcp_url = format!("http://{address}/mcp");
    let mcp_lab = McpLab::connect_with(&mcp_url, config.transport())
        .await
        .unwrap();
    assert_eq!(mcp_lab.max_image_bytes(), config.max_image_bytes());
    let mcp_api: Arc<dyn A2aLabApi> = Arc::new(mcp_lab);
    let (a2a_listener, a2a_address) = bind_local().await.unwrap();
    let server = A2aServer::new(&mcp_api).with_message_handler(Arc::new(Repeat));
    tokio::spawn(async move {
        server.listen(a2a_listener).await.unwrap();
    });
    tokio::time::sleep(Duration::from_millis(20)).await;
    Endpoints {
        a2a: A2aClient::new(&format!("http://{a2a_address}"))
            .unwrap()
            .with_image_transport(config.transport()),
        mcp: McpLab::connect_with(&mcp_url, config.transport())
            .await
            .unwrap(),
        mcp_url,
    }
}

fn config(max_bytes: u64) -> ImageServeConfig {
    ImageServeConfig::new(Some(4), None, max_bytes, 1_000, 4).unwrap()
}

#[tokio::test]
async fn both_sources_match_across_a2a_and_mcp() {
    let bytes = jpeg(2, 3);
    let (url, _) = picture_server(PictureBody::Jpeg(bytes.clone())).await;
    let grab = Script::ok(bytes.clone());
    let endpoints = serve(&url, &config(1024), grab.clone(), false).await;

    let card = endpoints.a2a.agent_card().await.unwrap();
    let skills: Vec<_> = card.skills.iter().map(|skill| skill.id.as_str()).collect();
    assert_eq!(
        skills,
        [
            "list-log-sources",
            "query-logs",
            "list-metrics",
            "query-metric",
            "list-tasks",
            "start-task",
            "get-task-status",
            "list-image-sources",
            "list-images",
            "search-images",
            "get-image",
            "get-current-image",
            "agent-message",
        ]
    );
    assert_eq!(mcp_tools(&endpoints).await, expected_tools());

    let a2a_sources = endpoints
        .a2a
        .list_image_sources(ListImageSourcesRequest::new(page()).unwrap())
        .await
        .unwrap();
    let mcp_sources = mcp_sources(&endpoints.mcp).await;
    let ids = |page: &a2a_lab_dev_kit::Page<a2a_lab_dev_kit::ImageSource>| {
        page.items()
            .iter()
            .map(|source| source.id.as_str().to_owned())
            .collect::<Vec<_>>()
    };
    assert_eq!(
        ids(&a2a_sources),
        vec![EXTERNAL_CAMERA_SOURCE_ID, OPENTRONS_CAMERA_SOURCE_ID]
    );
    assert_eq!(ids(&a2a_sources), ids(&mcp_sources));
    assert_eq!(
        a2a_sources
            .items()
            .iter()
            .find(|source| source.id.as_str() == OPENTRONS_CAMERA_SOURCE_ID)
            .unwrap()
            .asset_id
            .as_deref(),
        Some("opentrons-ot2")
    );

    let external = ImageSourceId::new(EXTERNAL_CAMERA_SOURCE_ID).unwrap();
    let robot = ImageSourceId::new(OPENTRONS_CAMERA_SOURCE_ID).unwrap();
    let a2a_frame = endpoints
        .a2a
        .get_current_image(GetCurrentImageRequest::new(external.clone()))
        .await
        .unwrap();
    let mcp_frame = mcp_current(&endpoints.mcp, robot).await.unwrap();
    assert_eq!(a2a_frame.data(), bytes.as_slice());
    assert_eq!(mcp_frame.data(), bytes.as_slice());
    assert_eq!(a2a_frame.descriptor().width(), 2);
    assert_eq!(grab.calls.lock().expect("calls").len(), 1);
}

#[tokio::test]
async fn one_source_can_fail_while_the_other_captures() {
    let bytes = jpeg(4, 1);
    let (url, _) = picture_server(PictureBody::Disabled).await;
    let grab = Script::ok(bytes.clone());
    let endpoints = serve(&url, &config(1024), grab, false).await;
    let external = ImageSourceId::new(EXTERNAL_CAMERA_SOURCE_ID).unwrap();
    let robot = ImageSourceId::new(OPENTRONS_CAMERA_SOURCE_ID).unwrap();
    let frame = endpoints
        .a2a
        .get_current_image(GetCurrentImageRequest::new(external))
        .await
        .unwrap();
    assert_eq!(frame.data(), bytes.as_slice());
    let failed = mcp_current(&endpoints.mcp, robot.clone())
        .await
        .unwrap_err();
    assert_eq!(failed.code(), "invalid");
    assert!(failed.to_string().contains("disabled"));
    let sources = endpoints
        .a2a
        .list_image_sources(ListImageSourcesRequest::new(page()).unwrap())
        .await
        .unwrap();
    assert_eq!(sources.items().len(), 2);

    let (url, _) = picture_server(PictureBody::Jpeg(bytes.clone())).await;
    let endpoints = serve(
        &url,
        &config(1024),
        Script::fail(CameraCaptureError::Backend("camera unavailable".to_owned())),
        false,
    )
    .await;
    let failed = endpoints
        .a2a
        .get_current_image(GetCurrentImageRequest::new(
            ImageSourceId::new(EXTERNAL_CAMERA_SOURCE_ID).unwrap(),
        ))
        .await
        .unwrap_err();
    assert_eq!(failed.code(), "unavailable");
    assert!(failed.to_string().contains("index 4"));
    let frame = mcp_current(&endpoints.mcp, robot).await.unwrap();
    assert_eq!(frame.data(), bytes.as_slice());
}

#[tokio::test]
async fn payload_limit_matches_and_readonly_keeps_image_reads() {
    let bytes = jpeg(1, 1);
    let (url, _) = picture_server(PictureBody::Jpeg(bytes)).await;
    let endpoints = serve(&url, &config(8), Script::ok(jpeg(3, 3)), true).await;
    let external = ImageSourceId::new(EXTERNAL_CAMERA_SOURCE_ID).unwrap();
    let a2a_error = endpoints
        .a2a
        .get_current_image(GetCurrentImageRequest::new(external.clone()))
        .await
        .unwrap_err();
    let mcp_error = mcp_current(&endpoints.mcp, external).await.unwrap_err();
    assert_eq!(a2a_error.code(), "invalid");
    assert_eq!(mcp_error.code(), "invalid");

    let tasks = endpoints
        .a2a
        .list_tasks(ListTasksRequest { page: page() })
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
    let denied = endpoints
        .mcp
        .execute(A2aLabCommand::StartTask(
            StartTaskRequest::new(TaskId::new("pause_run").unwrap(), JsonObject::empty())
                .immediate(),
        ))
        .await
        .unwrap_err();
    assert_eq!(denied.code(), "not_found");
    let sources = endpoints
        .a2a
        .list_image_sources(ListImageSourcesRequest::new(page()).unwrap())
        .await
        .unwrap();
    assert_eq!(sources.items().len(), 2);
}

fn expected_tools() -> Vec<String> {
    [
        "get_current_image",
        "get_image",
        "get_task_status",
        "list_image_sources",
        "list_images",
        "list_log_sources",
        "list_metrics",
        "list_tasks",
        "query_logs",
        "query_metric",
        "search_images",
        "start_task",
    ]
    .into_iter()
    .map(str::to_owned)
    .collect()
}

async fn mcp_sources(mcp: &McpLab) -> Page<a2a_lab_dev_kit::ImageSource> {
    let outcome = mcp
        .execute(A2aLabCommand::ListImageSources(
            ListImageSourcesRequest::new(page()).unwrap(),
        ))
        .await
        .unwrap();
    let A2aLabResult::ListImageSources(page) = outcome.task.result else {
        panic!("list_image_sources");
    };
    page
}

async fn mcp_current(mcp: &McpLab, source: ImageSourceId) -> Result<Image, A2aLabError> {
    mcp.execute(A2aLabCommand::GetCurrentImage(GetCurrentImageRequest::new(
        source,
    )))
    .await?
    .task
    .result
    .into_image()
}

trait ImageResult {
    fn into_image(self) -> Result<Image, A2aLabError>;
}

impl ImageResult for A2aLabResult {
    fn into_image(self) -> Result<Image, A2aLabError> {
        match self {
            Self::GetCurrentImage(image) => Ok(image),
            _ => Err(A2aLabError::protocol("expected a current image")),
        }
    }
}

async fn mcp_tools(endpoints: &Endpoints) -> Vec<String> {
    http_tools(&endpoints.mcp_url).await
}

async fn http_tools(url: &str) -> Vec<String> {
    let http = reqwest::Client::new();
    let opened = mcp_http(
        &http,
        url,
        None,
        r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2026-07-28","capabilities":{},"clientInfo":{"name":"a2a-lab-ot2","version":"0"}}}"#,
    )
    .await;
    mcp_http(
        &http,
        url,
        opened.0.as_deref(),
        r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#,
    )
    .await;
    let listed = mcp_http(
        &http,
        url,
        opened.0.as_deref(),
        r#"{"jsonrpc":"2.0","id":2,"method":"tools/list","params":{}}"#,
    )
    .await;
    let mut names = Vec::new();
    for payload in listed
        .1
        .lines()
        .filter_map(|line| line.strip_prefix("data:"))
    {
        let Ok(value) = serde_json::from_str::<serde_json::Value>(payload.trim()) else {
            continue;
        };
        let Some(tools) = value
            .get("result")
            .and_then(|result| result.get("tools"))
            .and_then(|tools| tools.as_array())
        else {
            continue;
        };
        names.extend(tools.iter().filter_map(|tool| {
            tool.get("name")
                .and_then(|name| name.as_str())
                .map(str::to_owned)
        }));
    }
    names.sort();
    names
}

async fn mcp_http(
    http: &reqwest::Client,
    url: &str,
    session: Option<&str>,
    body: &str,
) -> (Option<String>, String) {
    let mut request = http
        .post(url)
        .header("content-type", "application/json")
        .header("accept", "application/json, text/event-stream")
        .body(body.to_owned());
    if let Some(session) = session {
        request = request.header("mcp-session-id", session);
    }
    let response = request.send().await.expect("mcp http");
    assert!(response.status().is_success(), "{}", response.status());
    let session = response
        .headers()
        .get("mcp-session-id")
        .and_then(|value| value.to_str().ok())
        .map(str::to_owned);
    let body = response.text().await.expect("mcp body");
    (session, body)
}
