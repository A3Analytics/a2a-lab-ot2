//! OT-2 frame-set tasks and the separate media listener.
//!
//! A2A and MCP both call [`FrameSetApi::execute`]. Pixel bytes stay on the
//! media listener. This module is the downstream shell around `frame_sets`.

use std::path::Path;
use std::sync::Arc;

use a2a_lab_dev_kit::{ImageSourceId, JsonObject, TaskDefinition, TaskId, UtcTimestamp};
use axum::Router;
use axum::extract::{Path as AxumPath, Query, State};
use axum::http::{HeaderValue, StatusCode, header};
use axum::response::{Html, IntoResponse, Response};
use axum::routing::{get, post};
use serde::Deserialize;
use serde_json::{Value, json};
use tokio::sync::Mutex;

use crate::camera::CAMERA_SOURCE_ID;
use crate::frame_sets::{
    CaptureRequest, FrameChannelId, FramePage, FrameSet, FrameSetCapture, FrameSetCatalog,
    FrameSetError, FrameSetId, FrameSetMetadata, FrameSetSourceId,
};
use crate::images::{CapturedFrame, LiveImageCatalog};
use crate::time::now;

const TASK_PREFIX: &str = "ot2.frame-sets.";
const DEFAULT_TIMEOUT_MS: u64 = 20_000;
const DEFAULT_LIMIT: u32 = 20;

/// Shared frame-set operations for tasks and HTTP.
pub struct FrameSetApi {
    catalog: Mutex<FrameSetCatalog>,
    origin: String,
    images: Option<LiveImageCatalog>,
    default_timeout_ms: u64,
    max_channel_bytes: u64,
}

impl FrameSetApi {
    /// Opens a deferred catalog. Device discovery waits for a request.
    pub async fn open(
        path: &Path,
        capture: Arc<dyn FrameSetCapture>,
        retention: usize,
        max_channel_bytes: u64,
        origin: String,
        images: Option<LiveImageCatalog>,
    ) -> Result<Self, FrameSetError> {
        let catalog =
            FrameSetCatalog::open_deferred(path, capture, retention, max_channel_bytes).await?;
        Ok(Self {
            catalog: Mutex::new(catalog),
            origin,
            images,
            default_timeout_ms: DEFAULT_TIMEOUT_MS,
            max_channel_bytes,
        })
    }

    /// Runs one namespaced operation. The result contains metadata and URLs.
    pub async fn execute(&self, operation: &str, input: &Value) -> Result<Value, FrameSetError> {
        match operation {
            "list-sources" => self.list_sources().await,
            "capture-current" => self.capture_current(input).await,
            "list" => self.list(input, false).await,
            "search" => self.list(input, true).await,
            "get" => self.get(input).await,
            "get-channel" => self.get_channel(input).await,
            _ => Err(FrameSetError::invalid(
                "operation",
                format!("unknown frame-set operation {operation}"),
            )),
        }
    }

    /// Encoded bytes for one stored channel.
    pub async fn channel_bytes(
        &self,
        id: &str,
        channel_id: &str,
    ) -> Result<(String, Vec<u8>), FrameSetError> {
        let frame_id = frame_id(id)?;
        let channel_id = channel_id_of(channel_id)?;
        let catalog = self.catalog.lock().await;
        let channel = catalog.channel(&frame_id, &channel_id).await?;
        Ok((channel.media_type().to_owned(), channel.data().to_vec()))
    }

    async fn list_sources(&self) -> Result<Value, FrameSetError> {
        let catalog = self.catalog.lock().await;
        let backend = catalog.capture_backend();
        drop(catalog);
        let sources = tokio::task::spawn_blocking(move || backend.sources())
            .await
            .map_err(|error| {
                FrameSetError::Unavailable(format!("discovery task failed: {error}"))
            })??;
        let catalog = self.catalog.lock().await;
        catalog.replace_sources(sources.clone());
        Ok(json!({ "sources": sources }))
    }

    async fn capture_current(&self, input: &Value) -> Result<Value, FrameSetError> {
        let request = self.capture_request(input).await?;
        let blocked = request.clone();
        let catalog = self.catalog.lock().await;
        let backend = catalog.capture_backend();
        drop(catalog);
        let captured = tokio::task::spawn_blocking(move || backend.capture(&blocked))
            .await
            .map_err(|error| {
                FrameSetError::Unavailable(format!("capture task failed: {error}"))
            })??;
        let catalog = self.catalog.lock().await;
        let set = catalog.publish_captured(&request, captured).await?;
        drop(catalog);
        if let Err(error) = self.store_color(&set).await {
            let catalog = self.catalog.lock().await;
            catalog.remove(set.id()).await?;
            return Err(error);
        }
        self.metadata_value(&set.metadata())
    }

    async fn list(&self, input: &Value, search: bool) -> Result<Value, FrameSetError> {
        let source = optional_source(input)?;
        let cursor = input_str(input, "cursor");
        let limit = input_u64(input, "limit").unwrap_or(u64::from(DEFAULT_LIMIT));
        let limit = u32::try_from(limit).unwrap_or(DEFAULT_LIMIT);
        let query = if search {
            input_str(input, "query").unwrap_or("")
        } else {
            ""
        };
        let catalog = self.catalog.lock().await;
        let page = self
            .list_page(&catalog, source.as_ref(), query, cursor, limit)
            .await?;
        drop(catalog);
        let frame_sets = page
            .items()
            .iter()
            .map(|item| self.metadata_value(item))
            .collect::<Result<Vec<_>, _>>()?;
        Ok(json!({
            "frame_sets": frame_sets,
            "next_cursor": page.next_cursor(),
        }))
    }

    async fn get(&self, input: &Value) -> Result<Value, FrameSetError> {
        let id = required_str(input, "id")?;
        let catalog = self.catalog.lock().await;
        let set = catalog.get(&frame_id(id)?).await?;
        self.metadata_value(&set.metadata())
    }

    async fn get_channel(&self, input: &Value) -> Result<Value, FrameSetError> {
        let id = required_str(input, "id")?;
        let channel_id = required_str(input, "channel_id")?;
        let frame_id = frame_id(id)?;
        let channel_id = channel_id_of(channel_id)?;
        let catalog = self.catalog.lock().await;
        let channel = catalog.channel(&frame_id, &channel_id).await?;
        Ok(json!({
            "id": id,
            "channel_id": channel.id().as_str(),
            "media_type": channel.media_type(),
            "byte_len": channel.data().len(),
            "url": self.channel_url(id, channel.id().as_str()),
        }))
    }

    async fn capture_request(&self, input: &Value) -> Result<CaptureRequest, FrameSetError> {
        let source_id = match input_str(input, "source_id") {
            Some(id) => FrameSetSourceId::new(id)?,
            None => self.only_source().await?,
        };
        let channels = input
            .get("channels")
            .and_then(Value::as_array)
            .map(|items| {
                items
                    .iter()
                    .filter_map(Value::as_str)
                    .map(FrameChannelId::new)
                    .collect::<Result<Vec<_>, _>>()
            })
            .transpose()?
            .unwrap_or_default();
        CaptureRequest::new(
            source_id,
            channels,
            input_u64(input, "timeout_ms").unwrap_or(self.default_timeout_ms),
            input_u64(input, "max_channel_bytes").unwrap_or(self.max_channel_bytes),
        )
    }

    async fn only_source(&self) -> Result<FrameSetSourceId, FrameSetError> {
        let catalog = self.catalog.lock().await;
        let backend = catalog.capture_backend();
        drop(catalog);
        let sources = tokio::task::spawn_blocking(move || backend.sources())
            .await
            .map_err(|error| {
                FrameSetError::Unavailable(format!("discovery task failed: {error}"))
            })??;
        if sources.len() != 1 {
            return Err(FrameSetError::invalid(
                "source_id",
                "capture requires a source_id when the backend does not advertise exactly one source",
            ));
        }
        let id = sources[0].id().clone();
        self.catalog.lock().await.replace_sources(sources);
        Ok(id)
    }

    async fn store_color(&self, set: &FrameSet) -> Result<(), FrameSetError> {
        let Some(images) = &self.images else {
            return Ok(());
        };
        let Some(color) = set
            .channels()
            .iter()
            .find(|channel| channel.role().is_color())
        else {
            return Ok(());
        };
        let timestamp = match UtcTimestamp::parse(set.captured_at()) {
            Ok(timestamp) => timestamp,
            Err(_) => now()
                .map_err(|error| FrameSetError::Unavailable(format!("color timestamp: {error}")))?,
        };
        let frame = CapturedFrame::new(
            timestamp,
            color.media_type(),
            color.width(),
            color.height(),
            Some("Camera".to_owned()),
            JsonObject::empty(),
            color.data().to_vec(),
        );
        let source = ImageSourceId::new(CAMERA_SOURCE_ID)
            .map_err(|error| FrameSetError::Unavailable(format!("color source: {error}")))?;
        images.record_frame(source, frame).await.map_err(|error| {
            FrameSetError::Unavailable(format!("color image was not stored: {error}"))
        })?;
        Ok(())
    }

    fn metadata_value(&self, metadata: &FrameSetMetadata) -> Result<Value, FrameSetError> {
        let mut value = serde_json::to_value(metadata)
            .map_err(|error| FrameSetError::Unavailable(format!("metadata: {error}")))?;
        let urls: Vec<Value> = metadata
            .channels()
            .iter()
            .map(|channel| {
                json!({
                    "id": channel.id().as_str(),
                    "url": self.channel_url(metadata.id().as_str(), channel.id().as_str()),
                })
            })
            .collect();
        if let Some(object) = value.as_object_mut() {
            object.insert("channel_urls".to_owned(), Value::Array(urls));
        }
        Ok(value)
    }

    fn channel_url(&self, id: &str, channel_id: &str) -> String {
        format!(
            "{}/ot2/frame-sets/{id}/channels/{channel_id}",
            self.origin.trim_end_matches('/')
        )
    }

    async fn list_page(
        &self,
        catalog: &FrameSetCatalog,
        source_id: Option<&FrameSetSourceId>,
        query: &str,
        cursor: Option<&str>,
        limit: u32,
    ) -> Result<FramePage<FrameSetMetadata>, FrameSetError> {
        if query.is_empty() {
            match source_id {
                Some(source_id) => catalog.list(source_id, cursor, limit).await,
                None => catalog.search(None, "", cursor, limit).await,
            }
        } else {
            catalog.search(source_id, query, cursor, limit).await
        }
    }
}

/// Task definitions advertised only while a frame-set service is attached.
#[must_use]
pub fn task_definitions() -> Vec<TaskDefinition> {
    [
        (
            "list-sources",
            "List frame-set sources",
            "Advertised synchronized camera sources.",
        ),
        (
            "capture-current",
            "Capture a frame set",
            "Captures one synchronized frame set and stores it.",
        ),
        (
            "list",
            "List frame sets",
            "Lists stored frame-set metadata for one source.",
        ),
        (
            "search",
            "Search frame sets",
            "Searches stored frame-set metadata.",
        ),
        (
            "get",
            "Get a frame set",
            "Returns one stored frame set's metadata and channel URLs.",
        ),
        (
            "get-channel",
            "Get a frame-set channel",
            "Returns one channel's media type and download URL.",
        ),
    ]
    .into_iter()
    .map(|(id, name, description)| TaskDefinition {
        id: TaskId::new(format!("{TASK_PREFIX}{id}")).expect("frame-set task id"),
        name: name.to_owned(),
        description: description.to_owned(),
        asset_id: Some("opentrons-ot2".to_owned()),
        semantic_id: Some("ot2.frame-sets".to_owned()),
        input_schema: Some(r#"{"additionalProperties":true,"type":"object"}"#.to_owned()),
        output_schema: Some(r#"{"additionalProperties":true,"type":"object"}"#.to_owned()),
    })
    .collect()
}

/// Whether `task_id` belongs to this extension.
#[must_use]
pub fn is_frame_set_task(task_id: &str) -> bool {
    task_id.starts_with(TASK_PREFIX)
}

/// HTTP router for metadata, channel bytes, and the browser page.
pub fn router(api: Arc<FrameSetApi>) -> Router {
    Router::new()
        .route("/ot2/frame-sets/ui", get(ui))
        .route("/ot2/frame-sets/sources", get(sources))
        .route("/ot2/frame-sets/capture", post(capture))
        .route(
            "/ot2/frame-sets/{id}/channels/{channel}",
            get(channel_bytes),
        )
        .route("/ot2/frame-sets/{id}", get(one))
        .route("/ot2/frame-sets", get(listed))
        .with_state(api)
}

/// Binds the media listener.
pub async fn serve(
    api: Arc<FrameSetApi>,
    listener: tokio::net::TcpListener,
) -> Result<(), FrameSetError> {
    axum::serve(listener, router(api))
        .await
        .map_err(|error| FrameSetError::Unavailable(format!("frame-set listener: {error}")))
}

async fn ui() -> Html<&'static str> {
    Html(PAGE)
}

async fn sources(State(api): State<Arc<FrameSetApi>>) -> Response {
    json_result(api.execute("list-sources", &json!({})).await)
}

async fn capture(
    State(api): State<Arc<FrameSetApi>>,
    body: Result<axum::Json<Value>, axum::extract::rejection::JsonRejection>,
) -> Response {
    let input = match body {
        Ok(axum::Json(value)) => value,
        Err(_) => json!({}),
    };
    json_result(api.execute("capture-current", &input).await)
}

async fn listed(State(api): State<Arc<FrameSetApi>>, Query(query): Query<ListQuery>) -> Response {
    let input = json!({
        "source_id": query.source_id,
        "query": query.query,
        "cursor": query.cursor,
        "limit": query.limit,
    });
    let operation = if query.query.as_ref().is_some_and(|value| !value.is_empty()) {
        "search"
    } else {
        "list"
    };
    json_result(api.execute(operation, &input).await)
}

async fn one(State(api): State<Arc<FrameSetApi>>, AxumPath(id): AxumPath<String>) -> Response {
    json_result(api.execute("get", &json!({ "id": id })).await)
}

async fn channel_bytes(
    State(api): State<Arc<FrameSetApi>>,
    AxumPath((id, channel)): AxumPath<(String, String)>,
) -> Response {
    match api.channel_bytes(&id, &channel).await {
        Ok((media_type, bytes)) => bytes_response(&media_type, bytes),
        Err(error) => error_response(&error),
    }
}

fn bytes_response(media_type: &str, bytes: Vec<u8>) -> Response {
    let mut response = bytes.into_response();
    if let Ok(value) = HeaderValue::from_str(media_type) {
        response.headers_mut().insert(header::CONTENT_TYPE, value);
    }
    response
}

fn json_result(result: Result<Value, FrameSetError>) -> Response {
    match result {
        Ok(value) => axum::Json(value).into_response(),
        Err(error) => error_response(&error),
    }
}

fn error_response(error: &FrameSetError) -> Response {
    let status = match error.class() {
        "invalid" => StatusCode::BAD_REQUEST,
        "not_found" => StatusCode::NOT_FOUND,
        "malformed" | "unsynchronized" => StatusCode::UNPROCESSABLE_ENTITY,
        _ => StatusCode::SERVICE_UNAVAILABLE,
    };
    (
        status,
        axum::Json(json!({ "class": error.class(), "message": error.to_string() })),
    )
        .into_response()
}

#[derive(Debug, Deserialize, Default)]
struct ListQuery {
    source_id: Option<String>,
    query: Option<String>,
    cursor: Option<String>,
    limit: Option<u64>,
}

fn input_str<'a>(input: &'a Value, field: &str) -> Option<&'a str> {
    input
        .get(field)
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
}

fn required_str<'a>(input: &'a Value, field: &str) -> Result<&'a str, FrameSetError> {
    input_str(input, field).ok_or_else(|| FrameSetError::invalid(field_name(field), "is required"))
}

fn input_u64(input: &Value, field: &str) -> Option<u64> {
    input
        .get(field)
        .and_then(Value::as_u64)
        .filter(|value| *value > 0)
}

fn optional_source(input: &Value) -> Result<Option<FrameSetSourceId>, FrameSetError> {
    input_str(input, "source_id")
        .map(FrameSetSourceId::new)
        .transpose()
}

fn frame_id(value: &str) -> Result<FrameSetId, FrameSetError> {
    FrameSetId::new(value)
}

fn channel_id_of(value: &str) -> Result<FrameChannelId, FrameSetError> {
    FrameChannelId::new(value)
}

fn field_name(field: &str) -> &'static str {
    match field {
        "id" => "id",
        "channel_id" => "channel_id",
        "source_id" => "source_id",
        _ => "request",
    }
}

const PAGE: &str = r##"<!DOCTYPE html>
<html lang="en">
<head>
<meta charset="utf-8">
<title>OT-2 frame sets</title>
<style>
body { font-family: sans-serif; margin: 2rem; }
img, canvas { max-width: 100%; background: #111; }
figure { display: inline-block; margin-right: 1rem; vertical-align: top; }
pre { white-space: pre-wrap; }
</style>
</head>
<body>
<h1>Frame sets</h1>
<p>This page is an OT-2 extension for stored frame sets. Depth is a browser preview of the lossless PNG. Use the raw link for the 16-bit file.</p>
<button id="capture" type="button">Capture</button>
<div id="status"></div>
<div id="views"></div>
<pre id="meta"></pre>
<script>
const status = document.querySelector("#status");
const views = document.querySelector("#views");
const meta = document.querySelector("#meta");
document.querySelector("#capture").addEventListener("click", async () => {
  status.textContent = "Capturing…";
  views.replaceChildren();
  const response = await fetch("/ot2/frame-sets/capture", { method: "POST", headers: { "content-type": "application/json" }, body: "{}" });
  const body = await response.json();
  if (!response.ok) {
    status.textContent = (body.class || "error") + ": " + (body.message || response.status);
    return;
  }
  status.textContent = "Stored " + body.id;
  meta.textContent = JSON.stringify(body, null, 2);
  for (const channel of body.channel_urls || []) {
    const figure = document.createElement("figure");
    const title = document.createElement("figcaption");
    title.textContent = channel.id;
    const link = document.createElement("a");
    link.href = channel.url;
    link.textContent = " raw";
    title.append(link);
    const image = document.createElement("img");
    image.alt = channel.id;
    image.src = channel.url;
    figure.append(title, image);
    views.append(figure);
  }
});
</script>
</body>
</html>
"##;
