mod support;

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use a2a_lab_dev_kit::{
    A2aClient, A2aLabApi, A2aLabCommand, A2aLabError, A2aLabResult, A2aLabService, A2aServer,
    AgentMessageFuture, AgentMessageHandler, AgentMessageReply, AgentMessageRequest,
    GetTaskStatusRequest, JsonObject, ListTasksRequest, McpLab, McpServer, MemoryLogs,
    MemoryMetrics, MemoryTasks, PageRequest, StartTaskRequest, TaskId, TaskState, bind_local,
};
use a2a_lab_ot2::{OidcConfig, OpentronsLab, with_oidc};
use rmcp::ServiceExt;
use support::Mock;

fn protocol() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("protocols/serial_dilution.py")
}

async fn serve(
    service: Arc<dyn A2aLabApi>,
    handler: Option<Arc<dyn AgentMessageHandler>>,
) -> String {
    let (listener, address) = bind_local().await.unwrap();
    let mut server = A2aServer::new(&service);
    if let Some(handler) = handler {
        server = server.with_message_handler(handler);
    }
    tokio::spawn(async move {
        server.listen(listener).await.unwrap();
    });
    tokio::time::sleep(Duration::from_millis(20)).await;
    format!("http://{address}")
}

struct Repeat;

impl AgentMessageHandler for Repeat {
    fn handle(
        &self,
        request: AgentMessageRequest,
    ) -> AgentMessageFuture<'_, Result<AgentMessageReply, A2aLabError>> {
        Box::pin(async move {
            Ok(AgentMessageReply {
                text: format!("heard {}", request.text),
            })
        })
    }
}

#[tokio::test]
async fn a2a_run_pause_resume_and_status() {
    let mock = Mock::new();
    let (base, _) = mock.bind().await;
    let lab = OpentronsLab::new(&base, protocol()).unwrap();
    let service = A2aLabService::new(lab.clone(), lab.clone(), lab).share();
    let client = A2aClient::new(&serve(Arc::clone(&service), None).await).unwrap();

    let tasks = client
        .list_tasks(ListTasksRequest {
            page: PageRequest::new(None, 1000).unwrap(),
        })
        .await
        .unwrap();
    let dilution = tasks
        .items()
        .iter()
        .find(|item| item.id.as_str() == "run_serial_dilution")
        .unwrap();
    assert!(dilution.input_schema.is_some());
    assert!(dilution.output_schema.is_some());

    let started = client
        .start_task(
            StartTaskRequest::new(
                TaskId::new("run_serial_dilution").unwrap(),
                JsonObject::empty(),
            )
            .immediate(),
        )
        .await
        .unwrap();
    assert_eq!(started.state, TaskState::Working);
    let A2aLabResult::StartTask(run) = started.result else {
        panic!("start result");
    };
    assert_eq!(run.progress, Some(1.0));
    assert!(run.result.is_some());
    let run_id = run.id;

    let paused = client
        .start_task(StartTaskRequest::new(
            TaskId::new("pause_run").unwrap(),
            JsonObject::parse(&format!(r#"{{"run_id":"{run_id}"}}"#)).unwrap(),
        ))
        .await
        .unwrap();
    assert_eq!(paused.state, TaskState::Completed);

    let resumed = client
        .start_task(StartTaskRequest::new(
            TaskId::new("resume_run").unwrap(),
            JsonObject::parse(&format!(r#"{{"run_id":"{run_id}"}}"#)).unwrap(),
        ))
        .await
        .unwrap();
    assert_eq!(resumed.state, TaskState::Completed);

    let status = client
        .task_status(GetTaskStatusRequest { id: run_id })
        .await
        .unwrap();
    assert_eq!(status.state, TaskState::Working);

    let logs = client
        .query_logs(a2a_lab_dev_kit::QueryLogsRequest {
            source_id: a2a_lab_dev_kit::SourceId::new("run_commands").unwrap(),
            range: a2a_lab_dev_kit::TimeRange::new(
                a2a_lab_dev_kit::UtcTimestamp::parse("1970-01-01T00:00:00Z").unwrap(),
                a2a_lab_dev_kit::UtcTimestamp::parse("2099-01-01T00:00:00Z").unwrap(),
            )
            .unwrap(),
            page: PageRequest::new(None, 20).unwrap(),
        })
        .await
        .unwrap();
    assert!(!logs.items().is_empty());

    let metrics = client
        .query_metric(a2a_lab_dev_kit::QueryMetricRequest {
            metric_id: a2a_lab_dev_kit::MetricId::new("healthy").unwrap(),
            range: a2a_lab_dev_kit::TimeRange::new(
                a2a_lab_dev_kit::UtcTimestamp::parse("1970-01-01T00:00:00Z").unwrap(),
                a2a_lab_dev_kit::UtcTimestamp::parse("2099-01-01T00:00:00Z").unwrap(),
            )
            .unwrap(),
            page: PageRequest::new(None, 10).unwrap(),
        })
        .await
        .unwrap();
    assert!((metrics.items()[0].value - 1.0).abs() < f64::EPSILON);
}

#[tokio::test]
async fn a2a_readonly_keeps_reads_and_rejects_writes() {
    let mock = Mock::new();
    let (base, _) = mock.bind().await;
    let lab = OpentronsLab::new(&base, protocol())
        .unwrap()
        .with_readonly(true);
    let service = A2aLabService::new(lab.clone(), lab.clone(), lab).share();
    let client = A2aClient::new(&serve(Arc::clone(&service), None).await).unwrap();

    let tasks = client
        .list_tasks(ListTasksRequest {
            page: PageRequest::new(None, 1000).unwrap(),
        })
        .await
        .unwrap();
    assert!(
        !tasks
            .items()
            .iter()
            .any(|item| item.id.as_str() == "run_serial_dilution")
    );
    assert!(
        tasks
            .items()
            .iter()
            .any(|item| item.id.as_str() == "get_protocols")
    );

    let started = client
        .start_task(StartTaskRequest::new(
            TaskId::new("get_protocols").unwrap(),
            JsonObject::empty(),
        ))
        .await
        .unwrap();
    assert_eq!(started.state, TaskState::Completed);

    let denied = client
        .start_task(
            StartTaskRequest::new(
                TaskId::new("run_serial_dilution").unwrap(),
                JsonObject::empty(),
            )
            .immediate(),
        )
        .await
        .unwrap_err();
    assert_eq!(denied.code(), "not_found");

    let metrics = client
        .query_metric(a2a_lab_dev_kit::QueryMetricRequest {
            metric_id: a2a_lab_dev_kit::MetricId::new("healthy").unwrap(),
            range: a2a_lab_dev_kit::TimeRange::new(
                a2a_lab_dev_kit::UtcTimestamp::parse("1970-01-01T00:00:00Z").unwrap(),
                a2a_lab_dev_kit::UtcTimestamp::parse("2099-01-01T00:00:00Z").unwrap(),
            )
            .unwrap(),
            page: PageRequest::new(None, 10).unwrap(),
        })
        .await
        .unwrap();
    assert!((metrics.items()[0].value - 1.0).abs() < f64::EPSILON);
}

#[tokio::test]
async fn a2a_agent_message_continues_context_beside_lab_commands() {
    let mock = Mock::new();
    let (base, _) = mock.bind().await;
    let lab = OpentronsLab::new(&base, protocol()).unwrap();
    let service = A2aLabService::new(lab.clone(), lab.clone(), lab).share();
    let client =
        A2aClient::new(&serve(Arc::clone(&service), Some(Arc::new(Repeat))).await).unwrap();

    let card = client.agent_card().await.unwrap();
    assert_eq!(
        card.skills
            .iter()
            .map(|skill| skill.id.as_str())
            .collect::<Vec<_>>(),
        [
            "list-log-sources",
            "query-logs",
            "list-metrics",
            "query-metric",
            "list-tasks",
            "start-task",
            "get-task-status",
            "agent-message",
        ]
    );

    let first = client.agent_message("alpha", None).await.unwrap();
    assert_eq!(first.text, "heard alpha");
    let first_task = first.task_id.expect("task");
    let task = client.get_a2a_task(&first_task).await.unwrap();
    assert!(
        task.artifacts
            .iter()
            .flatten()
            .any(|artifact| artifact.name.as_deref() == Some("agent-message"))
    );

    let second = client
        .agent_message("beta", Some(&first.context_id))
        .await
        .unwrap();
    assert_eq!(second.text, "heard beta");
    assert_eq!(second.context_id, first.context_id);
    assert_ne!(second.task_id.as_deref(), Some(first_task.as_str()));

    let tasks = client
        .list_tasks(ListTasksRequest {
            page: PageRequest::new(None, 1000).unwrap(),
        })
        .await
        .unwrap();
    assert!(
        tasks
            .items()
            .iter()
            .any(|item| item.id.as_str() == "run_serial_dilution")
    );
}

#[tokio::test]
async fn mcp_lists_tools_and_readonly_hides_writes() {
    let mock = Mock::new();
    let (base, _) = mock.bind().await;
    let lab = OpentronsLab::new(&base, protocol())
        .unwrap()
        .with_readonly(true);
    let service = A2aLabService::new(lab.clone(), lab.clone(), lab).share();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let mcp = McpServer::new(&service);
    tokio::spawn(async move {
        mcp.serve_http(listener).await.unwrap();
    });
    let url = format!("http://{address}/mcp");

    assert_eq!(mcp_http_tool_names(&url).await, mcp_tool_names_sorted());
    let mut names = mcp_tool_names(&url).await;
    names.sort();
    assert_eq!(names, mcp_tool_names_sorted());

    let connected = McpLab::connect(&url).await.unwrap();
    let listed = connected
        .execute(A2aLabCommand::ListTasks(ListTasksRequest {
            page: PageRequest::new(None, 1000).unwrap(),
        }))
        .await
        .unwrap();
    let A2aLabResult::ListTasks(page) = listed.task.result else {
        panic!("list_tasks result");
    };
    assert!(
        !page
            .items()
            .iter()
            .any(|item| item.id.as_str() == "run_serial_dilution")
    );
    assert!(
        page.items()
            .iter()
            .any(|item| item.id.as_str() == "get_protocols")
    );

    let denied = connected
        .execute(A2aLabCommand::StartTask(
            StartTaskRequest::new(
                TaskId::new("run_serial_dilution").unwrap(),
                JsonObject::empty(),
            )
            .immediate(),
        ))
        .await
        .unwrap_err();
    assert_eq!(denied.code(), "not_found");
}

fn mcp_tool_names_sorted() -> Vec<String> {
    [
        "get_task_status",
        "list_log_sources",
        "list_metrics",
        "list_tasks",
        "query_logs",
        "query_metric",
        "start_task",
    ]
    .into_iter()
    .map(str::to_owned)
    .collect()
}

async fn mcp_http_tool_names(url: &str) -> Vec<String> {
    let http = reqwest::Client::new();
    let opened = mcp_http(
        &http,
        url,
        None,
        r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2026-07-28","capabilities":{},"clientInfo":{"name":"a2a-lab-ot2-smoke","version":"0"}}}"#,
    )
    .await;
    mcp_http(
        &http,
        url,
        opened.session.as_deref(),
        r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#,
    )
    .await;
    let listed = mcp_http(
        &http,
        url,
        opened.session.as_deref(),
        r#"{"jsonrpc":"2.0","id":2,"method":"tools/list","params":{}}"#,
    )
    .await;
    let mut names = mcp_tool_names_from_body(&listed.body);
    names.sort();
    names
}

struct McpHttp {
    session: Option<String>,
    body: String,
}

async fn mcp_http(http: &reqwest::Client, url: &str, session: Option<&str>, body: &str) -> McpHttp {
    let mut request = http
        .post(url)
        .header("content-type", "application/json")
        .header("accept", "application/json, text/event-stream")
        .body(body.to_owned());
    if let Some(session) = session {
        request = request.header("mcp-session-id", session);
    }
    let response = request.send().await.expect("mcp http");
    assert!(response.status().is_success(), "{}", response.status());
    let session = response
        .headers()
        .get("mcp-session-id")
        .map(|value| value.to_str().expect("session").to_owned());
    let body = response.text().await.expect("mcp body");
    McpHttp { session, body }
}

fn mcp_tool_names_from_body(raw: &str) -> Vec<String> {
    let payloads = raw
        .lines()
        .filter_map(|line| line.strip_prefix("data:").map(str::trim))
        .collect::<Vec<_>>();
    let payloads = if payloads.is_empty() {
        vec![raw.trim()]
    } else {
        payloads
    };
    payloads
        .into_iter()
        .filter_map(|payload| serde_json::from_str::<serde_json::Value>(payload).ok())
        .filter_map(|value| value.get("result")?.get("tools")?.as_array().cloned())
        .flatten()
        .filter_map(|tool| tool.get("name")?.as_str().map(str::to_owned))
        .collect()
}

async fn mcp_tool_names(url: &str) -> Vec<String> {
    let mut info = rmcp::model::ClientInfo::default();
    info.protocol_version = rmcp::model::ProtocolVersion::V_2026_07_28;
    let service = info
        .serve(rmcp::transport::StreamableHttpClientTransport::from_uri(
            url,
        ))
        .await
        .expect("mcp client");
    service
        .peer()
        .list_tools(None)
        .await
        .expect("tools")
        .tools
        .into_iter()
        .map(|tool| tool.name.to_string())
        .collect()
}

#[tokio::test]
async fn a2a_stays_open_without_an_oidc_issuer() {
    let service =
        A2aLabService::new(MemoryLogs::new(), MemoryMetrics::new(), MemoryTasks::new()).share();
    let client = A2aClient::new(&serve(service, None).await).unwrap();
    client
        .list_tasks(ListTasksRequest {
            page: PageRequest::new(None, 1).unwrap(),
        })
        .await
        .unwrap();
}

#[tokio::test]
async fn oidc_rejects_missing_and_wrong_scope_tokens() {
    let keys = signing_keys();
    let published = Arc::new(std::sync::Mutex::new(serde_json::json!({
        "keys": [keys.jwk.clone()],
    })));
    let issuer = oidc_provider(Arc::clone(&published)).await;
    let service =
        A2aLabService::new(MemoryLogs::new(), MemoryMetrics::new(), MemoryTasks::new()).share();
    let (listener, address) = bind_local().await.unwrap();
    let server = with_oidc(
        A2aServer::new(&service),
        &OidcConfig {
            issuer: issuer.clone(),
            audience: "a2a-lab".to_owned(),
            scope: "a2a.invoke".to_owned(),
            discovery_url: None,
        },
    )
    .unwrap();
    tokio::spawn(async move {
        server.listen(listener).await.unwrap();
    });
    let base = format!("http://{address}");
    wait_for_card(&base).await;

    let missing = send_list(&base, None).await;
    assert_eq!(missing.status(), reqwest::StatusCode::UNAUTHORIZED);

    let rejected = send_list(&base, Some(&access_token(&keys, &issuer, "profile"))).await;
    assert_eq!(rejected.status(), reqwest::StatusCode::FORBIDDEN);

    let accepted = send_list(&base, Some(&access_token(&keys, &issuer, "a2a.invoke"))).await;
    assert!(accepted.status().is_success());
    let body = accepted.text().await.unwrap();
    assert!(body.contains("list_tasks"));
}

struct SigningKeys {
    encoding: jsonwebtoken::EncodingKey,
    jwk: serde_json::Value,
}

fn signing_keys() -> SigningKeys {
    use base64::Engine;
    use rsa::pkcs8::EncodePrivateKey;
    use rsa::traits::PublicKeyParts;
    let mut rng = rand::thread_rng();
    let private = rsa::RsaPrivateKey::new(&mut rng, 2048).unwrap();
    let pem = private.to_pkcs8_pem(rsa::pkcs8::LineEnding::LF).unwrap();
    let encode = |bytes: &[u8]| base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes);
    SigningKeys {
        encoding: jsonwebtoken::EncodingKey::from_rsa_pem(pem.as_bytes()).unwrap(),
        jwk: serde_json::json!({
            "kty": "RSA",
            "use": "sig",
            "alg": "RS256",
            "kid": "ot2",
            "n": encode(&private.n().to_bytes_be()),
            "e": encode(&private.e().to_bytes_be()),
        }),
    }
}

fn access_token(keys: &SigningKeys, issuer: &str, scope: &str) -> String {
    let now = u64::try_from(jiff::Timestamp::now().as_second()).unwrap();
    let claims = serde_json::json!({
        "sub": "client-a",
        "iss": issuer,
        "aud": "a2a-lab",
        "exp": now + 600,
        "nbf": now.saturating_sub(60),
        "scope": scope,
    });
    let mut header = jsonwebtoken::Header::new(jsonwebtoken::Algorithm::RS256);
    header.kid = Some("ot2".to_owned());
    jsonwebtoken::encode(&header, &claims, &keys.encoding).unwrap()
}

async fn oidc_provider(published: Arc<std::sync::Mutex<serde_json::Value>>) -> String {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let issuer = format!("http://{}", listener.local_addr().unwrap());
    let issuer_for_doc = issuer.clone();
    let app = axum::Router::new()
        .route(
            "/.well-known/openid-configuration",
            axum::routing::get(move || {
                let issuer = issuer_for_doc.clone();
                async move {
                    axum::Json(serde_json::json!({
                        "issuer": issuer,
                        "jwks_uri": format!("{issuer}/jwks"),
                    }))
                }
            }),
        )
        .route(
            "/jwks",
            axum::routing::get(move || {
                let published = Arc::clone(&published);
                async move {
                    axum::Json(
                        published
                            .lock()
                            .unwrap_or_else(std::sync::PoisonError::into_inner)
                            .clone(),
                    )
                }
            }),
        );
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    issuer
}

async fn wait_for_card(base: &str) {
    let http = reqwest::Client::new();
    for _ in 0..50 {
        if http
            .get(format!("{base}/.well-known/agent-card.json"))
            .send()
            .await
            .is_ok_and(|response| response.status().is_success())
        {
            return;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    panic!("a2a did not start");
}

async fn send_list(base: &str, token: Option<&str>) -> reqwest::Response {
    let mut request = reqwest::Client::new()
        .post(format!("{base}/message:send"))
        .header("Content-Type", "application/json")
        .header("A2A-Version", "1.0")
        .json(&serde_json::json!({
            "message": {
                "messageId": "auth-list",
                "role": "ROLE_USER",
                "parts": [{
                    "mediaType": "application/json",
                    "data": {"operation": "list_tasks", "params": {"page": {"limit": 1}}}
                }]
            }
        }));
    if let Some(token) = token {
        request = request.bearer_auth(token);
    }
    request.send().await.unwrap()
}
