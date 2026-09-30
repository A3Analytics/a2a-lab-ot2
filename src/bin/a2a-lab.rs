//! A2A lab client for the seven lab operations.

use std::str::FromStr;

use a2a_lab_sdk::{
    A2aClient, GetWorkflowStatusRequest, JsonObject, LabResult, ListLogSourcesRequest,
    ListMetricsRequest, ListWorkflowsRequest, LogLevel, LogRecord, MetricId, PageRequest,
    QueryLogsRequest, QueryMetricRequest, RunId, RunState, SourceId, StartWorkflowRequest,
    TimeRange, UtcTimestamp, WorkflowId,
};
use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(
    name = "a2a-lab",
    about = "Call the a2a-lab operations: logs, metrics, and workflows"
)]
struct Cli {
    #[arg(long, default_value = "http://127.0.0.1:31000")]
    a2a: String,
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// `list_workflows`
    ListWorkflows,
    /// `list_log_sources`
    ListLogSources,
    /// `list_metrics`
    ListMetrics,
    /// `query_logs` (omit sources to merge every advertised source into one stream)
    QueryLogs { source_ids: Vec<String> },
    /// `query_metric` (omit to query every metric)
    QueryMetrics { metric_id: Option<String> },
    /// `start_workflow` (defaults to `run_serial_dilution`)
    StartWorkflow {
        #[arg(default_value = "run_serial_dilution")]
        workflow_id: String,
        #[arg(long, default_value = "{}")]
        input: String,
    },
    /// `get_workflow_status`
    GetWorkflowStatus { run_id: String },
    /// `start_workflow pause_run`
    Pause { run_id: String },
    /// `start_workflow resume_run`
    Resume { run_id: String },
    /// `start_workflow stop_run`
    Stop { run_id: String },
    /// `start_workflow execute_command` with `home`
    Home,
    /// `start_workflow execute_command`
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
    let client = A2aClient::new(&cli.a2a)?;
    match cli.command {
        Command::ListWorkflows => list_workflows(&client).await?,
        Command::ListLogSources => list_log_sources(&client).await?,
        Command::ListMetrics => list_metrics(&client).await?,
        Command::QueryLogs { source_ids } => query_logs(&client, &source_ids).await?,
        Command::QueryMetrics { metric_id } => query_metrics(&client, metric_id.as_deref()).await?,
        Command::StartWorkflow { workflow_id, input } => {
            start(&client, &workflow_id, JsonObject::parse(&input)?).await?;
        }
        Command::GetWorkflowStatus { run_id } => get_workflow_status(&client, &run_id).await?,
        Command::Pause { run_id } => action(&client, "pause_run", &run_id).await?,
        Command::Resume { run_id } => action(&client, "resume_run", &run_id).await?,
        Command::Stop { run_id } => action(&client, "stop_run", &run_id).await?,
        Command::Home => {
            start(
                &client,
                "execute_command",
                JsonObject::parse(r#"{"commandType":"home","params":{}}"#)?,
            )
            .await?;
        }
        Command::Engine {
            command_type,
            params,
        } => {
            let input = format!(r#"{{"commandType":"{command_type}","params":{params}}}"#);
            start(&client, "execute_command", JsonObject::parse(&input)?).await?;
        }
    }
    Ok(())
}

async fn list_workflows(client: &A2aClient) -> Result<(), Box<dyn std::error::Error>> {
    let page = client
        .list_workflows(ListWorkflowsRequest {
            page: PageRequest::new(None, 50)?,
        })
        .await?;
    println!("a2a-lab list_workflows");
    for workflow in page.items() {
        println!("{}	{}", workflow.id, workflow.name);
    }
    Ok(())
}

async fn list_log_sources(client: &A2aClient) -> Result<(), Box<dyn std::error::Error>> {
    let page = client
        .list_log_sources(ListLogSourcesRequest {
            page: PageRequest::new(None, 50)?,
        })
        .await?;
    println!("a2a-lab list_log_sources");
    for source in page.items() {
        println!("{}	{}", source.id, source.name);
    }
    Ok(())
}

async fn list_metrics(client: &A2aClient) -> Result<(), Box<dyn std::error::Error>> {
    let page = client
        .list_metrics(ListMetricsRequest {
            page: PageRequest::new(None, 50)?,
        })
        .await?;
    println!("a2a-lab list_metrics");
    for metric in page.items() {
        println!("{}	{}	{}", metric.id, metric.unit, metric.name);
    }
    Ok(())
}

async fn start(
    client: &A2aClient,
    workflow: &str,
    input: JsonObject,
) -> Result<(), Box<dyn std::error::Error>> {
    println!("a2a-lab start_workflow {workflow}");
    let snapshot = client
        .start_workflow(StartWorkflowRequest {
            workflow_id: WorkflowId::new(workflow)?,
            input,
        })
        .await?;
    let LabResult::StartWorkflow(run) = snapshot.result else {
        return Err("unexpected start result".into());
    };
    println!("run_id {}", run.id);
    println!("state {}", run_state(run.state));
    if let Some(message) = run.message {
        println!("message {message}");
    }
    Ok(())
}

async fn get_workflow_status(
    client: &A2aClient,
    run_id: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    println!("a2a-lab get_workflow_status");
    let run = client
        .workflow_status(GetWorkflowStatusRequest {
            run_id: RunId::new(run_id)?,
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
    client: &A2aClient,
    workflow: &str,
    run_id: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    let input = JsonObject::parse(&format!(r#"{{"run_id":"{run_id}"}}"#))?;
    start(client, workflow, input).await
}

async fn query_logs(
    client: &A2aClient,
    source_ids: &[String],
) -> Result<(), Box<dyn std::error::Error>> {
    let sources = if source_ids.is_empty() {
        client
            .list_log_sources(ListLogSourcesRequest {
                page: PageRequest::new(None, 50)?,
            })
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
        records.extend(all_log_pages(client, &id, range).await?);
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
    client: &A2aClient,
    source_id: &str,
    range: TimeRange,
) -> Result<Vec<LogRecord>, Box<dyn std::error::Error>> {
    let mut cursor = None;
    let mut records = Vec::new();
    loop {
        let page = client
            .query_logs(QueryLogsRequest {
                source_id: SourceId::new(source_id)?,
                range,
                page: PageRequest::new(cursor, 1_000)?,
            })
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
    client: &A2aClient,
    metric_id: Option<&str>,
) -> Result<(), Box<dyn std::error::Error>> {
    println!("a2a-lab query_metric");
    let metrics = match metric_id {
        Some(id) => vec![MetricId::new(id)?],
        None => client
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
        let page = client
            .query_metric(QueryMetricRequest {
                metric_id: id.clone(),
                range,
                page: PageRequest::new(None, 10)?,
            })
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

fn run_state(state: RunState) -> &'static str {
    match state {
        RunState::Submitted => "submitted",
        RunState::Working => "working",
        RunState::Completed => "completed",
        RunState::Failed => "failed",
        RunState::Canceled => "canceled",
    }
}
