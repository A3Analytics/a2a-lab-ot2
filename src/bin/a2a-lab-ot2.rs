//! a2a-lab operations against an OT-2 robot-server HTTP API.

use std::path::{Path, PathBuf};
use std::str::FromStr;
use std::sync::Arc;

use a2a_lab_dev_kit::sila::SilaServerHandle;
use a2a_lab_dev_kit::{
    A2aClient, A2aLabApi, A2aLabError, A2aLabService, A2aServer, AgentMessageHandler,
    DEFAULT_MCP_URL, GetTaskStatusRequest, JsonObject, ListLogSourcesRequest, ListMetricsRequest,
    ListTasksRequest, LogLevel, LogProvider, LogRecord, McpLab, McpServer, MetricId,
    MetricProvider, PageRequest, QueryLogsRequest, QueryMetricRequest, RunId, SourceId,
    StartTaskRequest, TaskId, TaskProvider, TaskRun, TaskState, TimeRange, UtcTimestamp, start_run,
};
use a2a_lab_ot2::{
    ConversationStore, DEFAULT_SILA_PORT, DEFAULT_SILA_UUID, LabAgent, ModelProvider, OidcConfig,
    OpentronsLab, SilaConfig, prepare_sila, selected_model, sila_server, with_oidc,
};
use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(
    name = "a2a-lab-ot2",
    version,
    about = "Call a2a-lab operations against an OT-2 robot-server HTTP API"
)]
struct Cli {
    /// Robot-server HTTP base URL (`GET /health`, `/runs`, `/logs`, …)
    #[arg(long, env = "OPENTRONS_URL", default_value = "http://127.0.0.1:31950")]
    opentrons_url: String,
    /// Advertise and execute only GET-backed tasks
    #[arg(long, global = true)]
    readonly: bool,
    /// A2A origin used by `agent-message`
    #[arg(long, env = "A2A_URL", default_value = "http://127.0.0.1:31000")]
    a2a_url: String,
    /// SQLite file for conversation history
    #[arg(
        long,
        env = "CONVERSATION_DB",
        default_value = ".a2a-lab-ot2/conversations.sqlite3"
    )]
    conversation_db: PathBuf,
    /// Model provider for plain-text messages
    #[arg(long, env = "MODEL_PROVIDER", default_value = "bedrock")]
    model_provider: ModelProviderArg,
    /// Model id. Defaults to the provider's small tool-capable model.
    #[arg(long, env = "MODEL")]
    model: Option<String>,
    /// Bedrock model id used when `--model` is omitted and the provider is Bedrock
    #[arg(long, env = "BEDROCK_MODEL")]
    bedrock_model: Option<String>,
    /// Rig messages retained for each A2A context
    #[arg(long, env = "HISTORY_LIMIT", default_value_t = 40)]
    history_limit: usize,
    /// Bearer token for `agent-message` when A2A requires `OpenID` Connect
    #[arg(long, env = "A2A_TOKEN")]
    a2a_token: Option<String>,
    /// `OpenID` Connect issuer. When set, A2A requires a bearer token. MCP and `SiLA` stay open.
    #[arg(long, env = "A2A_OIDC_ISSUER")]
    oidc_issuer: Option<String>,
    /// Access-token audience required when `OpenID` Connect is enabled
    #[arg(long, env = "A2A_OIDC_AUDIENCE", default_value = "a2a-lab")]
    oidc_audience: String,
    /// Access-token scope required when `OpenID` Connect is enabled
    #[arg(long, env = "A2A_OIDC_SCOPE", default_value = "a2a.invoke")]
    oidc_scope: String,
    /// `OpenID` Connect discovery document URL
    #[arg(long, env = "A2A_OIDC_DISCOVERY")]
    oidc_discovery: Option<String>,
    /// `SiLA` server UUID
    #[arg(long, env = "SILA_UUID", default_value = DEFAULT_SILA_UUID)]
    sila_uuid: String,
    /// `SiLA` bind host
    #[arg(long, env = "SILA_HOST", default_value = "127.0.0.1")]
    sila_host: String,
    /// `SiLA` bind port. `0` selects an ephemeral port.
    #[arg(long, env = "SILA_PORT", default_value_t = DEFAULT_SILA_PORT)]
    sila_port: u16,
    /// `PEM` certificate that replaces the self-signed development certificate
    #[arg(long, env = "SILA_CERT")]
    sila_cert: Option<PathBuf>,
    /// `PEM` private key for `--sila-cert`
    #[arg(long, env = "SILA_KEY")]
    sila_key: Option<PathBuf>,
    /// `PEM` CA for `--sila-cert`
    #[arg(long, env = "SILA_CA")]
    sila_ca: Option<PathBuf>,
    /// Persisted `SiLA` server name
    #[arg(long, env = "SILA_NAME_PATH", default_value = ".a2a-lab-ot2/sila-name")]
    sila_name_path: PathBuf,
    /// Persisted server-initiated `SiLA` clients
    #[arg(
        long,
        env = "SILA_CONNECTION_STORE",
        default_value = ".a2a-lab-ot2/sila-connections.json"
    )]
    sila_connection_store: PathBuf,
    /// Path written with the self-signed CA certificate
    #[arg(long, env = "SILA_CA_OUT", default_value = ".a2a-lab-ot2/sila-ca.crt")]
    sila_ca_out: PathBuf,
    /// Advertise the `SiLA` server over mDNS
    #[arg(long, env = "SILA_ANNOUNCE", default_value_t = false)]
    sila_announce: bool,
    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Clone, Copy, Debug, Default, clap::ValueEnum)]
enum ModelProviderArg {
    #[default]
    Bedrock,
    Openai,
    Anthropic,
}

impl From<ModelProviderArg> for ModelProvider {
    fn from(value: ModelProviderArg) -> Self {
        match value {
            ModelProviderArg::Bedrock => Self::Bedrock,
            ModelProviderArg::Openai => Self::OpenAi,
            ModelProviderArg::Anthropic => Self::Anthropic,
        }
    }
}

#[derive(Subcommand, Default)]
enum Command {
    /// Serve A2A, MCP, and `SiLA` backed by the OT-2 HTTP API
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
    /// `start_task`
    StartTask {
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
    /// Send plain text to the Bedrock agent over A2A
    AgentMessage {
        text: Vec<String>,
        /// Continue the conversation returned by an earlier turn
        #[arg(long)]
        context_id: Option<String>,
    },
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut cli = Cli::parse();
    let command = cli.command.take().unwrap_or_default();
    if let Command::AgentMessage { text, context_id } = &command {
        return agent_message(
            &cli.a2a_url,
            cli.a2a_token.as_deref(),
            text,
            context_id.as_deref(),
        )
        .await;
    }
    let lab = connect(&cli)?;
    match command {
        Command::Serve => serve(lab, &cli).await?,
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
        Command::AgentMessage { .. } => unreachable!("handled before the lab client connects"),
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

async fn agent_message(
    a2a_url: &str,
    token: Option<&str>,
    text: &[String],
    context_id: Option<&str>,
) -> Result<(), Box<dyn std::error::Error>> {
    let mut client = A2aClient::new(a2a_url)?;
    if let Some(token) = token.filter(|token| !token.is_empty()) {
        client = client.with_bearer_token(token);
    }
    let reply = client.agent_message(&text.join(" "), context_id).await?;
    println!("{}", reply.text);
    if let Some(task_id) = reply.task_id {
        println!("task_id {task_id}");
    }
    println!("context_id {}", reply.context_id);
    Ok(())
}

fn connect(cli: &Cli) -> Result<OpentronsLab, Box<dyn std::error::Error>> {
    Ok(OpentronsLab::new(&cli.opentrons_url)?.with_readonly(cli.readonly))
}

async fn serve(lab: OpentronsLab, cli: &Cli) -> Result<(), Box<dyn std::error::Error>> {
    let opentrons = format!("opentrons robot-server at {}", cli.opentrons_url);
    lab.client()
        .health()
        .await
        .map_err(|error| startup_error(&opentrons, error))?;
    let conversation = format!("conversation store at {}", cli.conversation_db.display());
    let store = ConversationStore::open(&cli.conversation_db, cli.history_limit)
        .await
        .map_err(|error| startup_error(&conversation, error))?;
    let service = A2aLabService::new(lab.clone(), lab.clone(), lab).share();
    let (sila, sila_ca) = start_sila(Arc::clone(&service), cli)
        .await
        .map_err(|error| startup_error("sila", error))?;
    let mcp = McpServer::new(&service);
    let mut mcp_task = AbortOnDrop(tokio::spawn(async move { mcp.serve_http(None).await }));
    let mcp_target = format!("mcp at {DEFAULT_MCP_URL}");
    let mcp_lab = tokio::select! {
        finished = mcp_task.join() => return Err(mcp_task_error(&mcp_target, finished).into()),
        connected = McpLab::connect_default() => {
            connected.map_err(|error| startup_error(&mcp_target, error))?
        }
    };
    let provider = ModelProvider::from(cli.model_provider);
    let model = selected_model(provider, cli.model.as_deref(), cli.bedrock_model.as_deref());
    let model_target = format!("{} model {model}", provider.as_str());
    let agent = LabAgent::from_provider(provider, &model, DEFAULT_MCP_URL, store)
        .await
        .map_err(|error| startup_error(&model_target, error))?;
    let mut a2a = A2aServer::new(&mcp_lab)
        .with_message_handler(Arc::new(agent) as Arc<dyn AgentMessageHandler>);
    if let Some(config) = oidc_config(cli) {
        a2a = with_oidc(a2a, &config).map_err(|error| startup_error("oidc", error))?;
        println!("OIDC         {} (A2A only)", config.issuer);
    } else {
        println!("OIDC         off");
    }
    println!("connected to {}", cli.opentrons_url);
    println!("A2A          http://127.0.0.1:31000");
    println!("MCP          {DEFAULT_MCP_URL}");
    println!("SiLA         {}", sila.local_addr());
    println!("SiLA CA      {sila_ca}");
    println!("model        {} {model}", provider.as_str());
    println!("conversation {}", cli.conversation_db.display());
    let a2a_target = "a2a at http://127.0.0.1:31000";
    tokio::select! {
        result = a2a.listen(None) => {
            result.map_err(|error| startup_error(a2a_target, error))?;
        }
        result = tokio::signal::ctrl_c() => {
            result.map_err(|error| startup_error("shutdown signal", error))?;
        }
    }
    drop(sila);
    drop(mcp_task);
    Ok(())
}

fn oidc_config(cli: &Cli) -> Option<OidcConfig> {
    let issuer = cli
        .oidc_issuer
        .as_deref()
        .map(str::trim)
        .filter(|issuer| !issuer.is_empty())?;
    Some(OidcConfig {
        issuer: issuer.to_owned(),
        audience: cli.oidc_audience.clone(),
        scope: cli.oidc_scope.clone(),
        discovery_url: cli.oidc_discovery.clone(),
    })
}

async fn start_sila(
    lab: Arc<dyn A2aLabApi>,
    cli: &Cli,
) -> Result<(SilaServerHandle, String), A2aLabError> {
    let config = sila_config(cli)?;
    let prepared = prepare_sila(&config)?;
    let address = prepared.address;
    let ca = if cli.sila_ca.is_none() {
        if let Some(parent) = cli.sila_ca_out.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|error| A2aLabError::unavailable(error.to_string()))?;
        }
        std::fs::write(&cli.sila_ca_out, &prepared.certificate.ca_pem)
            .map_err(|error| A2aLabError::unavailable(error.to_string()))?;
        cli.sila_ca_out.display().to_string()
    } else {
        cli.sila_ca
            .as_ref()
            .map(|path| path.display().to_string())
            .unwrap_or_default()
    };
    let handle = sila_server(prepared, lab, &config).serve(address).await?;
    Ok((handle, ca))
}

fn sila_config(cli: &Cli) -> Result<SilaConfig, A2aLabError> {
    let (cert_pem, key_pem, ca_pem) = match (&cli.sila_cert, &cli.sila_key, &cli.sila_ca) {
        (Some(cert), Some(key), Some(ca)) => (
            Some(read_pem(cert)?),
            Some(read_pem(key)?),
            Some(read_pem(ca)?),
        ),
        (None, None, None) => (None, None, None),
        _ => {
            return Err(A2aLabError::invalid(
                "certificate",
                "certificate, key, and CA PEM are required together",
            ));
        }
    };
    Ok(SilaConfig {
        uuid: cli.sila_uuid.clone(),
        host: cli.sila_host.clone(),
        port: cli.sila_port,
        name_path: Some(cli.sila_name_path.clone()),
        connection_store: Some(cli.sila_connection_store.clone()),
        announce: cli.sila_announce,
        cert_pem,
        key_pem,
        ca_pem,
    })
}

fn read_pem(path: &Path) -> Result<String, A2aLabError> {
    std::fs::read_to_string(path).map_err(|error| A2aLabError::unavailable(error.to_string()))
}

struct AbortOnDrop(tokio::task::JoinHandle<Result<(), A2aLabError>>);

impl AbortOnDrop {
    async fn join(&mut self) -> Result<Result<(), A2aLabError>, tokio::task::JoinError> {
        (&mut self.0).await
    }
}

impl Drop for AbortOnDrop {
    fn drop(&mut self) {
        self.0.abort();
    }
}

fn startup_error(dependency: &str, error: impl std::fmt::Display) -> A2aLabError {
    A2aLabError::unavailable(format!("{dependency}: {error}"))
}

fn mcp_task_error(
    target: &str,
    finished: Result<Result<(), A2aLabError>, tokio::task::JoinError>,
) -> A2aLabError {
    match finished {
        Ok(Ok(())) => startup_error(target, "stopped before it accepted a session"),
        Ok(Err(error)) => startup_error(target, error),
        Err(error) => startup_error(target, error),
    }
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
    print_run(&start_run(lab, request).await?);
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
    print_run(&run);
    Ok(())
}

fn print_run(run: &TaskRun) {
    println!("run_id {}", run.id);
    println!("state {}", run_state(run.state));
    if let Some(progress) = run.progress {
        println!("progress {progress}");
    }
    if let Some(message) = &run.message {
        println!("message {message}");
    }
    if let Some(result) = &run.result {
        println!("result {result}");
    }
    if let Some(kind) = &run.error_kind {
        println!("error_kind {kind}");
    }
    if let Some(identifier) = &run.error_identifier {
        println!("error_identifier {identifier}");
    }
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
