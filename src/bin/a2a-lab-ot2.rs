//! a2a-lab operations against an OT-2 robot-server HTTP API.

use std::path::PathBuf;
use std::str::FromStr;

use a2a_lab_sdk::{
    A2aServer, GetTaskStatusRequest, JsonObject, LabService, ListLogSourcesRequest,
    ListMetricsRequest, ListTasksRequest, LogLevel, LogProvider, LogRecord, McpLab, McpServer,
    MetricId, MetricProvider, PageRequest, QueryLogsRequest, QueryMetricRequest, RunId, SourceId,
    StartTaskRequest, TaskId, TaskProvider, TaskState, TimeRange, UtcTimestamp, start_run,
};
use a2a_lab_sdk_example::{OpentronsLab, default_protocol};
use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(
    name = "a2a-lab-ot2",
    about = "Call a2a-lab operations against an OT-2 robot-server HTTP API"
)]
struct Cli {
    /// Robot-server HTTP base URL (`GET /health`, `/runs`, `/logs`, …)
    #[arg(long, env = "OPENTRONS_URL", default_value = "http://127.0.0.1:31950")]
    opentrons_url: String,
    /// Protocol file for `start-task run_serial_dilution`
    #[arg(long, env = "OPENTRONS_PROTOCOL")]
    protocol: Option<PathBuf>,
    /// Advertise and execute only GET-backed tasks
    #[arg(long, global = true)]
    readonly: bool,
    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Subcommand, Default)]
enum Command {
    /// Serve A2A and MCP backed by the OT-2 HTTP API
    #[default]
    Serve,
    /// `list_tasks`
    ListTasks,
    /// `list_log_sources`
    ListLogSources,
    /// `list_metrics`
    ListMetrics,
    /// `query_logs` (omit sources to merge every advertised source into one stream)
    QueryLogs { source_ids: Vec<String> },
    /// `query_metric` (omit to query every metric)
    QueryMetrics { metric_id: Option<String> },
    /// `start_task` (defaults to `run_serial_dilution`)
    StartTask {
        #[arg(default_value = "run_serial_dilution")]
        task_id: String,
        #[arg(long, default_value = "{}")]
        input: String,
        /// Return as soon as the run is accepted
        #[arg(long)]
        no_wait: bool,
        /// Seconds to wait for a terminal state (default 60)
        #[arg(long)]
        timeout: Option<u32>,
    },
    /// `get_task_status`
    GetTaskStatus { run_id: String },
    /// `start_task pause_run`
    Pause { run_id: String },
    /// `start_task resume_run`
    Resume { run_id: String },
    /// `start_task stop_run`
    Stop { run_id: String },
    /// `start_task execute_command` with `home`
    Home,
    /// `start_task execute_command`
    #[command(name = "command")]
    Engine {
        command_type: String,
        #[arg(default_value = "{}")]
        params: String,
    },
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let cli = Cli::parse();
    let lab = connect(&cli)?;
    match cli.command.unwrap_or_default() {
        Command::Serve => serve(lab, &cli.opentrons_url).await?,
        Command::ListTasks => list_tasks(&lab).await?,
        Command::ListLogSources => list_log_sources(&lab).await?,
        Command::ListMetrics => list_metrics(&lab).await?,
        Command::QueryLogs { source_ids } => query_logs(&lab, &source_ids).await?,
        Command::QueryMetrics { metric_id } => query_metrics(&lab, metric_id.as_deref()).await?,
        Command::StartTask {
            task_id,
            input,
            no_wait,
            timeout,
        } => {
            start(
                &lab,
                &task_id,
                JsonObject::parse(&input)?,
                !no_wait,
                timeout,
            )
            .await?;
        }
        Command::GetTaskStatus { run_id } => get_task_status(&lab, &run_id).await?,
        Command::Pause { run_id } => action(&lab, "pause_run", &run_id).await?,
        Command::Resume { run_id } => action(&lab, "resume_run", &run_id).await?,
        Command::Stop { run_id } => action(&lab, "stop_run", &run_id).await?,
        Command::Home => {
            start(
                &lab,
                "execute_command",
                JsonObject::parse(r#"{"commandType":"home","params":{}}"#)?,
                true,
                None,
            )
            .await?;
        }
        Command::Engine {
            command_type,
            params,
        } => {
            let input = format!(r#"{{"commandType":"{command_type}","params":{params}}}"#);
            start(
                &lab,
                "execute_command",
                JsonObject::parse(&input)?,
                true,
                None,
            )
            .await?;
        }
    }
    Ok(())
}

fn connect(cli: &Cli) -> Result<OpentronsLab, Box<dyn std::error::Error>> {
    Ok(OpentronsLab::new(
        &cli.opentrons_url,
        cli.protocol.clone().unwrap_or_else(default_protocol),
    )?
    .with_readonly(cli.readonly))
}

async fn serve(lab: OpentronsLab, opentrons_url: &str) -> Result<(), Box<dyn std::error::Error>> {
    lab.client().health().await?;
    let service = LabService::new(lab.clone(), lab.clone(), lab).share();
    let mcp = McpServer::new(&service);
    let mcp_task = tokio::spawn(async move { mcp.serve_http(None).await });
    let a2a = A2aServer::new(&McpLab::connect_default().await?);
    println!("connected to {opentrons_url}");
    println!("A2A          http://127.0.0.1:31000");
    println!("MCP          http://127.0.0.1:31001/mcp");
    let result = a2a.listen(None).await;
    mcp_task.abort();
    result?;
    Ok(())
}

async fn list_tasks(lab: &OpentronsLab) -> Result<(), Box<dyn std::error::Error>> {
    let page = lab
        .list_tasks(ListTasksRequest {
            page: PageRequest::new(None, 50)?,
        })
        .await?;
    println!("a2a-lab list_tasks");
    for task in page.items() {
        println!("{}\t{}", task.id, task.name);
    }
    Ok(())
}

async fn list_log_sources(lab: &OpentronsLab) -> Result<(), Box<dyn std::error::Error>> {
    let page = LogProvider::list_sources(
        lab,
        ListLogSourcesRequest {
            page: PageRequest::new(None, 50)?,
        },
    )
    .await?;
    println!("a2a-lab list_log_sources");
    for source in page.items() {
        println!("{}\t{}", source.id, source.name);
    }
    Ok(())
}

async fn list_metrics(lab: &OpentronsLab) -> Result<(), Box<dyn std::error::Error>> {
    let page = lab
        .list_metrics(ListMetricsRequest {
            page: PageRequest::new(None, 50)?,
        })
        .await?;
    println!("a2a-lab list_metrics");
    for metric in page.items() {
        println!("{}\t{}\t{}", metric.id, metric.unit, metric.name);
    }
    Ok(())
}

async fn start(
    lab: &OpentronsLab,
    task: &str,
    input: JsonObject,
    wait: bool,
    timeout: Option<u32>,
) -> Result<(), Box<dyn std::error::Error>> {
    println!("a2a-lab start_task {task}");
    let mut request = StartTaskRequest::new(TaskId::new(task)?, input);
    request.wait = wait;
    request.timeout_seconds = timeout;
    let run = start_run(lab, request).await?;
    println!("run_id {}", run.id);
    println!("state {}", run_state(run.state));
    if let Some(message) = run.message {
        println!("message {message}");
    }
    Ok(())
}

async fn get_task_status(
    lab: &OpentronsLab,
    run_id: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    println!("a2a-lab get_task_status");
    let run = lab
        .status(GetTaskStatusRequest {
            id: RunId::new(run_id)?,
        })
        .await?;
    println!("run_id {}", run.id);
    println!("state {}", run_state(run.state));
    if let Some(message) = run.message {
        println!("message {message}");
    }
    Ok(())
}

async fn action(
    lab: &OpentronsLab,
    task: &str,
    run_id: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    let input = JsonObject::parse(&format!(r#"{{"run_id":"{run_id}"}}"#))?;
    start(lab, task, input, true, None).await
}

async fn query_logs(
    lab: &OpentronsLab,
    source_ids: &[String],
) -> Result<(), Box<dyn std::error::Error>> {
    let sources = if source_ids.is_empty() {
        LogProvider::list_sources(
            lab,
            ListLogSourcesRequest {
                page: PageRequest::new(None, 50)?,
            },
        )
        .await?
        .items()
        .iter()
        .map(|source| source.id.as_str().to_owned())
        .collect()
    } else {
        source_ids.to_vec()
    };
    let range = all_time()?;
    let mut records = Vec::new();
    for id in sources {
        records.extend(all_log_pages(lab, &id, range).await?);
    }
    records.sort_by(|left, right| {
        left.timestamp
            .cmp(&right.timestamp)
            .then_with(|| left.source_id.as_str().cmp(right.source_id.as_str()))
            .then_with(|| left.message.cmp(&right.message))
    });
    for record in records {
        println!("{}", serde_json::to_string(&otel_log_record(&record)?)?);
    }
    Ok(())
}

async fn all_log_pages(
    lab: &OpentronsLab,
    source_id: &str,
    range: TimeRange,
) -> Result<Vec<LogRecord>, Box<dyn std::error::Error>> {
    let mut cursor = None;
    let mut records = Vec::new();
    loop {
        let page = LogProvider::query(
            lab,
            QueryLogsRequest {
                source_id: SourceId::new(source_id)?,
                range,
                page: PageRequest::new(cursor, 1_000)?,
            },
        )
        .await?;
        records.extend(page.items().iter().cloned());
        match page.next_cursor() {
            Some(next) => cursor = Some(next.to_owned()),
            None => break,
        }
    }
    Ok(records)
}

async fn query_metrics(
    lab: &OpentronsLab,
    metric_id: Option<&str>,
) -> Result<(), Box<dyn std::error::Error>> {
    println!("a2a-lab query_metric");
    let metrics = match metric_id {
        Some(id) => vec![MetricId::new(id)?],
        None => lab
            .list_metrics(ListMetricsRequest {
                page: PageRequest::new(None, 10)?,
            })
            .await?
            .items()
            .iter()
            .map(|metric| metric.id.clone())
            .collect(),
    };
    let range = all_time()?;
    for id in metrics {
        let page = MetricProvider::query(
            lab,
            QueryMetricRequest {
                metric_id: id.clone(),
                range,
                page: PageRequest::new(None, 10)?,
            },
        )
        .await?;
        let value = page
            .items()
            .first()
            .map_or_else(|| "-".to_owned(), |point| point.value.to_string());
        println!("{id} {value}");
    }
    Ok(())
}

fn all_time() -> Result<TimeRange, Box<dyn std::error::Error>> {
    Ok(TimeRange::new(
        UtcTimestamp::parse("1970-01-01T00:00:00Z")?,
        UtcTimestamp::parse("2099-01-01T00:00:00Z")?,
    )?)
}

fn otel_log_record(record: &LogRecord) -> Result<serde_json::Value, Box<dyn std::error::Error>> {
    let nanos = jiff::Timestamp::from_str(&record.timestamp.to_rfc3339())?.as_nanosecond();
    let time_unix_nano = u64::try_from(nanos.max(0)).unwrap_or(u64::MAX);
    let mut attributes = vec![otel_kv(
        "source_id",
        serde_json::Value::String(record.source_id.to_string()),
    )];
    for (key, value) in record.attributes.as_map() {
        attributes.push(otel_kv(key, value.clone()));
    }
    Ok(serde_json::json!({
        "timeUnixNano": time_unix_nano.to_string(),
        "severityNumber": severity_number(record.level),
        "severityText": severity_text(record.level),
        "body": { "stringValue": record.message },
        "attributes": attributes,
    }))
}

fn otel_kv(key: &str, value: serde_json::Value) -> serde_json::Value {
    serde_json::json!({
        "key": key,
        "value": otel_any(value),
    })
}

fn otel_any(value: serde_json::Value) -> serde_json::Value {
    match value {
        serde_json::Value::String(text) => serde_json::json!({ "stringValue": text }),
        serde_json::Value::Bool(flag) => serde_json::json!({ "boolValue": flag }),
        serde_json::Value::Number(number) => {
            if let Some(integer) = number.as_i64() {
                serde_json::json!({ "intValue": integer.to_string() })
            } else if let Some(unsigned) = number.as_u64() {
                serde_json::json!({ "intValue": unsigned.to_string() })
            } else if let Some(float) = number.as_f64() {
                serde_json::json!({ "doubleValue": float })
            } else {
                serde_json::json!({ "stringValue": number.to_string() })
            }
        }
        other => serde_json::json!({ "stringValue": other.to_string() }),
    }
}

fn severity_number(level: LogLevel) -> u8 {
    match level {
        LogLevel::Trace => 1,
        LogLevel::Debug => 5,
        LogLevel::Info => 9,
        LogLevel::Warn => 13,
        LogLevel::Error => 17,
    }
}

fn severity_text(level: LogLevel) -> &'static str {
    match level {
        LogLevel::Trace => "TRACE",
        LogLevel::Debug => "DEBUG",
        LogLevel::Info => "INFO",
        LogLevel::Warn => "WARN",
        LogLevel::Error => "ERROR",
    }
}

fn run_state(state: TaskState) -> &'static str {
    match state {
        TaskState::Submitted => "submitted",
        TaskState::Working => "working",
        TaskState::Completed => "completed",
        TaskState::Failed => "failed",
        TaskState::Canceled => "canceled",
    }
}
