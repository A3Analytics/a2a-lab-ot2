//! A2A lab client for the seven lab operations.

use a2a_lab_sdk::{
    A2aClient, GetWorkflowStatusRequest, JsonObject, LabResult, ListLogSourcesRequest,
    ListMetricsRequest, ListWorkflowsRequest, PageRequest, QueryLogsRequest, QueryMetricRequest,
    RunId, RunState, SourceId, StartWorkflowRequest, TimeRange, UtcTimestamp, WorkflowId,
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
    /// `query_logs` for every advertised source
    QueryLogs,
    /// `query_metric` for every advertised metric
    QueryMetrics,
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
        Command::QueryLogs => query_logs(&client).await?,
        Command::QueryMetrics => query_metrics(&client).await?,
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

async fn query_logs(client: &A2aClient) -> Result<(), Box<dyn std::error::Error>> {
    println!("a2a-lab query_logs");
    let sources = client
        .list_log_sources(ListLogSourcesRequest {
            page: PageRequest::new(None, 10)?,
        })
        .await?;
    let range = TimeRange::new(
        UtcTimestamp::parse("1970-01-01T00:00:00Z")?,
        UtcTimestamp::parse("2099-01-01T00:00:00Z")?,
    )?;
    for source in sources.items() {
        let page = client
            .query_logs(QueryLogsRequest {
                source_id: SourceId::new(source.id.as_str())?,
                range,
                page: PageRequest::new(None, 20)?,
            })
            .await?;
        println!("{} ({} records)", source.id, page.items().len());
        for record in page.items() {
            println!("  {} {}", record.timestamp, record.message);
        }
    }
    Ok(())
}

async fn query_metrics(client: &A2aClient) -> Result<(), Box<dyn std::error::Error>> {
    println!("a2a-lab query_metric");
    let descriptors = client
        .list_metrics(ListMetricsRequest {
            page: PageRequest::new(None, 10)?,
        })
        .await?;
    let range = TimeRange::new(
        UtcTimestamp::parse("1970-01-01T00:00:00Z")?,
        UtcTimestamp::parse("2099-01-01T00:00:00Z")?,
    )?;
    for metric in descriptors.items() {
        let page = client
            .query_metric(QueryMetricRequest {
                metric_id: metric.id.clone(),
                range,
                page: PageRequest::new(None, 10)?,
            })
            .await?;
        let value = page
            .items()
            .first()
            .map_or_else(|| "-".to_owned(), |point| point.value.to_string());
        println!("{} {} {}", metric.id, metric.unit, value);
    }
    Ok(())
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
