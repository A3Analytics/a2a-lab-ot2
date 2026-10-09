//! Deterministic, dependency-free fixture for A2A-LAB compliance.

use std::future::Future;
use std::net::SocketAddr;
use std::path::{Path as FsPath, PathBuf};
use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::{collections::BTreeMap, fmt::Write as _};

use a2a_lab_dev_kit::{
    A2aClient, A2aLabApi, A2aLabCommand, A2aLabError, A2aLabFuture, A2aLabOutcome, A2aLabResult,
    A2aLabService, A2aServer, AgentMessageHandler, COMPLIANCE_PROFILE_VERSION, ComplianceFixtures,
    FixtureCapability, GetCurrentImageRequest, GetImageRequest, GetTaskStatusRequest, Image,
    ImageDescriptor, ImageFixtures, ImageProvider, ImageSource, ImageSourceId, JsonObject,
    ListImageSourcesRequest, ListImagesRequest, ListMetricsRequest, LogFixtures, McpLab, McpServer,
    MetricDescriptor, MetricFixtures, MetricPoint, MetricProvider, Page, QueryMetricRequest, RunId,
    SearchImagesRequest, SourceId, TaskFixtures, TaskId, TaskSnapshot, TimeRange, UtcTimestamp,
};
use axum::Router;
use axum::extract::{Path, Request};
use axum::http::StatusCode;
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use rig::completion::{
    AssistantContent, CompletionError, CompletionModel, CompletionRequest, CompletionResponse,
    Message, Usage,
};
use rig::message::UserContent;
use rig::streaming::StreamingCompletionResponse;
use serde::{Deserialize, Serialize};
use serde_json::json;
use tokio::net::TcpListener;
use tokio::task::JoinHandle;

use crate::{
    CapturedFrame, ConversationStore, ImageCapture, ImageCatalogConfig, LabAgent, LiveImageCatalog,
    OpentronsLab,
};

const PRIMARY_IMAGE_SOURCE: &str = "fixture-camera-primary";
const ALTERNATE_IMAGE_SOURCE: &str = "fixture-camera-secondary";
const FIXTURE_RUN: &str = "fixture-run";
const RANGE_START: &str = "1970-01-01T00:00:00Z";
const RANGE_END: &str = "2100-01-01T00:00:00Z";
const HISTORY_LIMIT: usize = 40;
const TASK_ID_MARKER: &str = "exact task identifier `";
static NEXT_FIXTURE: AtomicU64 = AtomicU64::new(0);

/// Controlled behavior used to prove that compliance failures are detected.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum FixtureVariant {
    /// Both protocol adapters expose the same lab behavior.
    #[default]
    Standard,
    /// MCP returns a wrong error for otherwise valid metric queries.
    McpMetricError,
    /// MCP returns an empty log-source discovery page.
    McpEmptyLogSources,
    /// The fake model uses the tool but omits the discovered task identifier.
    UngroundedAgentReply,
}

impl std::str::FromStr for FixtureVariant {
    type Err = String;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "standard" => Ok(Self::Standard),
            "mcp-metric-error" => Ok(Self::McpMetricError),
            "mcp-empty-log-sources" => Ok(Self::McpEmptyLogSources),
            "ungrounded-agent-reply" => Ok(Self::UngroundedAgentReply),
            _ => Err(format!(
                "unknown fixture variant `{value}`; expected standard, mcp-metric-error, or \
                 mcp-empty-log-sources, or ungrounded-agent-reply"
            )),
        }
    }
}

/// Network configuration for one isolated fixture process.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FixtureConfig {
    /// Interface used by both protocol listeners.
    pub host: String,
    /// A2A port; zero allocates an ephemeral port.
    pub a2a_port: u16,
    /// MCP port; zero allocates an ephemeral port.
    pub mcp_port: u16,
    /// Optional controlled mismatch for compliance evidence.
    pub variant: FixtureVariant,
}

impl Default for FixtureConfig {
    fn default() -> Self {
        Self {
            host: "127.0.0.1".to_owned(),
            a2a_port: 0,
            mcp_port: 0,
            variant: FixtureVariant::Standard,
        }
    }
}

/// Machine-readable signal emitted after both protocol listeners are bound.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FixtureReadiness {
    /// Stable readiness discriminator.
    pub status: String,
    /// Bound A2A origin.
    pub a2a_url: String,
    /// Bound MCP endpoint.
    pub mcp_url: String,
    /// Validated fixtures consumed by the compliance runner.
    pub fixtures: ComplianceFixtures,
}

/// Running isolated fixture. Dropping it stops every fixture-owned listener.
pub struct FixtureServer {
    readiness: FixtureReadiness,
    conversation_dir: PathBuf,
    model_invocations: Arc<AtomicUsize>,
    tasks: Vec<JoinHandle<()>>,
}

impl FixtureServer {
    /// Binds and starts a deterministic robot substitute and real A2A/MCP adapters.
    pub async fn start(config: FixtureConfig) -> Result<Self, A2aLabError> {
        let images = fixture_images().await?;
        let fixtures = fixture_declaration();
        fixtures
            .validate()
            .map_err(|error| A2aLabError::invalid("fixtures", error.to_string()))?;
        let robot_listener = bind("fixture robot-server", "127.0.0.1", 0).await?;
        let robot_address = local_addr("fixture robot-server", &robot_listener)?;
        let mcp_listener = bind("fixture MCP", &config.host, config.mcp_port).await?;
        let mcp_address = local_addr("fixture MCP", &mcp_listener)?;
        let a2a_listener = bind("fixture A2A", &config.host, config.a2a_port).await?;
        let a2a_address = local_addr("fixture A2A", &a2a_listener)?;

        let robot_url = format!("http://{robot_address}");
        let lab = OpentronsLab::new(&robot_url)?;
        let service = A2aLabService::new(lab.clone(), StableMetrics::new(lab.clone()), lab)
            .with_images(images)
            .share();

        let robot_task = tokio::spawn(async move {
            let _ = axum::serve(robot_listener, robot_router()).await;
        });
        let mcp_service = mcp_service(&service, config.variant);
        let mcp = McpServer::new(&mcp_service);
        let mcp_task = tokio::spawn(async move {
            let _ = mcp.serve_http(mcp_listener).await;
        });
        let conversation_dir = fixture_conversation_dir();
        let conversation_path = conversation_dir.join("conversations.sqlite3");
        let store = match ConversationStore::open(&conversation_path, HISTORY_LIMIT).await {
            Ok(store) => store,
            Err(error) => {
                robot_task.abort();
                mcp_task.abort();
                remove_conversation_dir(&conversation_dir);
                return Err(error);
            }
        };
        let model_invocations = Arc::new(AtomicUsize::new(0));
        let model = FixtureCompletionModel {
            ungrounded: config.variant == FixtureVariant::UngroundedAgentReply,
            invocations: Arc::clone(&model_invocations),
        };
        let mcp_url = format!("http://{mcp_address}/mcp");
        let agent = match LabAgent::connect(model, &mcp_url, store).await {
            Ok(agent) => agent,
            Err(error) => {
                robot_task.abort();
                mcp_task.abort();
                remove_conversation_dir(&conversation_dir);
                return Err(error);
            }
        };
        let a2a_api: Arc<dyn A2aLabApi> = service;
        let a2a_task = tokio::spawn(async move {
            let _ = A2aServer::new(&a2a_api)
                .with_message_handler(Arc::new(agent) as Arc<dyn AgentMessageHandler>)
                .listen(a2a_listener)
                .await;
        });

        let server = Self {
            readiness: FixtureReadiness {
                status: "ready".to_owned(),
                a2a_url: format!("http://{a2a_address}"),
                mcp_url,
                fixtures,
            },
            conversation_dir,
            model_invocations,
            tasks: vec![robot_task, mcp_task, a2a_task],
        };
        if let Err(error) = wait_until_ready(server.readiness()).await {
            server.shutdown().await;
            return Err(error);
        }
        Ok(server)
    }

    /// Readiness payload for this process.
    #[must_use]
    pub const fn readiness(&self) -> &FixtureReadiness {
        &self.readiness
    }

    /// Fixture-owned directory containing ephemeral conversation state.
    #[must_use]
    pub fn conversation_dir(&self) -> &FsPath {
        &self.conversation_dir
    }

    /// Number of deterministic completion-model calls made by this fixture.
    #[must_use]
    pub fn model_invocations(&self) -> usize {
        self.model_invocations.load(Ordering::Relaxed)
    }

    /// Stops all fixture-owned listeners and waits for task cancellation.
    pub async fn shutdown(mut self) {
        for task in &self.tasks {
            task.abort();
        }
        for task in self.tasks.drain(..) {
            let _ = task.await;
        }
        remove_conversation_dir(&self.conversation_dir);
    }
}

struct FixtureCompletionModel {
    ungrounded: bool,
    invocations: Arc<AtomicUsize>,
}

impl CompletionModel for FixtureCompletionModel {
    fn completion(
        &self,
        request: CompletionRequest,
    ) -> impl Future<Output = Result<CompletionResponse, CompletionError>> + Send {
        self.invocations.fetch_add(1, Ordering::Relaxed);
        let result = (|| {
            let used_list_tasks = request.chat_history.iter().any(|message| {
                matches!(
                    message,
                    Message::User { content }
                        if content.iter().any(|item| {
                            matches!(item, UserContent::ToolResult(result) if result.name == "list_tasks")
                        })
                )
            });
            let choice = if used_list_tasks {
                let task_id = requested_task_id(&request).ok_or_else(|| {
                    CompletionError::ResponseError(
                        "fixture prompt did not contain a bounded task identifier".to_owned(),
                    )
                })?;
                let tool_result = serde_json::to_string(&request.chat_history)?;
                if !tool_result.contains(task_id) {
                    return Err(CompletionError::ResponseError(format!(
                        "list_tasks result did not contain requested task `{task_id}`"
                    )));
                }
                let reply = if self.ungrounded {
                    "The deterministic fixture intentionally omitted the task identifier."
                        .to_owned()
                } else {
                    format!("MCP list_tasks returned task `{task_id}`.")
                };
                vec![AssistantContent::text(reply)]
            } else {
                if !request.tools.iter().any(|tool| tool.name == "list_tasks") {
                    return Err(CompletionError::ResponseError(
                        "fixture agent did not expose list_tasks".to_owned(),
                    ));
                }
                vec![AssistantContent::tool_call(
                    "fixture-list-tasks",
                    "list_tasks",
                    json!({"page": {"limit": 10}}),
                )]
            };
            Ok(CompletionResponse::new(
                choice,
                Usage::new(),
                "a2a-lab-ot2-fixture",
            ))
        })();
        std::future::ready(result)
    }

    fn stream(
        &self,
        _request: CompletionRequest,
    ) -> impl Future<Output = Result<StreamingCompletionResponse, CompletionError>> + Send {
        std::future::ready(Err(CompletionError::ProviderError(
            "fixture model does not stream".to_owned(),
        )))
    }
}

fn requested_task_id(request: &CompletionRequest) -> Option<&str> {
    request.chat_history.iter().rev().find_map(|message| {
        let Message::User { content } = message else {
            return None;
        };
        content.iter().find_map(|item| {
            let UserContent::Text(text) = item else {
                return None;
            };
            text.text
                .split_once(TASK_ID_MARKER)
                .and_then(|(_, rest)| rest.split_once('`'))
                .map(|(task_id, _)| task_id)
        })
    })
}

fn fixture_conversation_dir() -> PathBuf {
    let id = NEXT_FIXTURE.fetch_add(1, Ordering::Relaxed);
    std::env::temp_dir().join(format!(
        "a2a-lab-ot2-compliance-{}-{id}",
        std::process::id()
    ))
}

fn remove_conversation_dir(path: &FsPath) {
    if let Err(error) = std::fs::remove_dir_all(path)
        && error.kind() != std::io::ErrorKind::NotFound
    {
        eprintln!(
            "failed to remove fixture conversation directory {}: {error}",
            path.display()
        );
    }
}

#[derive(Clone)]
struct StableMetrics {
    inner: OpentronsLab,
    cache: Arc<tokio::sync::Mutex<BTreeMap<String, Page<MetricPoint>>>>,
}

impl StableMetrics {
    fn new(inner: OpentronsLab) -> Self {
        Self {
            inner,
            cache: Arc::new(tokio::sync::Mutex::new(BTreeMap::new())),
        }
    }
}

impl MetricProvider for StableMetrics {
    async fn list_metrics(
        &self,
        request: ListMetricsRequest,
    ) -> Result<Page<MetricDescriptor>, A2aLabError> {
        self.inner.list_metrics(request).await
    }

    async fn query(&self, request: QueryMetricRequest) -> Result<Page<MetricPoint>, A2aLabError> {
        let mut key = request.metric_id.to_string();
        write!(&mut key, ":{:?}:{:?}", request.range, request.page)
            .expect("writing to a string cannot fail");
        let mut cache = self.cache.lock().await;
        if let Some(page) = cache.get(&key) {
            return Ok(page.clone());
        }
        let page = self.inner.query(request).await?;
        cache.insert(key, page.clone());
        Ok(page)
    }
}

#[derive(Clone)]
struct StableImages {
    inner: LiveImageCatalog,
    current: Arc<tokio::sync::Mutex<BTreeMap<String, Image>>>,
}

impl StableImages {
    fn new(inner: LiveImageCatalog) -> Self {
        Self {
            inner,
            current: Arc::new(tokio::sync::Mutex::new(BTreeMap::new())),
        }
    }
}

impl ImageProvider for StableImages {
    async fn list_image_sources(
        &self,
        request: ListImageSourcesRequest,
    ) -> Result<Page<ImageSource>, A2aLabError> {
        self.inner.list_image_sources(request).await
    }

    async fn list_images(
        &self,
        request: ListImagesRequest,
    ) -> Result<Page<ImageDescriptor>, A2aLabError> {
        self.inner.list_images(request).await
    }

    async fn search_images(
        &self,
        request: SearchImagesRequest,
    ) -> Result<Page<ImageDescriptor>, A2aLabError> {
        self.inner.search_images(request).await
    }

    async fn get_image(&self, request: GetImageRequest) -> Result<Image, A2aLabError> {
        self.inner.get_image(request).await
    }

    async fn get_current_image(
        &self,
        request: GetCurrentImageRequest,
    ) -> Result<Image, A2aLabError> {
        let key = request.source_id().to_string();
        let mut current = self.current.lock().await;
        if let Some(image) = current.get(&key) {
            return Ok(image.clone());
        }
        let image = self.inner.get_current_image(request).await?;
        current.insert(key, image.clone());
        Ok(image)
    }
}

fn mcp_service(service: &Arc<dyn A2aLabApi>, variant: FixtureVariant) -> Arc<dyn A2aLabApi> {
    match variant {
        FixtureVariant::Standard | FixtureVariant::UngroundedAgentReply => Arc::clone(service),
        FixtureVariant::McpMetricError => Arc::new(McpMetricMismatch {
            inner: Arc::clone(service),
        }),
        FixtureVariant::McpEmptyLogSources => Arc::new(McpEmptyLogSources {
            inner: Arc::clone(service),
        }),
    }
}

struct McpEmptyLogSources {
    inner: Arc<dyn A2aLabApi>,
}

impl A2aLabApi for McpEmptyLogSources {
    fn execute(
        &self,
        command: A2aLabCommand,
    ) -> A2aLabFuture<'_, Result<A2aLabOutcome, A2aLabError>> {
        Box::pin(async move {
            let empty_discovery = matches!(command, A2aLabCommand::ListLogSources(_));
            let mut outcome = self.inner.execute(command).await?;
            if empty_discovery {
                outcome.task.result = A2aLabResult::ListLogSources(Page::new(Vec::new(), None));
            }
            Ok(outcome)
        })
    }

    fn task<'a>(&'a self, task_id: &str) -> A2aLabFuture<'a, Result<TaskSnapshot, A2aLabError>> {
        self.inner.task(task_id)
    }

    fn cancel(
        &self,
        request: GetTaskStatusRequest,
    ) -> A2aLabFuture<'_, Result<TaskSnapshot, A2aLabError>> {
        self.inner.cancel(request)
    }
}

struct McpMetricMismatch {
    inner: Arc<dyn A2aLabApi>,
}

impl A2aLabApi for McpMetricMismatch {
    fn execute(
        &self,
        command: A2aLabCommand,
    ) -> A2aLabFuture<'_, Result<A2aLabOutcome, A2aLabError>> {
        Box::pin(async move {
            if matches!(command, A2aLabCommand::QueryMetric(_)) {
                Err(A2aLabError::not_found(
                    "metric",
                    "controlled-compliance-mismatch",
                ))
            } else {
                self.inner.execute(command).await
            }
        })
    }

    fn task<'a>(&'a self, task_id: &str) -> A2aLabFuture<'a, Result<TaskSnapshot, A2aLabError>> {
        self.inner.task(task_id)
    }

    fn cancel(
        &self,
        request: GetTaskStatusRequest,
    ) -> A2aLabFuture<'_, Result<TaskSnapshot, A2aLabError>> {
        self.inner.cancel(request)
    }
}

async fn wait_until_ready(readiness: &FixtureReadiness) -> Result<(), A2aLabError> {
    let a2a = A2aClient::new(&readiness.a2a_url)?;
    let mut last = String::new();
    for _ in 0..50 {
        match a2a.agent_card().await {
            Ok(_) => {
                if McpLab::connect(&readiness.mcp_url).await.is_ok() {
                    return Ok(());
                }
                "MCP did not accept a session".clone_into(&mut last);
            }
            Err(error) => last = error.to_string(),
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    Err(A2aLabError::unavailable(format!(
        "fixture readiness failed: {last}"
    )))
}

impl Drop for FixtureServer {
    fn drop(&mut self) {
        for task in &self.tasks {
            task.abort();
        }
        remove_conversation_dir(&self.conversation_dir);
    }
}

/// Stable fixture declaration used by the OT-2 compliance adaptor.
#[must_use]
pub fn fixture_declaration() -> ComplianceFixtures {
    ComplianceFixtures {
        suite_version: COMPLIANCE_PROFILE_VERSION.to_owned(),
        range: TimeRange::new(timestamp(RANGE_START), timestamp(RANGE_END)).expect("fixture range"),
        logs: FixtureCapability::Required(LogFixtures {
            source_id: SourceId::new("run_commands").expect("source id"),
            alternate_source_id: SourceId::new("run_command_errors").expect("source id"),
            missing_source_id: SourceId::new("fixture-missing-log").expect("source id"),
        }),
        metrics: FixtureCapability::Required(MetricFixtures {
            metric_id: a2a_lab_dev_kit::MetricId::new("healthy").expect("metric id"),
            alternate_metric_id: a2a_lab_dev_kit::MetricId::new("disk_available_mb")
                .expect("metric id"),
            missing_metric_id: a2a_lab_dev_kit::MetricId::new("fixture-missing-metric")
                .expect("metric id"),
        }),
        tasks: FixtureCapability::Required(TaskFixtures {
            task_id: TaskId::new("pause_run").expect("task id"),
            alternate_task_id: TaskId::new("resume_run").expect("task id"),
            run_id: RunId::new(FIXTURE_RUN).expect("run id"),
            missing_task_id: TaskId::new("fixture-missing-task").expect("task id"),
            missing_run_id: RunId::new("fixture-missing-run").expect("run id"),
            input: JsonObject::parse(r#"{"run_id":"fixture-run"}"#).expect("task input"),
        }),
        images: FixtureCapability::Required(ImageFixtures {
            source_id: ImageSourceId::new(PRIMARY_IMAGE_SOURCE).expect("image source id"),
            alternate_source_id: ImageSourceId::new(ALTERNATE_IMAGE_SOURCE)
                .expect("image source id"),
            image_id: a2a_lab_dev_kit::ImageId::new("img-00000000000000000001").expect("image id"),
            alternate_image_id: a2a_lab_dev_kit::ImageId::new("img-00000000000000000002")
                .expect("image id"),
            missing_source_id: ImageSourceId::new("fixture-missing-camera")
                .expect("image source id"),
            missing_image_id: a2a_lab_dev_kit::ImageId::new("fixture-missing-image")
                .expect("image id"),
        }),
    }
}

async fn fixture_images() -> Result<StableImages, A2aLabError> {
    let primary_id = ImageSourceId::new(PRIMARY_IMAGE_SOURCE)?;
    let alternate_id = ImageSourceId::new(ALTERNATE_IMAGE_SOURCE)?;
    let catalog = LiveImageCatalog::new(
        vec![
            (
                image_source(primary_id.clone(), "Primary fixture camera"),
                Arc::new(StaticCapture::new(RANGE_START, "primary")) as Arc<dyn ImageCapture>,
            ),
            (
                image_source(alternate_id.clone(), "Alternate fixture camera"),
                Arc::new(StaticCapture::new("2026-01-01T00:00:01Z", "alternate"))
                    as Arc<dyn ImageCapture>,
            ),
        ],
        ImageCatalogConfig::new(16, 1024)?,
    )?;
    catalog
        .get_current_image(GetCurrentImageRequest::new(primary_id))
        .await?;
    catalog
        .get_current_image(GetCurrentImageRequest::new(ImageSourceId::new(
            PRIMARY_IMAGE_SOURCE,
        )?))
        .await?;
    catalog
        .get_current_image(GetCurrentImageRequest::new(alternate_id))
        .await?;
    Ok(StableImages::new(catalog))
}

fn image_source(id: ImageSourceId, name: &str) -> ImageSource {
    ImageSource {
        id,
        name: name.to_owned(),
        description: "Deterministic in-process compliance image.".to_owned(),
        asset_id: Some("opentrons-ot2-fixture".to_owned()),
        semantic_id: Some("a2a-lab.compliance.fixture".to_owned()),
    }
}

struct StaticCapture {
    captured_at: UtcTimestamp,
    caption: String,
}

impl StaticCapture {
    fn new(captured_at: &str, caption: &str) -> Self {
        Self {
            captured_at: timestamp(captured_at),
            caption: caption.to_owned(),
        }
    }
}

impl ImageCapture for StaticCapture {
    fn capture(
        &self,
    ) -> Pin<Box<dyn Future<Output = Result<CapturedFrame, A2aLabError>> + Send + '_>> {
        Box::pin(async {
            Ok(CapturedFrame::new(
                self.captured_at,
                "image/jpeg",
                1,
                1,
                Some(format!("{} fixture frame", self.caption)),
                JsonObject::empty(),
                fixture_jpeg(),
            ))
        })
    }
}

fn fixture_jpeg() -> Vec<u8> {
    vec![
        0xff, 0xd8, 0xff, 0xc0, 0x00, 0x0b, 0x08, 0x00, 0x01, 0x00, 0x01, 0x01, 0x01, 0x11, 0x00,
        0xff, 0xd9,
    ]
}

async fn bind(target: &str, host: &str, port: u16) -> Result<TcpListener, A2aLabError> {
    TcpListener::bind(format!("{host}:{port}"))
        .await
        .map_err(|error| A2aLabError::unavailable(format!("{target} bind {host}:{port}: {error}")))
}

fn local_addr(target: &str, listener: &TcpListener) -> Result<SocketAddr, A2aLabError> {
    listener
        .local_addr()
        .map_err(|error| A2aLabError::unavailable(format!("{target} address: {error}")))
}

fn timestamp(value: &str) -> UtcTimestamp {
    UtcTimestamp::parse(value).expect("fixture timestamp")
}

fn robot_router() -> Router {
    Router::new()
        .route("/health", get(health))
        .route("/runs", get(runs))
        .route("/runs/{id}", get(run))
        .route("/runs/{id}/commands", get(commands))
        .route("/runs/{id}/actions", post(action))
        .route("/robot/lights", get(lights))
        .route("/logs/{id}", get(empty_log))
        .fallback(fallback)
        .layer(middleware::from_fn(require_version))
}

async fn require_version(request: Request, next: Next) -> Response {
    if request.headers().get("Opentrons-Version").is_none() {
        return (
            StatusCode::BAD_REQUEST,
            json!({"errors":[{"detail":"Opentrons-Version required"}]}).to_string(),
        )
            .into_response();
    }
    next.run(request).await
}

async fn health() -> Response {
    json!({
        "name": "a2a-lab-ot2-fixture",
        "api_version": "10.0.0",
        "robot_model": "OT-2 Standard",
        "disk_details": {"systemAvailableMb": 4096.0}
    })
    .to_string()
    .into_response()
}

async fn runs() -> Response {
    json!({"data":[fixture_run()]}).to_string().into_response()
}

async fn run(Path(id): Path<String>) -> Response {
    if id == FIXTURE_RUN {
        json!({"data":fixture_run()}).to_string().into_response()
    } else {
        missing("run")
    }
}

async fn commands(Path(id): Path<String>) -> Response {
    if id != FIXTURE_RUN {
        return missing("run");
    }
    json!({"data":[
        {
            "id":"fixture-command-1",
            "commandType":"loadLabware",
            "status":"succeeded",
            "createdAt":"2026-01-01T00:00:00Z",
            "completedAt":"2026-01-01T00:00:01Z"
        },
        {
            "id":"fixture-command-2",
            "commandType":"aspirate",
            "status":"failed",
            "error":{"id":"fixture-error","errorType":"NoTipAttachedError","detail":"no tip attached"},
            "createdAt":"2026-01-01T00:00:02Z",
            "completedAt":"2026-01-01T00:00:03Z"
        }
    ]})
    .to_string()
    .into_response()
}

async fn action(Path(id): Path<String>) -> Response {
    if id == FIXTURE_RUN {
        json!({"data":{"id":"fixture-action"}})
            .to_string()
            .into_response()
    } else {
        missing("run")
    }
}

async fn lights() -> Response {
    json!({"on":false}).to_string().into_response()
}

async fn empty_log() -> Response {
    String::new().into_response()
}

async fn fallback() -> Response {
    json!({"data":{}}).to_string().into_response()
}

fn fixture_run() -> serde_json::Value {
    json!({
        "id":FIXTURE_RUN,
        "status":"running",
        "current":true,
        "protocolId":"fixture-protocol"
    })
}

fn missing(kind: &str) -> Response {
    (
        StatusCode::NOT_FOUND,
        json!({"errors":[{"id":"NotFound","title":format!("{kind} missing"),"detail":kind}]})
            .to_string(),
    )
        .into_response()
}
