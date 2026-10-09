//! OT-2 on-board camera still capture.

use std::sync::Arc;
use std::time::Duration;

use a2a_lab_dev_kit::{
    GetCurrentImageRequest, GetImageRequest, ImageProvider, ImageSourceId, ListImageSourcesRequest,
    ListImagesRequest, PageRequest,
};
use a2a_lab_ot2::opentrons::{
    OPENTRONS_CAMERA_ASSET_ID, OPENTRONS_CAMERA_SOURCE_ID, OpentronsCamera,
};
use axum::Router;
use axum::body::Body;
use axum::extract::{Request, State};
use axum::http::{HeaderValue, StatusCode, header};
use axum::middleware::{self, Next};
use axum::response::Response;
use axum::routing::post;
use tokio::net::TcpListener;
use tokio::sync::Mutex;

#[derive(Clone)]
struct Hit {
    method: String,
    path: String,
    version: Option<String>,
}

#[derive(Clone)]
struct AppState {
    hits: Arc<Mutex<Vec<Hit>>>,
    status: StatusCode,
    content_type: String,
    body: Vec<u8>,
    delay: Duration,
}

fn jpeg(width: u16, height: u16) -> Vec<u8> {
    let mut data = vec![0xFF, 0xD8, 0xFF, 0xC0, 0x00, 0x0B, 0x08];
    data.extend_from_slice(&height.to_be_bytes());
    data.extend_from_slice(&width.to_be_bytes());
    data.extend_from_slice(&[0x01, 0x01, 0x11, 0x00, 0xFF, 0xD9]);
    data
}

fn page() -> PageRequest {
    PageRequest::new(None, 10).unwrap()
}

fn source_id() -> ImageSourceId {
    ImageSourceId::new(OPENTRONS_CAMERA_SOURCE_ID).unwrap()
}

async fn serve(
    status: StatusCode,
    content_type: &str,
    body: Vec<u8>,
    delay: Duration,
) -> (String, Arc<Mutex<Vec<Hit>>>) {
    let hits = Arc::new(Mutex::new(Vec::new()));
    let state = AppState {
        hits: Arc::clone(&hits),
        status,
        content_type: content_type.to_owned(),
        body,
        delay,
    };
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let app = Router::new()
        .route("/camera/picture", post(picture))
        .fallback(other)
        .layer(middleware::from_fn_with_state(state.clone(), record))
        .with_state(state);
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    (format!("http://{address}"), hits)
}

async fn record(State(state): State<AppState>, request: Request, next: Next) -> Response {
    state.hits.lock().await.push(Hit {
        method: request.method().to_string(),
        path: request.uri().path().to_owned(),
        version: request
            .headers()
            .get("Opentrons-Version")
            .and_then(|value| value.to_str().ok())
            .map(str::to_owned),
    });
    next.run(request).await
}

async fn picture(State(state): State<AppState>) -> Response {
    if !state.delay.is_zero() {
        tokio::time::sleep(state.delay).await;
    }
    let mut response = Response::builder()
        .status(state.status)
        .body(Body::from(state.body.clone()))
        .unwrap();
    if !state.content_type.is_empty() {
        response.headers_mut().insert(
            header::CONTENT_TYPE,
            HeaderValue::from_str(&state.content_type).unwrap(),
        );
    }
    response
}

async fn other() -> StatusCode {
    StatusCode::NOT_FOUND
}

fn camera(url: &str, timeout_ms: u64, max_bytes: u64) -> OpentronsCamera {
    OpentronsCamera::new(url, Duration::from_millis(timeout_ms), max_bytes).unwrap()
}

async fn assert_only_picture(hits: &Mutex<Vec<Hit>>) {
    let hits = hits.lock().await;
    assert!(!hits.is_empty());
    for hit in hits.iter() {
        assert_eq!(hit.method, "POST");
        assert_eq!(hit.path, "/camera/picture");
        assert_eq!(hit.version.as_deref(), Some("*"));
    }
}

#[tokio::test]
async fn picture_returns_byte_identical_jpeg_metadata() {
    let bytes = jpeg(2, 3);
    let (url, hits) = serve(StatusCode::OK, "image/jpg", bytes.clone(), Duration::ZERO).await;
    let images = camera(&url, 1_000, 1024).catalog(4).unwrap();
    let first = images
        .get_current_image(GetCurrentImageRequest::new(source_id()))
        .await
        .unwrap();
    let second = images
        .get_current_image(GetCurrentImageRequest::new(source_id()))
        .await
        .unwrap();

    let sources = images
        .list_image_sources(ListImageSourcesRequest::new(page()).unwrap())
        .await
        .unwrap();
    assert_eq!(sources.items().len(), 1);
    assert_eq!(sources.items()[0].id.as_str(), OPENTRONS_CAMERA_SOURCE_ID);
    assert_eq!(
        sources.items()[0].asset_id.as_deref(),
        Some(OPENTRONS_CAMERA_ASSET_ID)
    );
    assert_eq!(first.data(), bytes.as_slice());
    assert_eq!(second.data(), bytes.as_slice());
    assert_ne!(first.descriptor().id(), second.descriptor().id());
    assert_eq!(first.descriptor().media_type(), "image/jpeg");
    assert_eq!(first.descriptor().width(), 2);
    assert_eq!(first.descriptor().height(), 3);
    assert!(
        first.descriptor().captured_at()
            > a2a_lab_dev_kit::UtcTimestamp::parse("2020-01-01T00:00:00Z").unwrap()
    );
    let stored = images
        .get_image(GetImageRequest::new(first.descriptor().id().clone()))
        .await
        .unwrap();
    assert_eq!(stored.data(), bytes.as_slice());
    assert_only_picture(&hits).await;
}

#[tokio::test]
async fn standard_jpeg_content_type_is_accepted() {
    let bytes = jpeg(4, 1);
    let (url, hits) = serve(StatusCode::OK, "image/jpeg", bytes.clone(), Duration::ZERO).await;
    let images = camera(&url, 1_000, 1024).catalog(2).unwrap();
    let image = images
        .get_current_image(GetCurrentImageRequest::new(source_id()))
        .await
        .unwrap();
    assert_eq!(image.data(), bytes.as_slice());
    assert_eq!(image.descriptor().media_type(), "image/jpeg");
    assert_eq!(image.descriptor().width(), 4);
    assert_only_picture(&hits).await;
}

async fn rejected(
    status: StatusCode,
    content_type: &str,
    body: Vec<u8>,
) -> a2a_lab_dev_kit::A2aLabError {
    let (url, hits) = serve(status, content_type, body, Duration::ZERO).await;
    let images = camera(&url, 1_000, 1024).catalog(2).unwrap();
    let error = images
        .get_current_image(GetCurrentImageRequest::new(source_id()))
        .await
        .unwrap_err();
    let listed = images
        .list_images(ListImagesRequest::new(source_id(), page()).unwrap())
        .await
        .unwrap();
    assert!(listed.items().is_empty());
    assert_only_picture(&hits).await;
    error
}

#[tokio::test]
async fn disabled_missing_and_bad_payloads_store_nothing() {
    let disabled = rejected(
        StatusCode::UNPROCESSABLE_ENTITY,
        "application/json",
        br#"{"detail":"Cannot take photo, camera is disabled."}"#.to_vec(),
    )
    .await;
    assert_eq!(disabled.code(), "invalid");
    assert!(disabled.to_string().contains("disabled"));

    let missing = rejected(
        StatusCode::INTERNAL_SERVER_ERROR,
        "application/json",
        br#"{"detail":"No video device found with device path: /dev/ot_system_camera"}"#.to_vec(),
    )
    .await;
    assert_eq!(missing.code(), "unavailable");
    assert!(missing.to_string().contains("video device"));

    let wrong_type = rejected(StatusCode::OK, "application/json", b"{}".to_vec()).await;
    assert_eq!(wrong_type.code(), "invalid");
    assert!(wrong_type.to_string().contains("expected image/jpeg"));

    let empty = rejected(StatusCode::OK, "image/jpeg", Vec::new()).await;
    assert_eq!(empty.code(), "invalid");
    assert!(empty.to_string().contains("empty body"));

    let malformed = rejected(StatusCode::OK, "image/jpg", b"not-a-jpeg".to_vec()).await;
    assert_eq!(malformed.code(), "invalid");
    assert!(malformed.to_string().contains("malformed JPEG"));
}

#[tokio::test]
async fn payload_limit_does_not_retain_the_frame() {
    let bytes = jpeg(8, 8);
    let (url, _) = serve(StatusCode::OK, "image/jpeg", bytes, Duration::ZERO).await;
    let images = camera(&url, 1_000, 8).catalog(2).unwrap();
    let error = images
        .get_current_image(GetCurrentImageRequest::new(source_id()))
        .await
        .unwrap_err();
    assert_eq!(error.code(), "invalid");
    assert!(error.to_string().contains("maximum"));
    let listed = images
        .list_images(ListImagesRequest::new(source_id(), page()).unwrap())
        .await
        .unwrap();
    assert!(listed.items().is_empty());
}

#[tokio::test]
async fn timeout_and_unreachable_camera_are_unavailable() {
    let (url, _) = serve(
        StatusCode::OK,
        "image/jpeg",
        jpeg(1, 1),
        Duration::from_secs(2),
    )
    .await;
    let images = camera(&url, 50, 1024).catalog(2).unwrap();
    let timed_out = images
        .get_current_image(GetCurrentImageRequest::new(source_id()))
        .await
        .unwrap_err();
    assert_eq!(timed_out.code(), "unavailable");
    assert!(timed_out.to_string().contains("timed out"));
    let listed = images
        .list_images(ListImagesRequest::new(source_id(), page()).unwrap())
        .await
        .unwrap();
    assert!(listed.items().is_empty());

    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let closed = listener.local_addr().unwrap();
    drop(listener);
    let offline = camera(&format!("http://{closed}"), 1_000, 1024)
        .catalog(2)
        .unwrap();
    let unreachable = offline
        .get_current_image(GetCurrentImageRequest::new(source_id()))
        .await
        .unwrap_err();
    assert_eq!(unreachable.code(), "unavailable");
    assert!(unreachable.to_string().contains("unreachable"));
}

#[test]
fn zero_timeout_is_rejected() {
    match OpentronsCamera::new("http://127.0.0.1:9", Duration::ZERO, 1024) {
        Ok(_) => panic!("zero timeout was accepted"),
        Err(error) => assert_eq!(error.code(), "invalid"),
    }
}
