use std::io::{BufRead, BufReader};
use std::process::{Command, Stdio};

use a2a_lab_dev_kit::{
    A2aClient, A2aLabApi, A2aLabCommand, A2aLabResult, GetImageRequest, ImageSource,
    ListImageSourcesRequest, ListLogSourcesRequest, ListMetricsRequest, ListTasksRequest,
    LogSource, McpLab, MetricDescriptor, Page, PageRequest, QueryMetricRequest, StartTaskRequest,
    TaskDefinition, TaskState,
};
use a2a_lab_ot2::fixture::{FixtureCapability, FixtureDeclaration};
use a2a_lab_ot2::{
    FixtureConfig, FixtureReadiness, FixtureServer, FixtureVariant, fixture_declaration,
};

#[tokio::test]
async fn fixture_serves_real_a2a_and_mcp_with_stable_state() {
    let first = FixtureServer::start(FixtureConfig::default())
        .await
        .unwrap();
    assert_eq!(first.readiness().status, "ready");
    assert_profile_document(&first);

    let a2a = A2aClient::new(&first.readiness().a2a_url).unwrap();
    let card = a2a.agent_card().await.unwrap();
    assert!(card.skills.iter().any(|skill| skill.id == "get-image"));
    assert!(card.skills.iter().any(|skill| skill.id == "agent-message"));
    let mcp = McpLab::connect(&first.readiness().mcp_url).await.unwrap();
    let listed = mcp
        .execute(A2aLabCommand::ListTasks(ListTasksRequest {
            page: PageRequest::new(None, 2).unwrap(),
        }))
        .await
        .unwrap();
    assert!(matches!(listed.task.result, A2aLabResult::ListTasks(_)));

    let fixtures = fixture_declaration();
    let FixtureCapability::Required(tasks) = fixtures.tasks else {
        panic!("tasks must be required");
    };
    let run = a2a
        .start_task(StartTaskRequest::new(
            tasks.task_id.clone(),
            tasks.input.clone(),
        ))
        .await
        .unwrap();
    assert_eq!(run.state, TaskState::Completed);
    let FixtureCapability::Required(images) = fixtures.images else {
        panic!("images must be required");
    };
    let image = a2a
        .get_image(GetImageRequest::new(images.image_id.clone()))
        .await
        .unwrap();
    assert_eq!(image.data().len(), 17);
    let first_declaration = first.readiness().fixtures.clone();
    let first_conversation_dir = first.conversation_dir().to_owned();
    assert!(first_conversation_dir.exists());
    first.shutdown().await;
    assert!(!first_conversation_dir.exists());

    let second = FixtureServer::start(FixtureConfig::default())
        .await
        .unwrap();
    assert_eq!(second.readiness().fixtures, first_declaration);
    second.shutdown().await;
}

#[tokio::test]
async fn fixture_shutdown_and_drop_release_all_configured_ports() {
    let a2a_port = unused_port().await;
    let mcp_port = unused_port().await;
    let server = FixtureServer::start(FixtureConfig {
        a2a_port,
        mcp_port,
        ..FixtureConfig::default()
    })
    .await
    .unwrap();
    let conversation_dir = server.conversation_dir().to_owned();
    server.shutdown().await;
    assert!(!conversation_dir.exists());
    tokio::net::TcpListener::bind(("127.0.0.1", a2a_port))
        .await
        .unwrap();
    tokio::net::TcpListener::bind(("127.0.0.1", mcp_port))
        .await
        .unwrap();

    let dropped_a2a_port = unused_port().await;
    let dropped_mcp_port = unused_port().await;
    let server = FixtureServer::start(FixtureConfig {
        a2a_port: dropped_a2a_port,
        mcp_port: dropped_mcp_port,
        ..FixtureConfig::default()
    })
    .await
    .unwrap();
    let dropped_conversation_dir = server.conversation_dir().to_owned();
    drop(server);
    tokio::task::yield_now().await;
    assert!(!dropped_conversation_dir.exists());
    tokio::net::TcpListener::bind(("127.0.0.1", dropped_a2a_port))
        .await
        .unwrap();
    tokio::net::TcpListener::bind(("127.0.0.1", dropped_mcp_port))
        .await
        .unwrap();
}

#[tokio::test]
async fn occupied_port_has_an_actionable_startup_error() {
    let reservation = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = reservation.local_addr().unwrap().port();
    let Err(error) = FixtureServer::start(FixtureConfig {
        a2a_port: port,
        ..FixtureConfig::default()
    })
    .await
    else {
        panic!("occupied port must fail");
    };
    let message = error.to_string();
    assert!(message.contains("fixture A2A bind"), "{message}");
    assert!(message.contains(&port.to_string()), "{message}");
}

#[tokio::test]
async fn protocols_expose_the_same_logs_metrics_tasks_and_images() {
    let server = FixtureServer::start(FixtureConfig::default())
        .await
        .unwrap();
    let fixtures = fixture_declaration();
    let a2a = A2aClient::new(&server.readiness().a2a_url).unwrap();
    let mcp = McpLab::connect(&server.readiness().mcp_url).await.unwrap();
    let page = PageRequest::new(None, 1_000).unwrap();
    assert_same_logs(&a2a, &mcp, &fixtures, page.clone()).await;
    assert_same_metrics(&a2a, &mcp, &fixtures, page.clone()).await;
    assert_same_tasks(&a2a, &mcp, &fixtures, page.clone()).await;
    assert_same_images(&a2a, &mcp, &fixtures, page).await;
    assert_eq!(server.model_invocations(), 0);
    server.shutdown().await;
}

async fn assert_same_logs(
    a2a: &A2aClient,
    mcp: &McpLab,
    fixtures: &FixtureDeclaration,
    page: PageRequest,
) {
    let a2a_logs = a2a
        .list_log_sources(ListLogSourcesRequest { page: page.clone() })
        .await
        .unwrap();
    let mcp_logs = mcp_page(
        mcp,
        A2aLabCommand::ListLogSources(ListLogSourcesRequest { page }),
        |result| match result {
            A2aLabResult::ListLogSources(page) => Some(page),
            _ => None,
        },
    )
    .await;
    let FixtureCapability::Required(logs) = &fixtures.logs else {
        panic!("logs must be required");
    };
    assert_eq!(source_ids(&a2a_logs), source_ids(&mcp_logs));
    assert!(source_ids(&a2a_logs).contains(&logs.source_id.as_str().to_owned()));
    assert!(source_ids(&a2a_logs).contains(&logs.alternate_source_id.as_str().to_owned()));
    assert!(!source_ids(&a2a_logs).contains(&logs.missing_source_id.as_str().to_owned()));
}

async fn assert_same_metrics(
    a2a: &A2aClient,
    mcp: &McpLab,
    fixtures: &FixtureDeclaration,
    page: PageRequest,
) {
    let a2a_metrics = a2a
        .list_metrics(ListMetricsRequest { page: page.clone() })
        .await
        .unwrap();
    let mcp_metrics = mcp_page(
        mcp,
        A2aLabCommand::ListMetrics(ListMetricsRequest { page: page.clone() }),
        |result| match result {
            A2aLabResult::ListMetrics(page) => Some(page),
            _ => None,
        },
    )
    .await;
    let FixtureCapability::Required(metrics) = &fixtures.metrics else {
        panic!("metrics must be required");
    };
    assert_eq!(metric_ids(&a2a_metrics), metric_ids(&mcp_metrics));
    assert!(metric_ids(&a2a_metrics).contains(&metrics.metric_id.as_str().to_owned()));
    assert!(metric_ids(&a2a_metrics).contains(&metrics.alternate_metric_id.as_str().to_owned()));
    assert!(!metric_ids(&a2a_metrics).contains(&metrics.missing_metric_id.as_str().to_owned()));
    let query = QueryMetricRequest {
        metric_id: metrics.metric_id.clone(),
        range: fixtures.range,
        page,
    };
    let a2a_points = a2a.query_metric(query.clone()).await.unwrap();
    let mcp_points = mcp_page(
        mcp,
        A2aLabCommand::QueryMetric(query),
        |result| match result {
            A2aLabResult::QueryMetric(page) => Some(page),
            _ => None,
        },
    )
    .await;
    assert_eq!(a2a_points, mcp_points);
    assert!(!a2a_points.items().is_empty());
}

async fn assert_same_tasks(
    a2a: &A2aClient,
    mcp: &McpLab,
    fixtures: &FixtureDeclaration,
    page: PageRequest,
) {
    let a2a_tasks = a2a
        .list_tasks(ListTasksRequest { page: page.clone() })
        .await
        .unwrap();
    let mcp_tasks = mcp_page(
        mcp,
        A2aLabCommand::ListTasks(ListTasksRequest { page }),
        |result| match result {
            A2aLabResult::ListTasks(page) => Some(page),
            _ => None,
        },
    )
    .await;
    let FixtureCapability::Required(tasks) = &fixtures.tasks else {
        panic!("tasks must be required");
    };
    assert_eq!(task_ids(&a2a_tasks), task_ids(&mcp_tasks));
    assert!(task_ids(&a2a_tasks).contains(&tasks.task_id.as_str().to_owned()));
    assert!(task_ids(&a2a_tasks).contains(&tasks.alternate_task_id.as_str().to_owned()));
    assert!(!task_ids(&a2a_tasks).contains(&tasks.missing_task_id.as_str().to_owned()));
    let started = mcp_value(
        mcp,
        A2aLabCommand::StartTask(StartTaskRequest::new(
            tasks.task_id.clone(),
            tasks.input.clone(),
        )),
        |result| match result {
            A2aLabResult::StartTask(run) => Some(run),
            _ => None,
        },
    )
    .await;
    assert_eq!(started.state, TaskState::Completed);
}

async fn assert_same_images(
    a2a: &A2aClient,
    mcp: &McpLab,
    fixtures: &FixtureDeclaration,
    page: PageRequest,
) {
    let sources = ListImageSourcesRequest::new(page).unwrap();
    let a2a_sources = a2a.list_image_sources(sources.clone()).await.unwrap();
    let mcp_sources = mcp_page(
        mcp,
        A2aLabCommand::ListImageSources(sources),
        |result| match result {
            A2aLabResult::ListImageSources(page) => Some(page),
            _ => None,
        },
    )
    .await;
    let FixtureCapability::Required(images) = &fixtures.images else {
        panic!("images must be required");
    };
    assert_eq!(
        image_source_ids(&a2a_sources),
        image_source_ids(&mcp_sources)
    );
    assert!(image_source_ids(&a2a_sources).contains(&images.source_id.as_str().to_owned()));
    assert!(
        image_source_ids(&a2a_sources).contains(&images.alternate_source_id.as_str().to_owned())
    );
    assert!(
        !image_source_ids(&a2a_sources).contains(&images.missing_source_id.as_str().to_owned())
    );
    let a2a_image = a2a
        .get_image(GetImageRequest::new(images.image_id.clone()))
        .await
        .unwrap();
    let mcp_image = mcp_value(
        mcp,
        A2aLabCommand::GetImage(GetImageRequest::new(images.image_id.clone())),
        |result| match result {
            A2aLabResult::GetImage(image) => Some(image),
            _ => None,
        },
    )
    .await;
    assert_eq!(a2a_image.data(), mcp_image.data());
    assert_eq!(a2a_image.descriptor().id(), &images.image_id);
}

#[tokio::test]
async fn deterministic_model_preserves_the_discovered_task_identifier() {
    let first = FixtureServer::start(FixtureConfig::default())
        .await
        .unwrap();
    let reply = ask_for_task(&first, "pause_run").await;
    assert_eq!(reply, "MCP list_tasks returned task `pause_run`.");
    assert_eq!(first.model_invocations(), 2);
    first.shutdown().await;

    let second = FixtureServer::start(FixtureConfig::default())
        .await
        .unwrap();
    let repeated = ask_for_task(&second, "pause_run").await;
    assert_eq!(repeated, reply);
    assert_eq!(second.model_invocations(), 2);
    second.shutdown().await;
}

#[tokio::test]
async fn controlled_variants_diverge_only_on_the_selected_surface() {
    let fixtures = fixture_declaration();
    let FixtureCapability::Required(metrics) = &fixtures.metrics else {
        panic!("metrics must be required");
    };
    let page = PageRequest::new(None, 100).unwrap();
    let query = QueryMetricRequest {
        metric_id: metrics.metric_id.clone(),
        range: fixtures.range,
        page: page.clone(),
    };
    let mismatched = FixtureServer::start(FixtureConfig {
        variant: FixtureVariant::McpMetricError,
        ..FixtureConfig::default()
    })
    .await
    .unwrap();
    let a2a = A2aClient::new(&mismatched.readiness().a2a_url).unwrap();
    let mcp = McpLab::connect(&mismatched.readiness().mcp_url)
        .await
        .unwrap();
    assert!(
        !a2a.query_metric(query.clone())
            .await
            .unwrap()
            .items()
            .is_empty()
    );
    let error = mcp
        .execute(A2aLabCommand::QueryMetric(query))
        .await
        .unwrap_err();
    assert_eq!(error.code(), "not_found");
    assert!(
        error.to_string().contains("controlled-compliance-mismatch"),
        "{error}"
    );
    let listed = mcp
        .execute(A2aLabCommand::ListTasks(ListTasksRequest { page }))
        .await
        .unwrap();
    assert!(matches!(listed.task.result, A2aLabResult::ListTasks(_)));
    assert_eq!(mismatched.model_invocations(), 0);
    mismatched.shutdown().await;

    let empty = FixtureServer::start(FixtureConfig {
        variant: FixtureVariant::McpEmptyLogSources,
        ..FixtureConfig::default()
    })
    .await
    .unwrap();
    let a2a = A2aClient::new(&empty.readiness().a2a_url).unwrap();
    let mcp = McpLab::connect(&empty.readiness().mcp_url).await.unwrap();
    let page = PageRequest::new(None, 100).unwrap();
    let a2a_logs = a2a
        .list_log_sources(ListLogSourcesRequest { page: page.clone() })
        .await
        .unwrap();
    let mcp_logs = mcp_page(
        &mcp,
        A2aLabCommand::ListLogSources(ListLogSourcesRequest { page: page.clone() }),
        |result| match result {
            A2aLabResult::ListLogSources(page) => Some(page),
            _ => None,
        },
    )
    .await;
    assert!(source_ids(&a2a_logs).contains(&"run_commands".to_owned()));
    assert!(mcp_logs.items().is_empty());
    let query = QueryMetricRequest {
        metric_id: metrics.metric_id.clone(),
        range: fixtures.range,
        page,
    };
    let a2a_points = a2a.query_metric(query.clone()).await.unwrap();
    let mcp_points = mcp_page(
        &mcp,
        A2aLabCommand::QueryMetric(query),
        |result| match result {
            A2aLabResult::QueryMetric(page) => Some(page),
            _ => None,
        },
    )
    .await;
    assert_eq!(a2a_points, mcp_points);
    empty.shutdown().await;
}

#[tokio::test]
async fn ungrounded_reply_omits_the_task_identifier_and_idle_fixture_skips_the_model() {
    let idle = FixtureServer::start(FixtureConfig::default())
        .await
        .unwrap();
    let a2a = A2aClient::new(&idle.readiness().a2a_url).unwrap();
    a2a.list_tasks(ListTasksRequest {
        page: PageRequest::new(None, 10).unwrap(),
    })
    .await
    .unwrap();
    assert_eq!(idle.model_invocations(), 0);
    idle.shutdown().await;

    let ungrounded = FixtureServer::start(FixtureConfig {
        variant: FixtureVariant::UngroundedAgentReply,
        ..FixtureConfig::default()
    })
    .await
    .unwrap();
    let reply = ask_for_task(&ungrounded, "pause_run").await;
    assert_eq!(
        reply,
        "The deterministic fixture intentionally omitted the task identifier."
    );
    assert!(!reply.contains("pause_run"));
    assert_eq!(ungrounded.model_invocations(), 2);
    ungrounded.shutdown().await;
}

#[test]
fn fixture_command_ignores_external_dependency_configuration() {
    let temp_root = std::env::temp_dir().join(format!(
        "a2a-lab-ot2-fixture-process-{}",
        std::process::id()
    ));
    std::fs::create_dir_all(&temp_root).unwrap();
    let mut child = Command::new(env!("CARGO_BIN_EXE_a2a-lab-ot2"))
        .arg("fixture")
        .env("TMPDIR", &temp_root)
        .env("A2ALAB_OPENTRONS_URL", "http://unreachable.invalid")
        .env("A2ALAB_OIDC_ISSUER", "https://unreachable.invalid")
        .env("A2ALAB_MODEL_PROVIDER", "openai")
        .env("A2ALAB_MODEL", "invalid-model")
        .env("A2ALAB_BEDROCK_MODEL", "invalid-bedrock-model")
        .env("OPENAI_API_KEY", "invalid-openai-key")
        .env("ANTHROPIC_API_KEY", "invalid-anthropic-key")
        .env("AWS_ACCESS_KEY_ID", "invalid-aws-key")
        .env("AWS_SECRET_ACCESS_KEY", "invalid-aws-secret")
        .env("AWS_SESSION_TOKEN", "invalid-aws-token")
        .env("AWS_REGION", "invalid-aws-region")
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    let line = BufReader::new(child.stdout.take().unwrap())
        .lines()
        .next()
        .unwrap()
        .unwrap();
    let readiness: FixtureReadiness = serde_json::from_str(&line).unwrap();
    assert_eq!(readiness.status, "ready");
    assert_eq!(readiness.fixtures.suite_version, "1.1.0");
    readiness.fixtures.validate().unwrap();
    assert!(readiness.a2a_url.starts_with("http://127.0.0.1:"));
    assert!(readiness.mcp_url.ends_with("/mcp"));
    child.kill().unwrap();
    child.wait().unwrap();
    std::fs::remove_dir_all(temp_root).unwrap();
}

fn assert_profile_document(server: &FixtureServer) {
    let declaration = &server.readiness().fixtures;
    declaration.validate().unwrap();
    let json = serde_json::to_value(declaration).unwrap();
    assert_eq!(json["suite_version"], "1.1.0");
    for capability in ["logs", "metrics", "tasks", "images"] {
        assert_eq!(json[capability]["availability"], "required", "{capability}");
        assert!(json[capability]["fixtures"].is_object(), "{capability}");
    }
    assert!(json["range"]["start"].is_string());
    assert!(json["range"]["end"].is_string());
    assert_eq!(
        json.as_object()
            .unwrap()
            .keys()
            .cloned()
            .collect::<std::collections::BTreeSet<_>>(),
        [
            "images",
            "logs",
            "metrics",
            "range",
            "suite_version",
            "tasks"
        ]
        .into_iter()
        .map(str::to_owned)
        .collect()
    );
    let decoded: FixtureDeclaration =
        serde_json::from_str(&serde_json::to_string(declaration).unwrap()).unwrap();
    assert_eq!(&decoded, declaration);
}

async fn ask_for_task(server: &FixtureServer, task_id: &str) -> String {
    let a2a = A2aClient::new(&server.readiness().a2a_url).unwrap();
    let prompt = format!(
        "Use your lab task tools to inspect task `{task_id}`. Reply with the exact task identifier `{task_id}`."
    );
    a2a.agent_message(&prompt, None).await.unwrap().text
}

async fn mcp_page<T>(
    mcp: &McpLab,
    command: A2aLabCommand,
    take: impl FnOnce(A2aLabResult) -> Option<Page<T>>,
) -> Page<T> {
    mcp_value(mcp, command, take).await
}

async fn mcp_value<T>(
    mcp: &McpLab,
    command: A2aLabCommand,
    take: impl FnOnce(A2aLabResult) -> Option<T>,
) -> T {
    let outcome = mcp.execute(command).await.unwrap();
    take(outcome.task.result).expect("mcp result variant")
}

fn source_ids(page: &Page<LogSource>) -> Vec<String> {
    page.items()
        .iter()
        .map(|source| source.id.as_str().to_owned())
        .collect()
}

fn metric_ids(page: &Page<MetricDescriptor>) -> Vec<String> {
    page.items()
        .iter()
        .map(|metric| metric.id.as_str().to_owned())
        .collect()
}

fn task_ids(page: &Page<TaskDefinition>) -> Vec<String> {
    page.items()
        .iter()
        .map(|task| task.id.as_str().to_owned())
        .collect()
}

fn image_source_ids(page: &Page<ImageSource>) -> Vec<String> {
    page.items()
        .iter()
        .map(|source| source.id.as_str().to_owned())
        .collect()
}

async fn unused_port() -> u16 {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    listener.local_addr().unwrap().port()
}
