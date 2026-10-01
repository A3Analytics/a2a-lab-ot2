//! Lab providers over a persistent Opentrons robot-server.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use a2a_lab_dev_kit::{
    A2aLabError, GetTaskStatusRequest, JsonObject, ListLogSourcesRequest, ListMetricsRequest,
    ListTasksRequest, LogLevel, LogProvider, LogRecord, LogSource, MetricDescriptor, MetricId,
    MetricPoint, MetricProvider, Page, QueryLogsRequest, QueryMetricRequest, RunId, SourceId,
    StartTaskRequest, TaskDefinition, TaskId, TaskProvider, TaskRun, TaskState, UtcTimestamp,
};
use serde_json::{Value, json};
use tokio::sync::Mutex;

use crate::page::slice_page;
use crate::time::{now, parse_timestamp};

use super::client::OpentronsClient;
use super::inventory::{self, Entry, Kind};
use super::model::{Command, Run};

const RUN_TASK: &str = "run_serial_dilution";
const PAUSE_TASK: &str = "pause_run";
const RESUME_TASK: &str = "resume_run";
const STOP_TASK: &str = "stop_run";
const DELETE_TASK: &str = "delete_run";
const RECOVERY_TASK: &str = "resume_from_recovery";
const RECOVERY_FALSE_TASK: &str = "resume_from_recovery_assuming_false_positive";
const COMMAND_TASK: &str = "execute_command";

/// Shared Opentrons adapter implementing the three lab provider traits.
#[derive(Clone)]
pub struct OpentronsLab {
    client: OpentronsClient,
    protocol_path: PathBuf,
    local: Arc<Mutex<BTreeMap<String, TaskRun>>>,
    readonly: bool,
}

impl OpentronsLab {
    /// Creates an adapter for a running robot-server.
    pub fn new(base_url: &str, protocol_path: impl Into<PathBuf>) -> Result<Self, A2aLabError> {
        Ok(Self {
            client: OpentronsClient::new(base_url)?,
            protocol_path: protocol_path.into(),
            local: Arc::new(Mutex::new(BTreeMap::new())),
            readonly: false,
        })
    }

    /// Advertises and starts only GET-backed tasks.
    #[must_use]
    pub fn with_readonly(mut self, readonly: bool) -> Self {
        self.readonly = readonly;
        self
    }

    /// Robot-server client used by this adapter.
    #[must_use]
    pub fn client(&self) -> &OpentronsClient {
        &self.client
    }

    /// Bundled protocol path.
    #[must_use]
    pub fn protocol_path(&self) -> &Path {
        &self.protocol_path
    }

    async fn store(&self, run: TaskRun) -> TaskRun {
        self.local
            .lock()
            .await
            .insert(run.id.as_str().to_owned(), run.clone());
        run
    }
}

impl TaskProvider for OpentronsLab {
    #[allow(clippy::unused_async_trait_impl)]
    async fn list_tasks(
        &self,
        request: ListTasksRequest,
    ) -> Result<Page<TaskDefinition>, A2aLabError> {
        slice_page(&task_definitions(self.readonly), &request.page)
    }

    async fn start(&self, request: StartTaskRequest) -> Result<TaskRun, A2aLabError> {
        if self.readonly && !allowed_in_readonly(request.task_id.as_str()) {
            return Err(A2aLabError::not_found("task", request.task_id.to_string()));
        }
        match request.task_id.as_str() {
            RUN_TASK => start_dilution(self, request.input).await,
            PAUSE_TASK => control(self, request, "pause", "pause").await,
            RESUME_TASK => control(self, request, "play", "resume").await,
            STOP_TASK => control(self, request, "stop", "stop").await,
            DELETE_TASK => delete_run(self, request).await,
            RECOVERY_TASK => control(self, request, "resume-from-recovery", "recovery").await,
            RECOVERY_FALSE_TASK => {
                control(
                    self,
                    request,
                    "resume-from-recovery-assuming-false-positive",
                    "recovery-false-positive",
                )
                .await
            }
            COMMAND_TASK => execute_command(self, request.input).await,
            id => match inventory::task_entry(id) {
                Some(entry) => http_task(self, entry, request).await,
                None => Err(A2aLabError::not_found("task", request.task_id.to_string())),
            },
        }
    }

    async fn status(&self, request: GetTaskStatusRequest) -> Result<TaskRun, A2aLabError> {
        if let Some(run) = self.local.lock().await.get(request.id.as_str()).cloned() {
            return Ok(run);
        }
        let run = self.client.run(request.id.as_str()).await?;
        Ok(task_from_run(&run, JsonObject::empty()))
    }
}

impl LogProvider for OpentronsLab {
    #[allow(clippy::unused_async_trait_impl)]
    async fn list_sources(
        &self,
        request: ListLogSourcesRequest,
    ) -> Result<Page<LogSource>, A2aLabError> {
        slice_page(&catalog_sources(), &request.page)
    }

    async fn query(&self, request: QueryLogsRequest) -> Result<Page<LogRecord>, A2aLabError> {
        request.range.check()?;
        let source = request.source_id.as_str();
        let mut records = collect_logs(&self.client, source).await?;
        records.retain(|record| request.range.contains(record.timestamp));
        records.sort_by(|left, right| {
            left.timestamp
                .cmp(&right.timestamp)
                .then_with(|| left.message.cmp(&right.message))
        });
        slice_page(&records, &request.page)
    }
}

impl MetricProvider for OpentronsLab {
    #[allow(clippy::unused_async_trait_impl)]
    async fn list_metrics(
        &self,
        request: ListMetricsRequest,
    ) -> Result<Page<MetricDescriptor>, A2aLabError> {
        slice_page(&catalog_metrics(), &request.page)
    }

    async fn query(&self, request: QueryMetricRequest) -> Result<Page<MetricPoint>, A2aLabError> {
        request.range.check()?;
        let timestamp = now()?;
        if !request.range.contains(timestamp) {
            return slice_page(&[], &request.page);
        }
        let value = metric_value(&self.client, request.metric_id.as_str()).await?;
        slice_page(&[MetricPoint::new(timestamp, value)?], &request.page)
    }
}

fn task_definitions(readonly: bool) -> Vec<TaskDefinition> {
    let mut items = if readonly { Vec::new() } else { definitions() };
    items.extend(
        inventory::of_kind(Kind::Task)
            .filter(|entry| !readonly || inventory::is_read(entry))
            .map(entry_task),
    );
    items.sort_by(|left, right| left.id.as_str().cmp(right.id.as_str()));
    items
}

fn allowed_in_readonly(task_id: &str) -> bool {
    inventory::task_entry(task_id).is_some_and(inventory::is_read)
}

fn entry_task(entry: &Entry) -> TaskDefinition {
    TaskDefinition {
        id: TaskId::new(entry.id).expect("task id"),
        name: entry.name.to_owned(),
        description: entry.description.to_owned(),
        asset_id: Some("opentrons-ot2".to_owned()),
        semantic_id: Some("opentrons.robot-server".to_owned()),
    }
}

fn catalog_sources() -> Vec<LogSource> {
    let mut items: Vec<LogSource> = inventory::of_kind(Kind::Log)
        .map(|entry| LogSource {
            id: SourceId::new(entry.id).expect("source id"),
            name: entry.name.to_owned(),
            description: entry.description.to_owned(),
            asset_id: Some("opentrons-ot2".to_owned()),
            semantic_id: Some("opentrons.robot-server".to_owned()),
        })
        .collect();
    items.sort_by(|left, right| left.id.as_str().cmp(right.id.as_str()));
    items
}

fn catalog_metrics() -> Vec<MetricDescriptor> {
    let mut items: Vec<MetricDescriptor> = inventory::of_kind(Kind::Metric)
        .map(|entry| MetricDescriptor {
            id: MetricId::new(entry.id).expect("metric id"),
            name: entry.name.to_owned(),
            description: entry.description.to_owned(),
            unit: entry.unit.to_owned(),
            asset_id: Some("opentrons-ot2".to_owned()),
            semantic_id: Some("opentrons.robot-server".to_owned()),
        })
        .collect();
    items.sort_by(|left, right| left.id.as_str().cmp(right.id.as_str()));
    items
}

fn definitions() -> Vec<TaskDefinition> {
    let mut items: Vec<TaskDefinition> = [
        (
            RUN_TASK,
            "Run serial dilution",
            "Upload the bundled OT-2 protocol, create a run, and play it. The protocol fails mid-run after dropping a tip.",
        ),
        (
            PAUSE_TASK,
            "Pause run",
            "Pause an in-progress Opentrons run.",
        ),
        (
            RESUME_TASK,
            "Resume run",
            "Resume a paused Opentrons run.",
        ),
        (STOP_TASK, "Stop run", "Stop (cancel) an Opentrons run."),
        (
            DELETE_TASK,
            "Delete run",
            "Delete an Opentrons run resource.",
        ),
        (
            RECOVERY_TASK,
            "Resume from recovery",
            "Resume protocol execution after error recovery.",
        ),
        (
            RECOVERY_FALSE_TASK,
            "Resume from recovery assuming false positive",
            "Resume after error recovery treating the error as a false positive.",
        ),
        (
            COMMAND_TASK,
            "Execute command",
            "Issue a Protocol Engine command to the simulator.",
        ),
    ]
    .into_iter()
    .map(|(id, name, description)| TaskDefinition {
        id: TaskId::new(id).expect("task id"),
        name: name.to_owned(),
        description: description.to_owned(),
        asset_id: Some("opentrons-ot2".to_owned()),
        semantic_id: Some("opentrons.robot-server".to_owned()),
    })
    .collect();
    items.sort_by(|left, right| left.id.as_str().cmp(right.id.as_str()));
    items
}

async fn start_dilution(lab: &OpentronsLab, input: JsonObject) -> Result<TaskRun, A2aLabError> {
    let bytes = tokio::fs::read(&lab.protocol_path)
        .await
        .map_err(|error| A2aLabError::unavailable(error.to_string()))?;
    let filename = lab
        .protocol_path
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| A2aLabError::invalid("protocol", "path must have a file name"))?;
    let protocol = lab.client.upload_protocol(filename, bytes).await?;
    lab.client.wait_for_analysis(&protocol.id).await?;
    let run = lab.client.create_run(&protocol.id).await?;
    lab.client.run_action(&run.id, "play").await?;
    let run = lab.client.run(&run.id).await?;
    Ok(task_from_run(&run, input))
}

async fn control(
    lab: &OpentronsLab,
    request: StartTaskRequest,
    action: &str,
    prefix: &str,
) -> Result<TaskRun, A2aLabError> {
    let run_id = string_field(request.input.as_map(), "run_id")?;
    lab.client.run_action(run_id, action).await?;
    Ok(lab
        .store(completed_run(
            &format!("{prefix}-{run_id}"),
            request.task_id,
            request.input,
            Some(format!("{action} accepted")),
        )?)
        .await)
}

async fn delete_run(lab: &OpentronsLab, request: StartTaskRequest) -> Result<TaskRun, A2aLabError> {
    let run_id = string_field(request.input.as_map(), "run_id")?;
    lab.client.delete_run(run_id).await?;
    Ok(lab
        .store(completed_run(
            &format!("delete-{run_id}"),
            request.task_id,
            request.input,
            Some("deleted".to_owned()),
        )?)
        .await)
}

async fn http_task(
    lab: &OpentronsLab,
    entry: &Entry,
    request: StartTaskRequest,
) -> Result<TaskRun, A2aLabError> {
    match (entry.method, entry.path) {
        ("POST", "/protocols") => {
            upload_file_task(lab, entry, request, "files", "text/x-python").await
        }
        ("POST", "/dataFiles") => {
            upload_file_task(lab, entry, request, "file", "application/octet-stream").await
        }
        ("POST", "/wifi/keys") => {
            upload_file_task(lab, entry, request, "key", "application/octet-stream").await
        }
        _ => {
            let path = fill_path(entry.path, request.input.as_map())?;
            let path = with_query(entry.method, &path, request.input.as_map());
            let body = http_body(entry.method, request.input.as_map());
            let result = lab.client.call(entry.method, &path, body).await?;
            complete_http(lab, entry, request, result).await
        }
    }
}

async fn upload_file_task(
    lab: &OpentronsLab,
    entry: &Entry,
    request: StartTaskRequest,
    field: &str,
    mime: &str,
) -> Result<TaskRun, A2aLabError> {
    let map = request.input.as_map();
    let file_path = map
        .get("path")
        .and_then(Value::as_str)
        .map(PathBuf::from)
        .or_else(|| (entry.path == "/protocols").then(|| lab.protocol_path.clone()))
        .ok_or_else(|| A2aLabError::invalid("path", "file path required"))?;
    let bytes = tokio::fs::read(&file_path)
        .await
        .map_err(|error| A2aLabError::unavailable(error.to_string()))?;
    let filename = map
        .get("filename")
        .and_then(Value::as_str)
        .map(ToOwned::to_owned)
        .or_else(|| {
            file_path
                .file_name()
                .and_then(|name| name.to_str())
                .map(ToOwned::to_owned)
        })
        .ok_or_else(|| A2aLabError::invalid("filename", "path must have a file name"))?;
    let result = lab
        .client
        .upload_bytes::<Value>(
            entry.path.trim_start_matches('/'),
            field,
            &filename,
            bytes,
            mime,
        )
        .await?;
    complete_http(lab, entry, request, result).await
}

async fn complete_http(
    lab: &OpentronsLab,
    entry: &Entry,
    request: StartTaskRequest,
    result: Value,
) -> Result<TaskRun, A2aLabError> {
    let suffix = lab.local.lock().await.len() + 1;
    Ok(lab
        .store(completed_run(
            &format!("{}-{suffix}", entry.id),
            request.task_id,
            request.input,
            Some(result.to_string()),
        )?)
        .await)
}

fn fill_path(
    template: &str,
    input: &serde_json::Map<String, Value>,
) -> Result<String, A2aLabError> {
    let mut path = template.trim_start_matches('/').to_owned();
    while let Some(start) = path.find('{') {
        let end = path[start..]
            .find('}')
            .map(|offset| start + offset)
            .ok_or_else(|| A2aLabError::invalid("path", "unclosed path parameter"))?;
        let name = &path[start + 1..end];
        let value = path_param(input, name)?;
        path.replace_range(start..=end, value);
    }
    Ok(path)
}

fn with_query(method: &str, path: &str, input: &serde_json::Map<String, Value>) -> String {
    let mut pairs = Vec::new();
    if let Some(query) = input.get("query").and_then(Value::as_object) {
        for (key, value) in query {
            if let Some(text) = query_value(value) {
                pairs.push((key.clone(), text));
            }
        }
    }
    if let Some(seconds) = input.get("seconds").and_then(query_value) {
        pairs.push(("seconds".to_owned(), seconds));
    }
    if matches!(method, "GET" | "DELETE") {
        for (key, value) in input {
            if reserved_input(key) {
                continue;
            }
            if let Some(text) = query_value(value) {
                pairs.push((key.clone(), text));
            }
        }
    }
    if pairs.is_empty() {
        return path.to_owned();
    }
    let query = pairs
        .into_iter()
        .map(|(key, value)| format!("{key}={}", query_escape(&value)))
        .collect::<Vec<_>>()
        .join("&");
    format!("{path}?{query}")
}

fn reserved_input(key: &str) -> bool {
    matches!(
        key,
        "body"
            | "data"
            | "query"
            | "seconds"
            | "path"
            | "filename"
            | "file"
            | "id"
            | "key"
            | "serial"
            | "key_uuid"
            | "run_id"
            | "protocol_id"
            | "command_id"
            | "analysis_id"
            | "session_id"
            | "pipette_id"
            | "calibration_id"
            | "data_file_id"
            | "camera_id"
            | "subsystem"
    ) || key.ends_with("Id")
}

fn query_value(value: &Value) -> Option<String> {
    match value {
        Value::String(text) => Some(text.clone()),
        Value::Number(number) => Some(number.to_string()),
        Value::Bool(flag) => Some(flag.to_string()),
        _ => None,
    }
}

fn query_escape(value: &str) -> String {
    let mut out = String::new();
    for byte in value.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(char::from(byte));
            }
            _ => {
                let _ = std::fmt::Write::write_fmt(&mut out, format_args!("%{byte:02X}"));
            }
        }
    }
    out
}

fn path_param<'a>(
    input: &'a serde_json::Map<String, Value>,
    name: &str,
) -> Result<&'a str, A2aLabError> {
    if let Ok(value) = string_field(input, name) {
        return Ok(value);
    }
    let snake = snake_case(name);
    if snake != name
        && let Ok(value) = string_field(input, &snake)
    {
        return Ok(value);
    }
    if name.ends_with("Id") {
        if let Ok(value) = string_field(input, "id") {
            return Ok(value);
        }
        if let Ok(value) = string_field(input, "run_id") {
            return Ok(value);
        }
    }
    Err(A2aLabError::invalid(
        "input",
        format!("missing path parameter `{name}`"),
    ))
}

fn snake_case(name: &str) -> String {
    let mut out = String::new();
    for (index, ch) in name.chars().enumerate() {
        if ch.is_uppercase() {
            if index > 0 {
                out.push('_');
            }
            out.extend(ch.to_lowercase());
        } else {
            out.push(ch);
        }
    }
    out
}

fn http_body(method: &str, input: &serde_json::Map<String, Value>) -> Option<Value> {
    if matches!(method, "GET" | "DELETE") {
        return None;
    }
    if let Some(body) = input.get("body") {
        return Some(body.clone());
    }
    if let Some(data) = input.get("data") {
        return Some(json!({ "data": data.clone() }));
    }
    let mut body = input.clone();
    for key in [
        "runId",
        "run_id",
        "protocolId",
        "protocol_id",
        "commandId",
        "analysisId",
        "maintenanceRunId",
        "sessionId",
        "dataFileId",
        "pipetteId",
        "calibrationId",
        "key_uuid",
        "key",
        "serial",
        "subsystem",
        "seconds",
        "query",
        "path",
        "filename",
        "file",
        "id",
    ] {
        body.remove(key);
    }
    if body.is_empty() {
        None
    } else {
        Some(Value::Object(body))
    }
}

async fn execute_command(lab: &OpentronsLab, input: JsonObject) -> Result<TaskRun, A2aLabError> {
    let command_type = string_field(input.as_map(), "commandType")
        .or_else(|_| string_field(input.as_map(), "command_type"))?;
    if command_type.is_empty() {
        return Err(A2aLabError::invalid("commandType", "must not be empty"));
    }
    let params = input
        .as_map()
        .get("params")
        .cloned()
        .unwrap_or_else(|| Value::Object(serde_json::Map::new()));
    if !params.is_object() {
        return Err(A2aLabError::invalid("params", "must be a JSON object"));
    }
    let command = lab.client.execute_command(command_type, params).await?;
    let state = if command.status == "failed" {
        TaskState::Failed
    } else {
        TaskState::Completed
    };
    let run = TaskRun {
        id: RunId::new(format!("cmd-{}", command.id))?,
        task_id: TaskId::new(COMMAND_TASK)?,
        state,
        input,
        message: Some(format!("{} {}", command.kind, command.status)),
    };
    Ok(lab.store(run).await)
}

async fn collect_logs(
    client: &OpentronsClient,
    source: &str,
) -> Result<Vec<LogRecord>, A2aLabError> {
    match source {
        "run_commands" => collect_commands(client, false).await,
        "run_command_errors" => collect_commands(client, true).await,
        "protocol_analyses" => collect_analyses(client).await,
        "stateless_commands" => collect_stateless(client).await,
        id if journal_source(id) => collect_journal(client, id).await,
        _ => Err(A2aLabError::not_found("log source", source)),
    }
}

fn journal_source(id: &str) -> bool {
    inventory::of_kind(Kind::Log).any(|entry| entry.id == id && entry.path.starts_with("/logs/"))
}

async fn collect_commands(
    client: &OpentronsClient,
    errors_only: bool,
) -> Result<Vec<LogRecord>, A2aLabError> {
    let runs = client.runs().await?;
    let ids: Vec<String> = match runs.iter().find(|run| run.current) {
        Some(run) => vec![run.id.clone()],
        None => runs.into_iter().map(|run| run.id).collect(),
    };
    let source = if errors_only {
        "run_command_errors"
    } else {
        "run_commands"
    };
    let mut records = Vec::new();
    for id in ids {
        for command in client.run_commands(&id).await? {
            if errors_only && command.status != "failed" && command.error.is_none() {
                continue;
            }
            records.push(command_record(source, &id, &command)?);
        }
    }
    Ok(records)
}

async fn collect_analyses(client: &OpentronsClient) -> Result<Vec<LogRecord>, A2aLabError> {
    let protocols = client.call("GET", "protocols", None).await?;
    let Some(items) = protocols.as_array() else {
        return Ok(Vec::new());
    };
    let mut records = Vec::new();
    for protocol in items {
        let Some(id) = protocol.get("id").and_then(Value::as_str) else {
            continue;
        };
        let analyses = client
            .call("GET", &format!("protocols/{id}/analyses"), None)
            .await
            .unwrap_or(Value::Array(Vec::new()));
        let message = analyses.to_string();
        records.push(LogRecord {
            source_id: SourceId::new("protocol_analyses")?,
            timestamp: now()?,
            level: LogLevel::Info,
            message,
            attributes: JsonObject::try_from_value(json!({ "protocol_id": id }))?,
        });
    }
    Ok(records)
}

async fn collect_stateless(client: &OpentronsClient) -> Result<Vec<LogRecord>, A2aLabError> {
    let commands = client.call("GET", "commands", None).await?;
    let Some(items) = commands.as_array() else {
        return Ok(Vec::new());
    };
    let mut records = Vec::new();
    for command in items {
        let id = command
            .get("id")
            .and_then(Value::as_str)
            .unwrap_or("unknown");
        records.push(LogRecord {
            source_id: SourceId::new("stateless_commands")?,
            timestamp: now()?,
            level: LogLevel::Info,
            message: command.to_string(),
            attributes: JsonObject::try_from_value(json!({ "command_id": id }))?,
        });
    }
    Ok(records)
}

async fn collect_journal(
    client: &OpentronsClient,
    source: &str,
) -> Result<Vec<LogRecord>, A2aLabError> {
    let body = client.troubleshooting_log(source).await?;
    let parsed = parse_journal(source, &body)?;
    if parsed.is_empty() {
        return simulated_journal(client, source).await;
    }
    Ok(parsed)
}

async fn simulated_journal(
    client: &OpentronsClient,
    source: &str,
) -> Result<Vec<LogRecord>, A2aLabError> {
    let run = current_run(client).await?;
    let commands = match &run {
        Some(run) => client.run_commands(&run.id).await?,
        None => Vec::new(),
    };
    let failed = commands
        .iter()
        .find(|command| command.status == "failed" || command.error.is_some());
    let origin = commands.first().map_or_else(now, command_stamp)?;
    match source {
        "api.log" => simulated_api(run.as_ref(), &commands, failed, origin),
        "serial.log" => simulated_serial(failed, origin),
        "server.log" => simulated_server(run.as_ref(), origin),
        "update_server.log" => simulated_update(origin),
        _ => Ok(Vec::new()),
    }
}

fn simulated_api(
    run: Option<&Run>,
    commands: &[Command],
    failed: Option<&Command>,
    origin: UtcTimestamp,
) -> Result<Vec<LogRecord>, A2aLabError> {
    let mut records = vec![
        journal_line(
            "api.log",
            "opentrons-api",
            offset(origin, -4)?,
            LogLevel::Info,
            "hardware_control.api: ENABLE_VIRTUAL_SMOOTHIE=true; using Smoothie emulator",
        )?,
        journal_line(
            "api.log",
            "opentrons-api",
            offset(origin, -3)?,
            LogLevel::Info,
            "protocol_reader: loaded serial_dilution.py (apiLevel=2.16, robotType=OT-2)",
        )?,
        journal_line(
            "api.log",
            "opentrons-api",
            offset(origin, -2)?,
            LogLevel::Info,
            "protocol_engine: analysis complete; creating run",
        )?,
    ];
    if let Some(run) = run {
        records.push(journal_line(
            "api.log",
            "opentrons-api",
            offset(origin, -1)?,
            LogLevel::Info,
            format!(
                "protocol_engine: run {} current=true status={}",
                run.id, run.status
            ),
        )?);
    }
    for command in commands
        .iter()
        .filter(|command| command.status == "succeeded")
    {
        records.push(journal_line(
            "api.log",
            "opentrons-api",
            command_stamp(command)?,
            LogLevel::Info,
            format!(
                "protocol_engine: executing {} ({})",
                command.kind, command.status
            ),
        )?);
    }
    if let Some(command) = failed {
        records.push(journal_line(
            "api.log",
            "opentrons-api",
            command_stamp(command)?,
            LogLevel::Error,
            format!(
                "protocol_engine: {} failed: Cannot aspirate without a tip attached (NoTipAttachedError)",
                command.kind
            ),
        )?);
        records.push(journal_line(
            "api.log",
            "opentrons-api",
            command_stamp(command)?,
            LogLevel::Warn,
            "protocol_engine: run entered awaiting-recovery",
        )?);
    }
    Ok(records)
}

fn simulated_serial(
    failed: Option<&Command>,
    origin: UtcTimestamp,
) -> Result<Vec<LogRecord>, A2aLabError> {
    let mut records = vec![
        journal_line(
            "serial.log",
            "ALL_SERIAL",
            offset(origin, -4)?,
            LogLevel::Info,
            "smoothie: virtual connection opened (emulator)",
        )?,
        journal_line(
            "serial.log",
            "ALL_SERIAL",
            offset(origin, -3)?,
            LogLevel::Debug,
            "snd: M115",
        )?,
        journal_line(
            "serial.log",
            "ALL_SERIAL",
            offset(origin, -3)?,
            LogLevel::Debug,
            "recv: FIRMWARE_NAME:Smoothie FIRMWARE_VERSION:edge PROTOCOL_VERSION:1 ok",
        )?,
        journal_line(
            "serial.log",
            "ALL_SERIAL",
            offset(origin, -1)?,
            LogLevel::Info,
            "snd: G28",
        )?,
        journal_line(
            "serial.log",
            "ALL_SERIAL",
            origin,
            LogLevel::Info,
            "recv: ok",
        )?,
        journal_line(
            "serial.log",
            "ALL_SERIAL",
            origin,
            LogLevel::Debug,
            "snd: M114",
        )?,
        journal_line(
            "serial.log",
            "ALL_SERIAL",
            origin,
            LogLevel::Debug,
            "recv: ok C: X:0.00 Y:0.00 Z:0.00 A:0.00 B:0.00",
        )?,
    ];
    if let Some(command) = failed {
        records.push(journal_line(
            "serial.log",
            "ALL_SERIAL",
            command_stamp(command)?,
            LogLevel::Warn,
            "smoothie: command ignored; pipette has no tip",
        )?);
    }
    Ok(records)
}

fn simulated_server(
    run: Option<&Run>,
    origin: UtcTimestamp,
) -> Result<Vec<LogRecord>, A2aLabError> {
    let mut records = vec![
        journal_line(
            "server.log",
            "uvicorn",
            offset(origin, -6)?,
            LogLevel::Info,
            "Started server process",
        )?,
        journal_line(
            "server.log",
            "uvicorn",
            offset(origin, -5)?,
            LogLevel::Info,
            "Waiting for application startup.",
        )?,
        journal_line(
            "server.log",
            "uvicorn",
            offset(origin, -4)?,
            LogLevel::Info,
            "Application startup complete.",
        )?,
        journal_line(
            "server.log",
            "uvicorn",
            offset(origin, -4)?,
            LogLevel::Info,
            "Uvicorn running on http://127.0.0.1:31950 (Press CTRL+C to quit)",
        )?,
        journal_line(
            "server.log",
            "uvicorn",
            offset(origin, -3)?,
            LogLevel::Info,
            r#"127.0.0.1:53120 - "GET /health HTTP/1.1" 200"#,
        )?,
        journal_line(
            "server.log",
            "uvicorn",
            offset(origin, -2)?,
            LogLevel::Info,
            r#"127.0.0.1:53121 - "POST /protocols HTTP/1.1" 201"#,
        )?,
        journal_line(
            "server.log",
            "uvicorn",
            offset(origin, -1)?,
            LogLevel::Info,
            r#"127.0.0.1:53121 - "POST /runs HTTP/1.1" 201"#,
        )?,
    ];
    if let Some(run) = run {
        records.push(journal_line(
            "server.log",
            "uvicorn",
            origin,
            LogLevel::Info,
            format!(
                r#"127.0.0.1:53121 - "POST /runs/{}/actions HTTP/1.1" 201"#,
                run.id
            ),
        )?);
    }
    Ok(records)
}

fn simulated_update(origin: UtcTimestamp) -> Result<Vec<LogRecord>, A2aLabError> {
    Ok(vec![
        journal_line(
            "update_server.log",
            "opentrons-update-server",
            offset(origin, -6)?,
            LogLevel::Info,
            "update_server: starting (dev robot)",
        )?,
        journal_line(
            "update_server.log",
            "opentrons-update-server",
            offset(origin, -5)?,
            LogLevel::Warn,
            "Could not open /etc/VERSION.json - is this a dev server?",
        )?,
        journal_line(
            "update_server.log",
            "opentrons-update-server",
            origin,
            LogLevel::Info,
            "no robot update in progress",
        )?,
        journal_line(
            "update_server.log",
            "opentrons-update-server",
            origin,
            LogLevel::Debug,
            "balance check skipped",
        )?,
    ])
}

fn journal_line(
    source: &str,
    syslog: &str,
    timestamp: UtcTimestamp,
    level: LogLevel,
    message: impl Into<String>,
) -> Result<LogRecord, A2aLabError> {
    Ok(LogRecord {
        source_id: SourceId::new(source)?,
        timestamp,
        level,
        message: message.into(),
        attributes: JsonObject::try_from_value(json!({ "syslog_identifier": syslog }))?,
    })
}

fn command_stamp(command: &Command) -> Result<UtcTimestamp, A2aLabError> {
    command
        .completed_at
        .as_deref()
        .or(command.created_at.as_deref())
        .map_or_else(now, parse_timestamp)
}

fn offset(origin: UtcTimestamp, seconds: i64) -> Result<UtcTimestamp, A2aLabError> {
    crate::time::shift_seconds(origin, seconds)
}

fn parse_journal(source: &str, body: &str) -> Result<Vec<LogRecord>, A2aLabError> {
    let trimmed = body.trim();
    if trimmed.is_empty() {
        return Ok(Vec::new());
    }
    let values = if let Ok(array) = serde_json::from_str::<Vec<Value>>(trimmed) {
        array
    } else if trimmed.starts_with('{') && !trimmed.contains('\n') {
        vec![
            serde_json::from_str(trimmed)
                .map_err(|error| A2aLabError::protocol(error.to_string()))?,
        ]
    } else {
        trimmed
            .lines()
            .map(str::trim)
            .filter(|line| !line.is_empty())
            .map(|line| {
                serde_json::from_str(line).map_err(|error| A2aLabError::protocol(error.to_string()))
            })
            .collect::<Result<Vec<_>, _>>()?
    };
    values
        .iter()
        .filter(|entry| entry.get("MESSAGE").is_some() || entry.get("SYSLOG_IDENTIFIER").is_some())
        .map(|entry| journal_record(source, entry))
        .collect()
}

fn journal_record(source: &str, entry: &Value) -> Result<LogRecord, A2aLabError> {
    let message = entry
        .get("MESSAGE")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_owned();
    Ok(LogRecord {
        source_id: SourceId::new(source)?,
        timestamp: journal_timestamp(entry)?,
        level: journal_level(entry),
        message,
        attributes: JsonObject::try_from_value(json!({
            "syslog_identifier": entry.get("SYSLOG_IDENTIFIER").and_then(Value::as_str).unwrap_or("")
        }))?,
    })
}

fn journal_timestamp(entry: &Value) -> Result<UtcTimestamp, A2aLabError> {
    let Some(raw) = entry.get("__REALTIME_TIMESTAMP") else {
        return now();
    };
    let micros = raw
        .as_str()
        .and_then(|value| value.parse::<i64>().ok())
        .or_else(|| raw.as_i64());
    let Some(micros) = micros else {
        return now();
    };
    crate::time::from_unix_microseconds(micros)
}

fn journal_level(entry: &Value) -> LogLevel {
    let priority = entry
        .get("PRIORITY")
        .and_then(|value| {
            value
                .as_str()
                .and_then(|text| text.parse::<u8>().ok())
                .or_else(|| value.as_u64().and_then(|number| u8::try_from(number).ok()))
        })
        .unwrap_or(6);
    match priority {
        0..=3 => LogLevel::Error,
        4 => LogLevel::Warn,
        7 => LogLevel::Debug,
        _ => LogLevel::Info,
    }
}

fn command_message(command: &Command) -> String {
    let Some(error) = &command.error else {
        return format!("{} {}", command.kind, command.status);
    };
    let kind = error
        .get("errorType")
        .and_then(Value::as_str)
        .unwrap_or("Error");
    match error.get("detail").and_then(Value::as_str) {
        Some(detail) if !detail.is_empty() => {
            format!("{} {}: {kind}: {detail}", command.kind, command.status)
        }
        _ => format!("{} {}: {kind}", command.kind, command.status),
    }
}

fn command_record(source: &str, run_id: &str, command: &Command) -> Result<LogRecord, A2aLabError> {
    let stamp = command
        .completed_at
        .as_deref()
        .or(command.created_at.as_deref())
        .map_or_else(now, parse_timestamp)?;
    let message = command_message(command);
    Ok(LogRecord {
        source_id: SourceId::new(source)?,
        timestamp: stamp,
        level: if command.status == "failed" || command.error.is_some() {
            LogLevel::Error
        } else {
            LogLevel::Info
        },
        message,
        attributes: JsonObject::try_from_value(json!({
            "run_id": run_id,
            "command_id": command.id
        }))?,
    })
}

async fn metric_value(client: &OpentronsClient, metric_id: &str) -> Result<f64, A2aLabError> {
    match metric_id {
        "healthy" => Ok(f64::from(u8::from(client.health().await.is_ok()))),
        "disk_available_mb" => gauge(client, "GET", "health", disk_available).await,
        "run_progress_percent" | "run_command_count" => run_metric(client, metric_id).await,
        "door_open" => gauge(client, "GET", "robot/door/status", door_open).await,
        "lights_on" => gauge(client, "GET", "robot/lights", lights_on).await,
        "pipette_count" => gauge(client, "GET", "pipettes", count_attached).await,
        "instrument_count" => gauge(client, "GET", "instruments", count_attached).await,
        "module_count" => gauge(client, "GET", "modules", count_attached).await,
        _ => Err(A2aLabError::not_found("metric", metric_id)),
    }
}

async fn gauge(
    client: &OpentronsClient,
    method: &str,
    path: &str,
    pick: fn(&Value) -> f64,
) -> Result<f64, A2aLabError> {
    let value = client.call(method, path, None).await.unwrap_or(Value::Null);
    Ok(pick(&value))
}

fn disk_available(value: &Value) -> f64 {
    value
        .get("disk_details")
        .or_else(|| value.get("diskDetails"))
        .and_then(|details| {
            details
                .get("systemAvailableMb")
                .or_else(|| details.get("system_available_mb"))
        })
        .and_then(Value::as_f64)
        .unwrap_or(0.0)
}

fn door_open(value: &Value) -> f64 {
    let status = value
        .get("status")
        .or_else(|| value.get("data").and_then(|data| data.get("status")))
        .and_then(Value::as_str)
        .unwrap_or("");
    f64::from(u8::from(status.eq_ignore_ascii_case("open")))
}

fn lights_on(value: &Value) -> f64 {
    let on = value
        .get("on")
        .or_else(|| value.get("data").and_then(|data| data.get("on")))
        .and_then(Value::as_bool)
        .unwrap_or(false);
    f64::from(u8::from(on))
}

fn count_attached(value: &Value) -> f64 {
    let len = match value {
        Value::Array(items) => items.len(),
        Value::Object(map) => {
            if let Some(items) = map.get("data").and_then(Value::as_array) {
                items.len()
            } else {
                ["left", "right"]
                    .iter()
                    .filter(|key| map.get(**key).is_some_and(|item| !item.is_null()))
                    .count()
            }
        }
        _ => 0,
    };
    f64::from(u32::try_from(len).unwrap_or(u32::MAX))
}

async fn run_metric(client: &OpentronsClient, metric_id: &str) -> Result<f64, A2aLabError> {
    let Some(run) = current_run(client).await? else {
        return Ok(0.0);
    };
    let commands = client.run_commands(&run.id).await?;
    let total = f64::from(u32::try_from(commands.len()).unwrap_or(u32::MAX));
    if metric_id == "run_command_count" {
        return Ok(total);
    }
    if total == 0.0 {
        return Ok(0.0);
    }
    let done = f64::from(
        u32::try_from(
            commands
                .iter()
                .filter(|command| matches!(command.status.as_str(), "succeeded" | "failed"))
                .count(),
        )
        .unwrap_or(u32::MAX),
    );
    Ok((done / total) * 100.0)
}

async fn current_run(client: &OpentronsClient) -> Result<Option<Run>, A2aLabError> {
    Ok(client.runs().await?.into_iter().find(|run| run.current))
}

fn task_from_run(run: &Run, input: JsonObject) -> TaskRun {
    TaskRun {
        id: RunId::new(&run.id).expect("opentrons run id"),
        task_id: TaskId::new(RUN_TASK).expect("task id"),
        state: map_status(&run.status),
        input,
        message: Some(run.status.clone()),
    }
}

fn completed_run(
    id: &str,
    task_id: TaskId,
    input: JsonObject,
    message: Option<String>,
) -> Result<TaskRun, A2aLabError> {
    Ok(TaskRun {
        id: RunId::new(id)?,
        task_id,
        state: TaskState::Completed,
        input,
        message,
    })
}

fn map_status(status: &str) -> TaskState {
    match status {
        "idle" => TaskState::Submitted,
        "succeeded" => TaskState::Completed,
        "failed" => TaskState::Failed,
        "stopped" | "stop-requested" => TaskState::Canceled,
        _ => TaskState::Working,
    }
}

fn string_field<'a>(
    map: &'a serde_json::Map<String, Value>,
    field: &str,
) -> Result<&'a str, A2aLabError> {
    map.get(field)
        .and_then(Value::as_str)
        .ok_or_else(|| A2aLabError::invalid("input", format!("{field} must be a string")))
}
