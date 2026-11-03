//! Pinned v10.0.0 `GET /camera/stream` metadata. Not a video source.

use a2a_lab_ot2::opentrons::{
    OT2_HLS_RELATIVE_PATH, OT2_RTMP_RELATIVE_PATH, OT2_STREAM_SETTINGS_UNAVAILABLE,
    OpentronsClient, STREAM_CAMERA_MISSING_FIXTURE, STREAM_METADATA_ENABLED_FIXTURE,
    STREAM_METADATA_ENVELOPE_FIXTURE, STREAM_METADATA_FIXTURE, STREAM_SETTINGS_OT2_FIXTURE,
    is_stream_metadata, reject_native_video_stream,
};
use axum::Router;
use axum::body::Body;
use axum::extract::{Request, State};
use axum::http::{HeaderValue, StatusCode, header};
use axum::middleware::{self, Next};
use axum::response::Response;
use axum::routing::{get, post};
use tokio::net::TcpListener;
use tokio::sync::Mutex;

use std::sync::Arc;

#[derive(Clone)]
struct Hit {
    method: String,
    path: String,
    version: Option<String>,
}

#[derive(Clone)]
struct AppState {
    hits: Arc<Mutex<Vec<Hit>>>,
    stream_status: StatusCode,
    stream_body: &'static str,
    hls_status: StatusCode,
    hls_type: &'static str,
    hls_body: &'static str,
}

async fn serve(state: AppState) -> (String, Arc<Mutex<Vec<Hit>>>) {
    let hits = Arc::clone(&state.hits);
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let app = Router::new()
        .route("/camera/stream", get(stream))
        .route("/camera/stream/settings", get(settings).post(settings))
        .route("/camera/picture", post(picture))
        .route("/hls/stream.m3u", get(hls))
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

async fn stream(State(state): State<AppState>) -> Response {
    json_response(state.stream_status, state.stream_body)
}

async fn settings() -> Response {
    json_response(StatusCode::BAD_REQUEST, STREAM_SETTINGS_OT2_FIXTURE)
}

async fn picture() -> StatusCode {
    StatusCode::INTERNAL_SERVER_ERROR
}

async fn hls(State(state): State<AppState>) -> Response {
    let mut response = Response::builder()
        .status(state.hls_status)
        .body(Body::from(state.hls_body))
        .unwrap();
    response.headers_mut().insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static(state.hls_type),
    );
    response
}

fn json_response(status: StatusCode, body: &'static str) -> Response {
    let mut response = Response::builder()
        .status(status)
        .body(Body::from(body))
        .unwrap();
    response.headers_mut().insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("application/json"),
    );
    response
}

fn ok_state(body: &'static str) -> AppState {
    AppState {
        hits: Arc::new(Mutex::new(Vec::new())),
        stream_status: StatusCode::OK,
        stream_body: body,
        hls_status: StatusCode::NOT_FOUND,
        hls_type: "text/html",
        hls_body: "not a playlist",
    }
}

#[tokio::test]
async fn stream_metadata_is_json_url_templates_not_frames() {
    let (url, hits) = serve(ok_state(STREAM_METADATA_FIXTURE)).await;
    let info = OpentronsClient::new(&url)
        .unwrap()
        .camera_stream()
        .await
        .unwrap();
    assert!(!info.enabled);
    assert_eq!(info.hls, OT2_HLS_RELATIVE_PATH);
    assert_eq!(info.rtmp, OT2_RTMP_RELATIVE_PATH);
    assert!(is_stream_metadata(&info));
    let error = reject_native_video_stream(&info);
    assert_eq!(error.code(), "unavailable");
    assert!(error.to_string().contains("metadata only"));
    assert!(
        error
            .to_string()
            .contains("POST /camera/picture is not a video source")
    );
    let hits = hits.lock().await;
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].method, "GET");
    assert_eq!(hits[0].path, "/camera/stream");
    assert_eq!(hits[0].version.as_deref(), Some("*"));
}

#[tokio::test]
async fn envelope_and_enabled_payloads_are_still_rejected() {
    let (url, _) = serve(ok_state(STREAM_METADATA_ENVELOPE_FIXTURE)).await;
    let enveloped = OpentronsClient::new(&url)
        .unwrap()
        .camera_stream()
        .await
        .unwrap();
    assert!(is_stream_metadata(&enveloped));
    assert_eq!(reject_native_video_stream(&enveloped).code(), "unavailable");

    let (url, _) = serve(ok_state(STREAM_METADATA_ENABLED_FIXTURE)).await;
    let enabled = OpentronsClient::new(&url)
        .unwrap()
        .camera_stream()
        .await
        .unwrap();
    assert!(enabled.enabled);
    assert!(is_stream_metadata(&enabled));
    assert_eq!(reject_native_video_stream(&enabled).code(), "unavailable");
}

#[tokio::test]
async fn missing_camera_and_ot2_settings_are_not_video() {
    let mut state = ok_state(STREAM_CAMERA_MISSING_FIXTURE);
    state.stream_status = StatusCode::SERVICE_UNAVAILABLE;
    let (url, _) = serve(state).await;
    let client = OpentronsClient::new(&url).unwrap();
    let missing = client.camera_stream().await.unwrap_err();
    assert_eq!(missing.code(), "unavailable");
    assert!(missing.to_string().contains("video device"));

    let settings = client
        .call("GET", "/camera/stream/settings", None)
        .await
        .unwrap_err();
    assert_eq!(settings.code(), "invalid");
    assert!(
        settings
            .to_string()
            .contains(OT2_STREAM_SETTINGS_UNAVAILABLE)
    );
}

#[tokio::test]
async fn advertised_hls_path_is_not_an_ingestible_frame_stream() {
    let (url, hits) = serve(ok_state(STREAM_METADATA_FIXTURE)).await;
    let client = OpentronsClient::new(&url).unwrap();
    let info = client.camera_stream().await.unwrap();
    let hls = client.call("GET", &info.hls, None).await.unwrap_err();
    assert_eq!(hls.code(), "not_found");
    let hits = hits.lock().await;
    assert!(hits.iter().any(|hit| hit.path == "/hls/stream.m3u"));
    assert!(!hits.iter().any(|hit| hit.path == "/camera/picture"));
}
