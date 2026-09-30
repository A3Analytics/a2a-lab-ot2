//! Lab providers over a persistent Opentrons robot-server.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use a2a_lab_sdk::{
    GetWorkflowStatusRequest, JsonObject, ListLogSourcesRequest, ListMetricsRequest,
    ListWorkflowsRequest, LogLevel, LogProvider, LogRecord, LogSource, MetricDescriptor, MetricId,
    MetricPoint, MetricProvider, Page, QueryLogsRequest, QueryMetricRequest, RunId, RunState,
    SdkError, SourceId, StartWorkflowRequest, WorkflowDefinition, WorkflowId, WorkflowProvider,
    WorkflowRun,
};
use serde_json::{Value, json};
use tokio::sync::Mutex;

use crate::page::slice_page;
use crate::time::{now, parse_timestamp};

use super::client::OpentronsClient;
use super::model::{Command, Run};

const RUN_WORKFLOW: &str = "run_serial_dilution";
const PAUSE_WORKFLOW: &str = "pause_run";
const RESUME_WORKFLOW: &str = "resume_run";
const STOP_WORKFLOW: &str = "stop_run";
const DELETE_WORKFLOW: &str = "delete_run";
const RECOVERY_WORKFLOW: &str = "resume_from_recovery";
const RECOVERY_FALSE_WORKFLOW: &str = "resume_from_recovery_assuming_false_positive";
const COMMAND_WORKFLOW: &str = "execute_command";
const COMMANDS_SOURCE: &str = "run_commands";
const ERRORS_SOURCE: &str = "command_errors";
const HEALTH_METRIC: &str = "healthy";
const PROGRESS_METRIC: &str = "run_progress_percent";
const COUNT_METRIC: &str = "run_command_count";

/// Shared Opentrons adapter implementing the three lab provider traits.
#[derive(Clone)]
pub struct OpentronsLab {
    client: OpentronsClient,
    protocol_path: PathBuf,
    local: Arc<Mutex<BTreeMap<String, WorkflowRun>>>,
}

impl OpentronsLab {
    /// Creates an adapter for a running robot-server.
    pub fn new(base_url: &str, protocol_path: impl Into<PathBuf>) -> Result<Self, SdkError> {
        Ok(Self {
            client: OpentronsClient::new(base_url)?,
            protocol_path: protocol_path.into(),
            local: Arc::new(Mutex::new(BTreeMap::new())),
        })
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

    async fn store(&self, run: WorkflowRun) -> WorkflowRun {
        self.local
            .lock()
            .await
            .insert(run.id.as_str().to_owned(), run.clone());
        run
    }
}

impl WorkflowProvider for OpentronsLab {
    #[allow(clippy::unused_async_trait_impl)]
    async fn list_workflows(
        &self,
        request: ListWorkflowsRequest,
    ) -> Result<Page<WorkflowDefinition>, SdkError> {
        slice_page(&definitions(), &request.page)
    }

    async fn start(&self, request: StartWorkflowRequest) -> Result<WorkflowRun, SdkError> {
        match request.workflow_id.as_str() {
            RUN_WORKFLOW => start_dilution(self, request.input).await,
            PAUSE_WORKFLOW => control(self, request, "pause", "pause").await,
            RESUME_WORKFLOW => control(self, request, "play", "resume").await,
            STOP_WORKFLOW => control(self, request, "stop", "stop").await,
            DELETE_WORKFLOW => delete_run(self, request).await,
            RECOVERY_WORKFLOW => control(self, request, "resume-from-recovery", "recovery").await,
            RECOVERY_FALSE_WORKFLOW => {
                control(
                    self,
                    request,
                    "resume-from-recovery-assuming-false-positive",
                    "recovery-false-positive",
                )
                .await
            }
            COMMAND_WORKFLOW => execute_command(self, request.input).await,
            _ => Err(SdkError::not_found(
                "workflow",
                request.workflow_id.to_string(),
            )),
        }
    }

    async fn status(&self, request: GetWorkflowStatusRequest) -> Result<WorkflowRun, SdkError> {
        if let Some(run) = self
            .local
            .lock()
            .await
            .get(request.run_id.as_str())
            .cloned()
        {
            return Ok(run);
        }
        let run = self.client.run(request.run_id.as_str()).await?;
        Ok(workflow_from_run(&run, JsonObject::empty()))
    }
}

impl LogProvider for OpentronsLab {
    #[allow(clippy::unused_async_trait_impl)]
    async fn list_sources(
        &self,
        request: ListLogSourcesRequest,
    ) -> Result<Page<LogSource>, SdkError> {
        slice_page(&log_sources(), &request.page)
    }

    async fn query(&self, request: QueryLogsRequest) -> Result<Page<LogRecord>, SdkError> {
        request.range.check()?;
        let source = request.source_id.as_str();
        if source != COMMANDS_SOURCE && source != ERRORS_SOURCE {
            return Err(SdkError::not_found("log source", source));
        }
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
    ) -> Result<Page<MetricDescriptor>, SdkError> {
        slice_page(&metrics(), &request.page)
    }

    async fn query(&self, request: QueryMetricRequest) -> Result<Page<MetricPoint>, SdkError> {
        request.range.check()?;
        let timestamp = now()?;
        if !request.range.contains(timestamp) {
            return slice_page(&[], &request.page);
        }
        let value = metric_value(&self.client, request.metric_id.as_str()).await?;
        slice_page(&[MetricPoint::new(timestamp, value)?], &request.page)
    }
}

fn definitions() -> Vec<WorkflowDefinition> {
    let mut items: Vec<WorkflowDefinition> = [
        (
            RUN_WORKFLOW,
            "Run serial dilution",
            "Upload the bundled OT-2 protocol, create a run, and play it.",
        ),
        (
            PAUSE_WORKFLOW,
            "Pause run",
            "Pause an in-progress Opentrons run.",
        ),
        (
            RESUME_WORKFLOW,
            "Resume run",
            "Resume a paused Opentrons run.",
        ),
        (STOP_WORKFLOW, "Stop run", "Stop (cancel) an Opentrons run."),
        (
            DELETE_WORKFLOW,
            "Delete run",
            "Delete an Opentrons run resource.",
        ),
        (
            RECOVERY_WORKFLOW,
            "Resume from recovery",
            "Resume protocol execution after error recovery.",
        ),
        (
            RECOVERY_FALSE_WORKFLOW,
            "Resume from recovery assuming false positive",
            "Resume after error recovery treating the error as a false positive.",
        ),
        (
            COMMAND_WORKFLOW,
            "Execute command",
            "Issue a Protocol Engine command to the simulator.",
        ),
    ]
    .into_iter()
    .map(|(id, name, description)| WorkflowDefinition {
        id: WorkflowId::new(id).expect("workflow id"),
        name: name.to_owned(),
        description: description.to_owned(),
        asset_id: Some("opentrons-ot2".to_owned()),
        semantic_id: Some("opentrons.robot-server".to_owned()),
    })
    .collect();
    items.sort_by(|left, right| left.id.as_str().cmp(right.id.as_str()));
    items
}

fn log_sources() -> Vec<LogSource> {
    let mut items: Vec<LogSource> = [
        (
            COMMANDS_SOURCE,
            "Run commands",
            "Protocol Engine commands from Opentrons runs.",
        ),
        (
            ERRORS_SOURCE,
            "Command errors",
            "Failed commands and attached error payloads.",
        ),
    ]
    .into_iter()
    .map(|(id, name, description)| LogSource {
        id: SourceId::new(id).expect("source id"),
        name: name.to_owned(),
        description: description.to_owned(),
        asset_id: Some("opentrons-ot2".to_owned()),
        semantic_id: Some("opentrons.robot-server".to_owned()),
    })
    .collect();
    items.sort_by(|left, right| left.id.as_str().cmp(right.id.as_str()));
    items
}

fn metrics() -> Vec<MetricDescriptor> {
    let mut items: Vec<MetricDescriptor> = [
        (
            HEALTH_METRIC,
            "Healthy",
            "1 when robot-server /health succeeds.",
            "count",
        ),
        (
            PROGRESS_METRIC,
            "Run progress",
            "Percent of current-run commands that have completed.",
            "percent",
        ),
        (
            COUNT_METRIC,
            "Run command count",
            "Number of commands on the current run.",
            "count",
        ),
    ]
    .into_iter()
    .map(|(id, name, description, unit)| MetricDescriptor {
        id: MetricId::new(id).expect("metric id"),
        name: name.to_owned(),
        description: description.to_owned(),
        unit: unit.to_owned(),
        asset_id: Some("opentrons-ot2".to_owned()),
        semantic_id: Some("opentrons.robot-server".to_owned()),
    })
    .collect();
    items.sort_by(|left, right| left.id.as_str().cmp(right.id.as_str()));
    items
}

async fn start_dilution(lab: &OpentronsLab, input: JsonObject) -> Result<WorkflowRun, SdkError> {
    let bytes = tokio::fs::read(&lab.protocol_path)
        .await
        .map_err(|error| SdkError::unavailable(error.to_string()))?;
    let filename = lab
        .protocol_path
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| SdkError::invalid("protocol", "path must have a file name"))?;
    let protocol = lab.client.upload_protocol(filename, bytes).await?;
    lab.client.wait_for_analysis(&protocol.id).await?;
    let run = lab.client.create_run(&protocol.id).await?;
    lab.client.run_action(&run.id, "play").await?;
    let run = lab.client.run(&run.id).await?;
    Ok(workflow_from_run(&run, input))
}

async fn control(
    lab: &OpentronsLab,
    request: StartWorkflowRequest,
    action: &str,
    prefix: &str,
) -> Result<WorkflowRun, SdkError> {
    let run_id = string_field(request.input.as_map(), "run_id")?;
    lab.client.run_action(run_id, action).await?;
    Ok(lab
        .store(completed_run(
            &format!("{prefix}-{run_id}"),
            request.workflow_id,
            request.input,
            Some(format!("{action} accepted")),
        )?)
        .await)
}

async fn delete_run(
    lab: &OpentronsLab,
    request: StartWorkflowRequest,
) -> Result<WorkflowRun, SdkError> {
    let run_id = string_field(request.input.as_map(), "run_id")?;
    lab.client.delete_run(run_id).await?;
    Ok(lab
        .store(completed_run(
            &format!("delete-{run_id}"),
            request.workflow_id,
            request.input,
            Some("deleted".to_owned()),
        )?)
        .await)
}

async fn execute_command(lab: &OpentronsLab, input: JsonObject) -> Result<WorkflowRun, SdkError> {
    let command_type = string_field(input.as_map(), "commandType")
        .or_else(|_| string_field(input.as_map(), "command_type"))?;
    if command_type.is_empty() {
        return Err(SdkError::invalid("commandType", "must not be empty"));
    }
    let params = input
        .as_map()
        .get("params")
        .cloned()
        .unwrap_or_else(|| Value::Object(serde_json::Map::new()));
    if !params.is_object() {
        return Err(SdkError::invalid("params", "must be a JSON object"));
    }
    let command = lab.client.execute_command(command_type, params).await?;
    let state = if command.status == "failed" {
        RunState::Failed
    } else {
        RunState::Completed
    };
    let run = WorkflowRun {
        id: RunId::new(format!("cmd-{}", command.id))?,
        workflow_id: WorkflowId::new(COMMAND_WORKFLOW)?,
        state,
        input,
        message: Some(format!("{} {}", command.kind, command.status)),
    };
    Ok(lab.store(run).await)
}

async fn collect_logs(client: &OpentronsClient, source: &str) -> Result<Vec<LogRecord>, SdkError> {
    let mut records = Vec::new();
    for run in client.runs().await? {
        for command in client.run_commands(&run.id).await? {
            let failed = command.status == "failed" || command.error.is_some();
            if source == ERRORS_SOURCE && !failed {
                continue;
            }
            if source == COMMANDS_SOURCE && failed {
                continue;
            }
            records.push(command_record(source, &run.id, &command)?);
        }
    }
    Ok(records)
}

fn command_record(source: &str, run_id: &str, command: &Command) -> Result<LogRecord, SdkError> {
    let stamp = command
        .completed_at
        .as_deref()
        .or(command.created_at.as_deref())
        .map_or_else(now, parse_timestamp)?;
    let message = if let Some(error) = &command.error {
        format!("{} {}: {error}", command.kind, command.status)
    } else {
        format!("{} {}", command.kind, command.status)
    };
    Ok(LogRecord {
        source_id: SourceId::new(source)?,
        timestamp: stamp,
        level: if command.status == "failed" {
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

async fn metric_value(client: &OpentronsClient, metric_id: &str) -> Result<f64, SdkError> {
    match metric_id {
        HEALTH_METRIC => Ok(f64::from(u8::from(client.health().await.is_ok()))),
        PROGRESS_METRIC | COUNT_METRIC => run_metric(client, metric_id).await,
        _ => Err(SdkError::not_found("metric", metric_id)),
    }
}

async fn run_metric(client: &OpentronsClient, metric_id: &str) -> Result<f64, SdkError> {
    let Some(run) = current_run(client).await? else {
        return Ok(0.0);
    };
    let commands = client.run_commands(&run.id).await?;
    let total = f64::from(u32::try_from(commands.len()).unwrap_or(u32::MAX));
    if metric_id == COUNT_METRIC {
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

async fn current_run(client: &OpentronsClient) -> Result<Option<Run>, SdkError> {
    Ok(client.runs().await?.into_iter().find(|run| run.current))
}

fn workflow_from_run(run: &Run, input: JsonObject) -> WorkflowRun {
    WorkflowRun {
        id: RunId::new(&run.id).expect("opentrons run id"),
        workflow_id: WorkflowId::new(RUN_WORKFLOW).expect("workflow id"),
        state: map_status(&run.status),
        input,
        message: Some(run.status.clone()),
    }
}

fn completed_run(
    id: &str,
    workflow_id: WorkflowId,
    input: JsonObject,
    message: Option<String>,
) -> Result<WorkflowRun, SdkError> {
    Ok(WorkflowRun {
        id: RunId::new(id)?,
        workflow_id,
        state: RunState::Completed,
        input,
        message,
    })
}

fn map_status(status: &str) -> RunState {
    match status {
        "idle" => RunState::Submitted,
        "succeeded" => RunState::Completed,
        "failed" => RunState::Failed,
        "stopped" | "stop-requested" => RunState::Canceled,
        _ => RunState::Working,
    }
}

fn string_field<'a>(
    map: &'a serde_json::Map<String, Value>,
    field: &'static str,
) -> Result<&'a str, SdkError> {
    map.get(field)
        .and_then(Value::as_str)
        .ok_or_else(|| SdkError::invalid(field, "must be a string"))
}
