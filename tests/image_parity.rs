//! A2A and MCP image parity for the two OT-2 camera sources.

use std::collections::VecDeque;
use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use a2a_lab_dev_kit::JsonObject;
use a2a_lab_dev_kit::{
    A2aClient, A2aLabApi, A2aLabCommand, A2aLabError, A2aLabResult, A2aLabService, A2aServer,
    GetCurrentImageRequest, GetImageRequest, Image, ImageDescriptor, ImageSource, ImageSourceId,
    ListImageSourcesRequest, ListImagesRequest, McpLab, McpServer, Page, PageRequest,
    SearchImagesRequest, TimeRange, UtcTimestamp, bind_local,
};
use a2a_lab_ot2::camera::{
    CameraCapture, CameraCaptureError, EXTERNAL_CAMERA_SOURCE_ID, ExternalCamera,
    ExternalCameraFormat, external_camera_source,
};
use a2a_lab_ot2::opentrons::{
    OPENTRONS_CAMERA_SOURCE_ID, OpentronsCamera, opentrons_camera_source,
};
use a2a_lab_ot2::{CapturedFrame, ImageCapture, ImageCatalogConfig, LiveImageCatalog};
use axum::Router;
use axum::body::Body;
use axum::extract::State;
use axum::http::{HeaderValue, StatusCode, header};
use axum::response::Response;
use axum::routing::post;
use base64::Engine;
use tokio::net::TcpListener;

const OT: &str = OPENTRONS_CAMERA_SOURCE_ID;
const EXT: &str = EXTERNAL_CAMERA_SOURCE_ID;

struct Endpoints {
    a2a: A2aClient,
    mcp: McpLab,
    mcp_url: String,
    a2a_base: String,
}

enum Outcome {
    Sources(Page<ImageSource>),
    Images(Page<ImageDescriptor>),
    Image(Image),
    Error(A2aLabError),
}

struct Script(Mutex<VecDeque<Result<CapturedFrame, A2aLabError>>>);

impl Script {
    fn new(frames: Vec<Result<CapturedFrame, A2aLabError>>) -> Self {
        Self(Mutex::new(frames.into()))
    }
}

impl ImageCapture for Script {
    fn capture(
        &self,
    ) -> Pin<Box<dyn Future<Output = Result<CapturedFrame, A2aLabError>> + Send + '_>> {
        let result = self
            .0
            .lock()
            .expect("frames")
            .pop_front()
            .unwrap_or_else(|| Err(A2aLabError::unavailable("scripted capture is exhausted")));
        Box::pin(std::future::ready(result))
    }
}

fn stamp(value: &str) -> UtcTimestamp {
    UtcTimestamp::parse(value).unwrap()
}

fn jpeg(width: u16, height: u16) -> Vec<u8> {
    let mut data = vec![0xFF, 0xD8, 0xFF, 0xC0, 0x00, 0x0B, 0x08];
    data.extend_from_slice(&height.to_be_bytes());
    data.extend_from_slice(&width.to_be_bytes());
    data.extend_from_slice(&[0x01, 0x01, 0x11, 0x00, 0xFF, 0xD9]);
    data
}

fn padded(len: usize) -> Vec<u8> {
    let mut data = jpeg(1, 1);
    assert!(data.len() <= len);
    data.resize(len, 0);
    data
}

fn frame(at: &str, caption: &str, bytes: Vec<u8>) -> CapturedFrame {
    CapturedFrame::new(
        stamp(at),
        "image/jpeg",
        2,
        2,
        Some(caption.to_owned()),
        JsonObject::empty(),
        bytes,
    )
}

fn page(limit: u32) -> PageRequest {
    PageRequest::new(None, limit).unwrap()
}

fn source(id: &str) -> ImageSourceId {
    ImageSourceId::new(id).unwrap()
}

fn catalog(retention: u64, max_bytes: u64, external: Script, robot: Script) -> LiveImageCatalog {
    LiveImageCatalog::new(
        vec![
            (external_camera_source().unwrap(), Arc::new(external)),
            (opentrons_camera_source().unwrap(), Arc::new(robot)),
        ],
        ImageCatalogConfig::new(retention, max_bytes).unwrap(),
    )
    .unwrap()
}

async fn serve(images: LiveImageCatalog, max_bytes: u64) -> Endpoints {
    let transport = a2a_lab_dev_kit::ImageTransportConfig::new(max_bytes).unwrap();
    let lab = a2a_lab_ot2::OpentronsLab::new("http://127.0.0.1:9").unwrap();
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
    let mcp_url = format!("http://{address}/mcp");
    let mcp_lab = McpLab::connect_with(&mcp_url, transport).await.unwrap();
    let mcp_api: Arc<dyn A2aLabApi> = Arc::new(mcp_lab);
    let (a2a_listener, a2a_address) = bind_local().await.unwrap();
    let server = A2aServer::new(&mcp_api);
    tokio::spawn(async move {
        server.listen(a2a_listener).await.unwrap();
    });
    tokio::time::sleep(Duration::from_millis(20)).await;
    let a2a_base = format!("http://{a2a_address}");
    Endpoints {
        a2a: A2aClient::new(&a2a_base)
            .unwrap()
            .with_image_transport(transport),
        mcp: McpLab::connect_with(&mcp_url, transport).await.unwrap(),
        mcp_url,
        a2a_base,
    }
}

fn render(outcome: &Outcome) -> String {
    match outcome {
        Outcome::Sources(page) => format!(
            "sources {:?} next={:?}",
            page.items()
                .iter()
                .map(|item| item.id.as_str())
                .collect::<Vec<_>>(),
            page.next_cursor()
        ),
        Outcome::Images(page) => format!(
            "images {:?} next={:?}",
            page.items()
                .iter()
                .map(|item| {
                    format!(
                        "{}@{}:{}",
                        item.id(),
                        item.source_id(),
                        item.caption().unwrap_or("")
                    )
                })
                .collect::<Vec<_>>(),
            page.next_cursor()
        ),
        Outcome::Image(image) => format!(
            "image {} bytes={}",
            image.descriptor().id(),
            image.data().len()
        ),
        Outcome::Error(error) => format!("error {} {error}", error.code()),
    }
}

fn diverge(case: &str, a2a: &Outcome, mcp: &Outcome) -> ! {
    panic!("case {case}\nA2A: {}\nMCP: {}", render(a2a), render(mcp));
}

fn agree(case: &str, a2a: Outcome, mcp: &Outcome) -> Outcome {
    let same = match (&a2a, mcp) {
        (Outcome::Sources(left), Outcome::Sources(right)) => left == right,
        (Outcome::Images(left), Outcome::Images(right)) => left == right,
        (Outcome::Image(left), Outcome::Image(right)) => {
            left.descriptor() == right.descriptor() && left.data() == right.data()
        }
        (Outcome::Error(left), Outcome::Error(right)) => left.code() == right.code(),
        _ => false,
    };
    if !same {
        diverge(case, &a2a, mcp);
    }
    a2a
}

async fn list_sources(endpoints: &Endpoints, limit: u32) -> Outcome {
    let request = ListImageSourcesRequest::new(page(limit)).unwrap();
    let a2a = endpoints
        .a2a
        .list_image_sources(request.clone())
        .await
        .map_or_else(Outcome::Error, Outcome::Sources);
    let mcp = match endpoints
        .mcp
        .execute(A2aLabCommand::ListImageSources(request))
        .await
    {
        Ok(outcome) => match outcome.task.result {
            A2aLabResult::ListImageSources(page) => Outcome::Sources(page),
            other => Outcome::Error(A2aLabError::protocol(format!("{other:?}"))),
        },
        Err(error) => Outcome::Error(error),
    };
    agree("list_image_sources", a2a, &mcp)
}

async fn list_images(endpoints: &Endpoints, id: &str, limit: u32, cursor: Option<&str>) -> Outcome {
    let request = ListImagesRequest::new(
        source(id),
        PageRequest::new(cursor.map(str::to_owned), limit).unwrap(),
    )
    .unwrap();
    let a2a = endpoints
        .a2a
        .list_images(request.clone())
        .await
        .map_or_else(Outcome::Error, Outcome::Images);
    let mcp = image_page(
        &endpoints.mcp,
        A2aLabCommand::ListImages(request),
        "list_images",
    )
    .await;
    agree("list_images", a2a, &mcp)
}

async fn search(
    endpoints: &Endpoints,
    id: Option<&str>,
    start: Option<&str>,
    end: Option<&str>,
    text: Option<&str>,
) -> Outcome {
    let range = match (start, end) {
        (Some(start), Some(end)) => Some(TimeRange::new(stamp(start), stamp(end)).unwrap()),
        _ => None,
    };
    let request =
        SearchImagesRequest::new(id.map(source), range, text.map(str::to_owned), page(10)).unwrap();
    let a2a = endpoints
        .a2a
        .search_images(request.clone())
        .await
        .map_or_else(Outcome::Error, Outcome::Images);
    let mcp = image_page(
        &endpoints.mcp,
        A2aLabCommand::SearchImages(request),
        "search_images",
    )
    .await;
    agree("search_images", a2a, &mcp)
}

async fn image_page(mcp: &McpLab, command: A2aLabCommand, kind: &str) -> Outcome {
    match mcp.execute(command).await {
        Ok(outcome) => match outcome.task.result {
            A2aLabResult::ListImages(page) | A2aLabResult::SearchImages(page) => {
                Outcome::Images(page)
            }
            other => Outcome::Error(A2aLabError::protocol(format!("{kind} {other:?}"))),
        },
        Err(error) => Outcome::Error(error),
    }
}

async fn current_a2a(endpoints: &Endpoints, id: &str) -> Image {
    let image = endpoints
        .a2a
        .get_current_image(GetCurrentImageRequest::new(source(id)))
        .await
        .unwrap_or_else(|error| panic!("capture {id}: {error}"));
    fetch(endpoints, &image).await
}

async fn current_error(endpoints: &Endpoints, id: &str) -> Outcome {
    let request = GetCurrentImageRequest::new(source(id));
    let a2a = endpoints
        .a2a
        .get_current_image(request.clone())
        .await
        .map_or_else(Outcome::Error, Outcome::Image);
    let mcp = current_mcp(&endpoints.mcp, request).await;
    match (&a2a, &mcp) {
        (Outcome::Error(left), Outcome::Error(right)) if left.code() == right.code() => a2a,
        (Outcome::Image(left), Outcome::Image(right)) if left.data() == right.data() => {
            fetch(endpoints, left).await;
            a2a
        }
        _ => diverge("get_current_image", &a2a, &mcp),
    }
}

async fn current_mcp(mcp: &McpLab, request: GetCurrentImageRequest) -> Outcome {
    match mcp.execute(A2aLabCommand::GetCurrentImage(request)).await {
        Ok(outcome) => match outcome.task.result {
            A2aLabResult::GetCurrentImage(image) => Outcome::Image(image),
            other => Outcome::Error(A2aLabError::protocol(format!("{other:?}"))),
        },
        Err(error) => Outcome::Error(error),
    }
}

async fn fetch(endpoints: &Endpoints, image: &Image) -> Image {
    let request = GetImageRequest::new(image.descriptor().id().clone());
    let a2a = endpoints.a2a.get_image(request.clone()).await.unwrap();
    let mcp = match endpoints
        .mcp
        .execute(A2aLabCommand::GetImage(request))
        .await
        .unwrap()
        .task
        .result
    {
        A2aLabResult::GetImage(image) => image,
        other => panic!("get_image {other:?}"),
    };
    assert_eq!(a2a.data(), image.data());
    assert_eq!(a2a.descriptor(), image.descriptor());
    assert_eq!(mcp.data(), image.data());
    assert_eq!(mcp.descriptor(), image.descriptor());
    let encoded = base64::engine::general_purpose::STANDARD.encode(image.data());
    let json = serde_json::to_string(&a2a).unwrap();
    assert!(json.contains(&encoded));
    assert!(!json.contains("\"uri\""));
    a2a
}

fn captions(outcome: &Outcome) -> Vec<Option<String>> {
    let Outcome::Images(page) = outcome else {
        panic!("{}", render(outcome));
    };
    page.items()
        .iter()
        .map(|item| item.caption().map(str::to_owned))
        .collect()
}

#[tokio::test]
async fn history_search_pages_and_retention_match() {
    let external = Script::new(vec![
        Ok(frame("2024-06-01T00:00:00Z", "deck left", jpeg(2, 2))),
        Ok(frame("2024-06-01T00:00:01Z", "Deck right", jpeg(2, 3))),
        Ok(frame("2024-06-01T00:00:02Z", "tip rack", jpeg(3, 2))),
    ]);
    let robot = Script::new(vec![Ok(frame(
        "2024-06-01T00:00:01Z",
        "ot deck",
        jpeg(4, 1),
    ))]);
    let endpoints = serve(catalog(2, 4096, external, robot), 4096).await;
    let sources = list_sources(&endpoints, 1).await;
    let Outcome::Sources(first) = &sources else {
        panic!("{}", render(&sources));
    };
    assert_eq!(first.items()[0].id.as_str(), EXT);
    assert!(first.next_cursor().is_some());
    let second = list_sources(&endpoints, 10).await;
    let Outcome::Sources(all) = &second else {
        panic!("{}", render(&second));
    };
    assert_eq!(
        all.items()
            .iter()
            .map(|item| item.id.as_str())
            .collect::<Vec<_>>(),
        vec![EXT, OT]
    );
    assert_eq!(
        all.items()
            .iter()
            .find(|item| item.id.as_str() == OT)
            .unwrap()
            .asset_id
            .as_deref(),
        Some("opentrons-ot2")
    );

    let oldest = current_a2a(&endpoints, EXT).await;
    let kept = current_a2a(&endpoints, EXT).await;
    let newest = current_a2a(&endpoints, EXT).await;
    let robot_frame = current_a2a(&endpoints, OT).await;
    assert_eq!(kept.descriptor().caption(), Some("Deck right"));
    assert_eq!(newest.descriptor().caption(), Some("tip rack"));
    assert_eq!(robot_frame.descriptor().caption(), Some("ot deck"));
    let missing = endpoints
        .a2a
        .get_image(GetImageRequest::new(oldest.descriptor().id().clone()))
        .await
        .unwrap_err();
    let missing_mcp = endpoints
        .mcp
        .execute(A2aLabCommand::GetImage(GetImageRequest::new(
            oldest.descriptor().id().clone(),
        )))
        .await
        .unwrap_err();
    assert_eq!(missing.code(), missing_mcp.code());
    assert_eq!(missing.code(), "not_found");

    let listed = list_images(&endpoints, EXT, 1, None).await;
    assert_eq!(captions(&listed), vec![Some("Deck right".to_owned())]);
    let Outcome::Images(page_one) = &listed else {
        panic!("{}", render(&listed));
    };
    let final_page = list_images(&endpoints, EXT, 1, page_one.next_cursor()).await;
    assert_eq!(captions(&final_page), vec![Some("tip rack".to_owned())]);
    let Outcome::Images(last) = &final_page else {
        panic!("{}", render(&final_page));
    };
    assert!(last.next_cursor().is_none());
    assert_filters(&endpoints).await;
}

async fn assert_filters(endpoints: &Endpoints) {
    let ranged = search(
        endpoints,
        None,
        Some("2024-06-01T00:00:01Z"),
        Some("2024-06-01T00:00:02Z"),
        None,
    )
    .await;
    assert_eq!(
        captions(&ranged),
        vec![Some("Deck right".to_owned()), Some("ot deck".to_owned())]
    );
    let text = search(endpoints, None, None, None, Some("DECK")).await;
    assert_eq!(
        captions(&text),
        vec![Some("Deck right".to_owned()), Some("ot deck".to_owned())]
    );
    let combined = search(
        endpoints,
        Some(EXT),
        Some("2024-06-01T00:00:01Z"),
        Some("2024-06-01T00:00:03Z"),
        Some("deck"),
    )
    .await;
    assert_eq!(captions(&combined), vec![Some("Deck right".to_owned())]);
    let empty = search(endpoints, None, None, None, Some("absent")).await;
    let Outcome::Images(empty_page) = &empty else {
        panic!("{}", render(&empty));
    };
    assert!(empty_page.items().is_empty());
    assert!(empty_page.next_cursor().is_none());
    assert_unknown(endpoints).await;
}

async fn assert_unknown(endpoints: &Endpoints) {
    let unknown = list_images(endpoints, "missing-cam", 10, None).await;
    let Outcome::Error(error) = &unknown else {
        panic!("{}", render(&unknown));
    };
    assert_eq!(error.code(), "not_found");
    let unknown_image = agree(
        "unknown image",
        endpoints
            .a2a
            .get_image(GetImageRequest::new(
                a2a_lab_dev_kit::ImageId::new("img-missing").unwrap(),
            ))
            .await
            .map_or_else(Outcome::Error, Outcome::Image),
        &match endpoints
            .mcp
            .execute(A2aLabCommand::GetImage(GetImageRequest::new(
                a2a_lab_dev_kit::ImageId::new("img-missing").unwrap(),
            )))
            .await
        {
            Ok(outcome) => {
                Outcome::Error(A2aLabError::protocol(format!("{:?}", outcome.task.result)))
            }
            Err(error) => Outcome::Error(error),
        },
    );
    let Outcome::Error(error) = unknown_image else {
        panic!("unknown image was returned");
    };
    assert_eq!(error.code(), "not_found");
}

#[tokio::test]
async fn limits_and_device_failures_match() {
    let at = padded(32);
    let below = padded(31);
    let above = padded(33);
    let endpoints = serve(
        catalog(
            4,
            32,
            Script::new(vec![
                Ok(frame("2024-06-01T00:00:00Z", "below", below.clone())),
                Ok(frame("2024-06-01T00:00:01Z", "above", above.clone())),
                Ok(frame("2024-06-01T00:00:02Z", "above", above)),
            ]),
            Script::new(vec![Ok(frame("2024-06-01T00:00:02Z", "at", at.clone()))]),
        ),
        32,
    )
    .await;
    let low = current_a2a(&endpoints, EXT).await;
    assert_eq!(low.data(), below.as_slice());
    let exact = current_a2a(&endpoints, OT).await;
    assert_eq!(exact.data(), at.as_slice());
    let over = current_error(&endpoints, EXT).await;
    let Outcome::Error(error) = &over else {
        panic!("{}", render(&over));
    };
    assert_eq!(error.code(), "invalid");

    let url = serve_picture(PictureBody::Disabled).await;
    let failed = device_pair(
        &url,
        Some(CameraCaptureError::Backend("camera busy".to_owned())),
        Duration::from_millis(500),
    )
    .await;
    assert_eq!(failed.robot_code, "invalid");
    assert_eq!(failed.external_code, "unavailable");
    assert!(failed.external_bytes.is_none());

    let url = serve_picture(PictureBody::Garbage).await;
    let failed = device_pair(
        &url,
        Some(CameraCaptureError::Backend("camera missing".to_owned())),
        Duration::from_millis(500),
    )
    .await;
    assert_eq!(failed.robot_code, "invalid");
    assert_eq!(failed.external_code, "unavailable");

    let url = serve_picture(PictureBody::Jpeg(jpeg(2, 2))).await;
    let failed = device_pair(&url, None, Duration::from_millis(40)).await;
    assert_eq!(failed.robot_code, "ok");
    assert_eq!(failed.external_code, "unavailable");
}

struct DeviceResult {
    robot_code: &'static str,
    external_code: &'static str,
    external_bytes: Option<Vec<u8>>,
}

#[derive(Clone)]
struct FailedCapture(CameraCaptureError);

impl CameraCapture for FailedCapture {
    fn capture(
        &self,
        _index: u32,
        _format: ExternalCameraFormat,
        _frames: u32,
    ) -> Result<Vec<u8>, CameraCaptureError> {
        Err(self.0.clone())
    }
}

struct Slow;

impl CameraCapture for Slow {
    fn capture(
        &self,
        _index: u32,
        _format: ExternalCameraFormat,
        _frames: u32,
    ) -> Result<Vec<u8>, CameraCaptureError> {
        std::thread::sleep(Duration::from_millis(200));
        Ok(jpeg(1, 1))
    }
}

async fn device_pair(
    url: &str,
    failure: Option<CameraCaptureError>,
    timeout: Duration,
) -> DeviceResult {
    let robot = OpentronsCamera::new(url, timeout, 4096).unwrap();
    let external = if let Some(failure) = failure {
        ExternalCamera::with_capture(4, timeout, 4096, FailedCapture(failure)).unwrap()
    } else {
        ExternalCamera::with_capture(4, timeout, 4096, Slow).unwrap()
    };
    let images = LiveImageCatalog::new(
        vec![
            (external_camera_source().unwrap(), Arc::new(external)),
            (opentrons_camera_source().unwrap(), Arc::new(robot)),
        ],
        ImageCatalogConfig::new(4, 4096).unwrap(),
    )
    .unwrap();
    let endpoints = serve(images, 4096).await;
    let external = current_error(&endpoints, EXT).await;
    let robot = current_error(&endpoints, OT).await;
    let (external_code, external_bytes) = match external {
        Outcome::Error(error) => (code_name(&error), None),
        Outcome::Image(image) => ("ok", Some(image.data().to_vec())),
        other => panic!("{}", render(&other)),
    };
    let robot_code = match robot {
        Outcome::Error(error) => code_name(&error),
        Outcome::Image(_) => "ok",
        other => panic!("{}", render(&other)),
    };
    DeviceResult {
        robot_code,
        external_code,
        external_bytes,
    }
}

fn code_name(error: &A2aLabError) -> &'static str {
    match error.code() {
        "invalid" => "invalid",
        "unavailable" => "unavailable",
        "not_found" => "not_found",
        _ => "protocol",
    }
}

#[derive(Clone)]
enum PictureBody {
    Jpeg(Vec<u8>),
    Disabled,
    Garbage,
}

async fn serve_picture(body: PictureBody) -> String {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let app = Router::new()
        .route("/camera/picture", post(picture))
        .with_state(body);
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    format!("http://{address}")
}

async fn picture(State(body): State<PictureBody>) -> Response {
    match body {
        PictureBody::Jpeg(bytes) => jpeg_response(StatusCode::OK, bytes),
        PictureBody::Garbage => jpeg_response(StatusCode::OK, b"not-a-jpeg".to_vec()),
        PictureBody::Disabled => Response::builder()
            .status(StatusCode::UNPROCESSABLE_ENTITY)
            .body(Body::from(
                r#"{"detail":"Cannot take photo, camera is disabled."}"#,
            ))
            .unwrap(),
    }
}

fn jpeg_response(status: StatusCode, bytes: Vec<u8>) -> Response {
    let mut response = Response::builder()
        .status(status)
        .body(Body::from(bytes))
        .unwrap();
    response
        .headers_mut()
        .insert(header::CONTENT_TYPE, HeaderValue::from_static("image/jpeg"));
    response
}

#[tokio::test]
async fn skills_and_tools_differ_only_by_protocol_framing() {
    let endpoints = serve(
        catalog(
            4,
            4096,
            Script::new(vec![Ok(frame("2024-06-01T00:00:00Z", "deck", jpeg(2, 2)))]),
            Script::new(vec![]),
        ),
        4096,
    )
    .await;
    let card = endpoints.a2a.agent_card().await.unwrap();
    let skills: Vec<_> = card
        .skills
        .iter()
        .map(|skill| skill.id.as_str().to_owned())
        .filter(|id| id.contains("image"))
        .collect();
    let tools = image_tools(&endpoints.mcp_url).await;
    let mut from_skills: Vec<_> = skills.iter().map(|id| id.replace('-', "_")).collect();
    from_skills.sort();
    assert_eq!(
        skills.len(),
        5,
        "A2A skills: {skills:?}\nMCP tools: {tools:?}"
    );
    assert_eq!(
        from_skills, tools,
        "A2A skills: {skills:?}\nMCP tools: {tools:?}"
    );
    let image = current_a2a(&endpoints, EXT).await;
    let encoded = base64::engine::general_purpose::STANDARD.encode(image.data());
    let raw = mcp_tool_text(
        &endpoints.mcp_url,
        "get_image",
        &serde_json::json!({ "id": image.descriptor().id().as_str() }),
    )
    .await;
    assert!(raw.contains(&encoded), "{raw}");
    assert!(!raw.contains("\"uri\""), "{raw}");
    let _ = endpoints.a2a_base;
}

fn json_payloads(raw: &str) -> Vec<serde_json::Value> {
    let lines: Vec<_> = raw
        .lines()
        .filter_map(|line| line.strip_prefix("data:"))
        .map(str::trim)
        .collect();
    let chunks = if lines.is_empty() {
        vec![raw.trim()]
    } else {
        lines
    };
    chunks
        .into_iter()
        .filter_map(|chunk| serde_json::from_str(chunk).ok())
        .collect()
}

async fn image_tools(url: &str) -> Vec<String> {
    let raw = mcp_raw(url, "tools/list", &serde_json::json!({})).await;
    let mut names = Vec::new();
    for value in json_payloads(&raw) {
        let Some(tools) = value
            .pointer("/result/tools")
            .and_then(|tools| tools.as_array())
        else {
            continue;
        };
        names.extend(tools.iter().filter_map(|tool| {
            tool.get("name")
                .and_then(|name| name.as_str())
                .filter(|name| name.contains("image"))
                .map(str::to_owned)
        }));
    }
    names.sort();
    names
}

async fn mcp_tool_text(url: &str, name: &str, arguments: &serde_json::Value) -> String {
    mcp_raw(
        url,
        "tools/call",
        &serde_json::json!({ "name": name, "arguments": arguments }),
    )
    .await
}

async fn mcp_raw(url: &str, method: &str, params: &serde_json::Value) -> String {
    let http = reqwest::Client::new();
    let opened = post_mcp(
        &http,
        url,
        None,
        &serde_json::json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "initialize",
            "params": {
                "protocolVersion": "2026-07-28",
                "capabilities": {},
                "clientInfo": {"name": "parity", "version": "0"}
            }
        }),
    )
    .await;
    post_mcp(
        &http,
        url,
        opened.0.as_deref(),
        &serde_json::json!({"jsonrpc": "2.0", "method": "notifications/initialized"}),
    )
    .await;
    let listed = post_mcp(
        &http,
        url,
        opened.0.as_deref(),
        &serde_json::json!({
            "jsonrpc": "2.0",
            "id": 2,
            "method": method,
            "params": params
        }),
    )
    .await;
    listed.1
}

async fn post_mcp(
    http: &reqwest::Client,
    url: &str,
    session: Option<&str>,
    body: &serde_json::Value,
) -> (Option<String>, String) {
    let mut request = http
        .post(url)
        .header("content-type", "application/json")
        .header("accept", "application/json, text/event-stream")
        .json(body);
    if let Some(session) = session {
        request = request.header("mcp-session-id", session);
    }
    let response = request.send().await.expect("mcp");
    assert!(response.status().is_success(), "{}", response.status());
    let session = response
        .headers()
        .get("mcp-session-id")
        .and_then(|value| value.to_str().ok())
        .map(str::to_owned);
    (session, response.text().await.expect("body"))
}
