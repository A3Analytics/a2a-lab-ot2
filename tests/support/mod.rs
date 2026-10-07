//! Shared mock robot-server for integration tests.

use std::collections::HashMap;
use std::fs;
use std::net::SocketAddr;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use a2a_lab_dev_kit::{JsonObject, StartTaskRequest, TaskId, TaskProvider};
use a2a_lab_ot2::OpentronsLab;
use axum::Router;
use axum::body::Bytes;
use axum::extract::{Path, Query, Request, State};
use axum::http::StatusCode;
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use serde_json::{Value, json};
use tokio::net::TcpListener;
use tokio::sync::Mutex;

#[derive(Clone, Default)]
pub struct Mock {
    inner: Arc<Mutex<Inner>>,
}

#[derive(Default)]
struct Inner {
    health: bool,
    protocols: HashMap<String, Value>,
    runs: HashMap<String, Value>,
    commands: HashMap<String, Vec<Value>>,
    actions: Vec<(String, String)>,
    stateless: Vec<Value>,
    ids: AtomicU64,
}

impl Mock {
    pub fn new() -> Self {
        Self {
            inner: Arc::new(Mutex::new(Inner {
                health: true,
                ..Inner::default()
            })),
        }
    }

    pub async fn bind(&self) -> (String, SocketAddr) {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let state = self.clone();
        tokio::spawn(async move {
            axum::serve(listener, router(state)).await.unwrap();
        });
        (format!("http://{address}"), address)
    }

    #[allow(dead_code)]
    pub async fn actions(&self) -> Vec<(String, String)> {
        self.inner.lock().await.actions.clone()
    }
}

fn router(state: Mock) -> Router {
    Router::new()
        .route("/health", get(health))
        .route("/protocols", get(list_protocols).post(upload_protocol))
        .route("/protocols/{id}", get(get_protocol))
        .route("/protocols/{id}/analyses", get(list_analyses))
        .route("/runs", get(list_runs).post(create_run))
        .route(
            "/runs/{id}",
            get(get_run).patch(patch_run).delete(delete_run),
        )
        .route("/runs/{id}/actions", post(run_action))
        .route("/runs/{id}/commands", get(run_commands))
        .route("/commands", post(stateless_command).get(list_stateless))
        .route("/modules", get(empty_list))
        .route("/pipettes", get(empty_object))
        .route("/instruments", get(empty_list))
        .route("/robot/door/status", get(door_status))
        .route("/robot/lights", get(lights))
        .fallback(fallback)
        .layer(middleware::from_fn(require_version))
        .with_state(state)
}

async fn fallback(uri: axum::http::Uri) -> Response {
    if uri.path().starts_with("/logs/") {
        return not_found("log");
    }
    json!({ "data": {} }).to_string().into_response()
}

async fn require_version(request: Request, next: Next) -> Response {
    if request.headers().get("Opentrons-Version").is_none() {
        return (
            StatusCode::BAD_REQUEST,
            json!({
                "errors": [{ "id": "InvalidHeader", "title": "missing Opentrons-Version", "detail": "Opentrons-Version required" }]
            })
            .to_string(),
        )
            .into_response();
    }
    next.run(request).await
}

async fn health(State(mock): State<Mock>) -> Response {
    let inner = mock.inner.lock().await;
    if inner.health {
        json!({
            "name": "opentrons-dev",
            "api_version": "10.0.0",
            "robot_model": "OT-2 Standard",
            "disk_details": { "systemAvailableMb": 4096.0 }
        })
        .to_string()
        .into_response()
    } else {
        (
            StatusCode::SERVICE_UNAVAILABLE,
            json!({ "errors": [{ "detail": "offline" }] }).to_string(),
        )
            .into_response()
    }
}

async fn upload_protocol(State(mock): State<Mock>, _body: Bytes) -> Response {
    let mut inner = mock.inner.lock().await;
    let id = next_id(&inner.ids, "protocol");
    let protocol = json!({
        "id": id,
        "analysisSummaries": [{ "id": "analysis-1", "status": "completed" }]
    });
    inner.protocols.insert(id.clone(), protocol.clone());
    json!({ "data": protocol }).to_string().into_response()
}

async fn get_protocol(State(mock): State<Mock>, Path(id): Path<String>) -> Response {
    let inner = mock.inner.lock().await;
    match inner.protocols.get(&id) {
        Some(protocol) => json!({ "data": protocol }).to_string().into_response(),
        None => not_found("protocol"),
    }
}

async fn list_analyses(Path(id): Path<String>) -> Response {
    json!({
        "data": [{
            "id": "analysis-1",
            "status": "completed",
            "protocolId": id
        }]
    })
    .to_string()
    .into_response()
}

async fn list_runs(State(mock): State<Mock>) -> Response {
    let inner = mock.inner.lock().await;
    let runs: Vec<Value> = inner.runs.values().cloned().collect();
    json!({ "data": runs }).to_string().into_response()
}

async fn create_run(State(mock): State<Mock>, axum::Json(body): axum::Json<Value>) -> Response {
    let protocol_id = body["data"]["protocolId"]
        .as_str()
        .unwrap_or_default()
        .to_owned();
    let mut inner = mock.inner.lock().await;
    if !inner.protocols.contains_key(&protocol_id) {
        return not_found("protocol");
    }
    for run in inner.runs.values_mut() {
        run["current"] = json!(false);
    }
    let id = next_id(&inner.ids, "run");
    let run = json!({
        "id": id,
        "status": "idle",
        "current": true,
        "protocolId": protocol_id
    });
    inner.runs.insert(id.clone(), run.clone());
    inner.commands.insert(
        id.clone(),
        vec![
            json!({
                "id": "cmd-setup",
                "commandType": "loadLabware",
                "status": "succeeded",
                "createdAt": "2026-01-01T00:00:00Z",
                "completedAt": "2026-01-01T00:00:01Z"
            }),
            json!({
                "id": "cmd-fail",
                "commandType": "aspirate",
                "status": "failed",
                "error": { "errorType": "NoTipAttachedError", "detail": "no tip attached" },
                "createdAt": "2026-01-01T00:00:02Z",
                "completedAt": "2026-01-01T00:00:03Z"
            }),
        ],
    );
    json!({ "data": run }).to_string().into_response()
}

async fn get_run(State(mock): State<Mock>, Path(id): Path<String>) -> Response {
    let inner = mock.inner.lock().await;
    match inner.runs.get(&id) {
        Some(run) => json!({ "data": run }).to_string().into_response(),
        None => not_found("run"),
    }
}

async fn patch_run(
    State(mock): State<Mock>,
    Path(id): Path<String>,
    axum::Json(body): axum::Json<Value>,
) -> Response {
    let mut inner = mock.inner.lock().await;
    let Some(run) = inner.runs.get_mut(&id) else {
        return not_found("run");
    };
    if body["data"]["current"] == false {
        run["current"] = json!(false);
    }
    json!({ "data": run.clone() }).to_string().into_response()
}

async fn delete_run(State(mock): State<Mock>, Path(id): Path<String>) -> Response {
    let mut inner = mock.inner.lock().await;
    if inner.runs.remove(&id).is_none() {
        return not_found("run");
    }
    inner.commands.remove(&id);
    StatusCode::OK.into_response()
}

async fn run_action(
    State(mock): State<Mock>,
    Path(id): Path<String>,
    axum::Json(body): axum::Json<Value>,
) -> Response {
    let action = body["data"]["actionType"]
        .as_str()
        .unwrap_or_default()
        .to_owned();
    let mut inner = mock.inner.lock().await;
    let Some(run) = inner.runs.get_mut(&id) else {
        return not_found("run");
    };
    run["status"] = json!(status_for_action(&action));
    inner.actions.push((id, action.clone()));
    json!({ "data": { "id": "action-1", "actionType": action } })
        .to_string()
        .into_response()
}

async fn run_commands(State(mock): State<Mock>, Path(id): Path<String>) -> Response {
    let inner = mock.inner.lock().await;
    if !inner.runs.contains_key(&id) {
        return not_found("run");
    }
    let commands = inner.commands.get(&id).cloned().unwrap_or_default();
    json!({ "data": commands }).to_string().into_response()
}

async fn stateless_command(
    State(mock): State<Mock>,
    Query(query): Query<HashMap<String, String>>,
    axum::Json(body): axum::Json<Value>,
) -> Response {
    let _ = query;
    let command_type = body["data"]["commandType"]
        .as_str()
        .unwrap_or_default()
        .to_owned();
    if command_type.is_empty() {
        return (
            StatusCode::BAD_REQUEST,
            json!({ "errors": [{ "detail": "commandType required" }] }).to_string(),
        )
            .into_response();
    }
    let mut inner = mock.inner.lock().await;
    let id = next_id(&inner.ids, "cmd");
    let failed = command_type == "fail";
    let command = json!({
        "id": id,
        "commandType": command_type,
        "status": if failed { "failed" } else { "succeeded" },
        "error": failed.then(|| json!({
            "id": "err-fail",
            "errorType": "RoboticsControlError",
            "detail": "failed"
        })),
        "createdAt": "2026-01-01T00:00:00Z",
        "params": body["data"]["params"]
    });
    inner.stateless.push(command.clone());
    json!({ "data": command }).to_string().into_response()
}

async fn list_stateless(State(mock): State<Mock>) -> Response {
    let inner = mock.inner.lock().await;
    json!({ "data": inner.stateless.clone() })
        .to_string()
        .into_response()
}

async fn list_protocols(State(mock): State<Mock>) -> Response {
    let inner = mock.inner.lock().await;
    let protocols: Vec<Value> = inner.protocols.values().cloned().collect();
    json!({ "data": protocols }).to_string().into_response()
}

async fn empty_list() -> Response {
    json!({ "data": [] }).to_string().into_response()
}

async fn empty_object() -> Response {
    json!({ "data": {} }).to_string().into_response()
}

async fn door_status() -> Response {
    json!({ "data": { "status": "closed" } })
        .to_string()
        .into_response()
}

async fn lights() -> Response {
    json!({ "on": false }).to_string().into_response()
}

fn status_for_action(action: &str) -> &'static str {
    match action {
        "pause" => "paused",
        "stop" => "stopped",
        "play" | "resume-from-recovery" | "resume-from-recovery-assuming-false-positive" => {
            "running"
        }
        _ => "idle",
    }
}

/// Uploads a throwaway protocol and creates a current robot-server run.
pub async fn seed_run(lab: &OpentronsLab) -> String {
    let path = std::env::temp_dir().join(format!(
        "a2a-lab-ot2-seed-{}-{}.py",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock")
            .as_nanos()
    ));
    fs::write(&path, b"def run(protocol):\n    pass\n").expect("seed protocol");
    let uploaded = lab
        .start(StartTaskRequest::new(
            TaskId::new("post_protocols").expect("task id"),
            JsonObject::parse(&format!(r#"{{"path":"{}"}}"#, path.display())).expect("input"),
        ))
        .await
        .expect("upload protocol");
    let _ = fs::remove_file(&path);
    let protocol_id = result_id(&uploaded);
    let created = lab
        .start(StartTaskRequest::new(
            TaskId::new("post_runs").expect("task id"),
            JsonObject::parse(&format!(r#"{{"data":{{"protocolId":"{protocol_id}"}}}}"#))
                .expect("input"),
        ))
        .await
        .expect("create run");
    result_id(&created)
}

fn result_id(run: &a2a_lab_dev_kit::TaskRun) -> String {
    run.result
        .as_ref()
        .and_then(|value| value.as_map().get("id"))
        .and_then(serde_json::Value::as_str)
        .expect("result id")
        .to_owned()
}

fn next_id(counter: &AtomicU64, prefix: &str) -> String {
    format!("{prefix}-{}", counter.fetch_add(1, Ordering::Relaxed) + 1)
}

fn not_found(kind: &str) -> Response {
    (
        StatusCode::NOT_FOUND,
        json!({ "errors": [{ "id": "NotFound", "title": format!("{kind} missing"), "detail": kind }] })
            .to_string(),
    )
        .into_response()
}
