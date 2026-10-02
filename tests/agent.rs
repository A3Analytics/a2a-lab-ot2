use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use a2a_lab_dev_kit::{
    AgentMessageHandler, AgentMessageRequest, LabService, McpServer, MemoryLogs, MemoryMetrics,
    MemoryTasks, TaskDefinition, TaskId,
};
use a2a_lab_ot2::{
    ConversationStore, DEFAULT_ANTHROPIC_MODEL, DEFAULT_BEDROCK_MODEL, DEFAULT_OPENAI_MODEL,
    LabAgent, ModelProvider, selected_model,
};
use rig::completion::Message;
use rig::memory::ConversationMemory;
use rig::test_utils::{MockCompletionModel, MockTurn};
use tokio::net::TcpListener;

fn temp_db() -> PathBuf {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock")
        .as_nanos();
    let id = NEXT.fetch_add(1, Ordering::Relaxed);
    std::env::temp_dir().join(format!("a2a-lab-ot2-{nanos}-{id}.sqlite3"))
}

fn request(text: &str, context_id: &str) -> AgentMessageRequest {
    AgentMessageRequest {
        text: text.to_owned(),
        context_id: context_id.to_owned(),
        task_id: format!("task-{context_id}-{text}"),
        reference_task_ids: Vec::new(),
    }
}

async fn mcp_url() -> String {
    let tasks = MemoryTasks::new();
    tasks
        .insert(TaskDefinition {
            id: TaskId::new("home").expect("task id"),
            name: "Home".to_owned(),
            description: "Home the robot".to_owned(),
            asset_id: None,
            semantic_id: None,
        })
        .await;
    let service = LabService::new(MemoryLogs::new(), MemoryMetrics::new(), tasks).share();
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let address = listener.local_addr().expect("address");
    let mcp = McpServer::new(&service);
    tokio::spawn(async move {
        mcp.serve_http(listener).await.expect("mcp");
    });
    format!("http://{address}/mcp")
}

#[test]
fn model_selection_uses_provider_defaults_and_overrides() {
    assert_eq!(
        selected_model(ModelProvider::OpenAi, Some("gpt-5.2"), Some("ignored")),
        "gpt-5.2"
    );
    assert_eq!(
        selected_model(ModelProvider::Bedrock, None, Some("custom-nova")),
        "custom-nova"
    );
    assert_eq!(
        selected_model(ModelProvider::Anthropic, None, Some("custom-nova")),
        DEFAULT_ANTHROPIC_MODEL
    );
    assert_eq!(
        selected_model(ModelProvider::OpenAi, None, None),
        DEFAULT_OPENAI_MODEL
    );
    assert_eq!(
        selected_model(ModelProvider::Bedrock, None, None),
        DEFAULT_BEDROCK_MODEL
    );
    assert!(ModelProvider::parse("OpenAI").is_ok());
    assert_eq!(
        ModelProvider::parse("other").expect_err("provider").code(),
        "invalid"
    );
}

#[tokio::test]
async fn conversation_store_orders_trims_and_clears() {
    let path = temp_db();
    let store = ConversationStore::open(&path, 2).await.expect("open");
    match ConversationStore::open(&path, 0).await {
        Err(error) => assert_eq!(error.code(), "invalid"),
        Ok(_) => panic!("zero history limit was accepted"),
    }

    store
        .append(
            "ctx-a",
            vec![
                Message::user("one"),
                Message::assistant("two"),
                Message::user("three"),
                Message::assistant("four"),
            ],
        )
        .await
        .expect("append");
    store
        .append("ctx-b", vec![Message::user("other")])
        .await
        .expect("other");

    assert_eq!(
        store.load("ctx-a").await.expect("load"),
        vec![Message::user("three"), Message::assistant("four")]
    );
    assert_eq!(
        store.load("ctx-b").await.expect("load other"),
        vec![Message::user("other")]
    );
    store.clear("ctx-a").await.expect("clear");
    assert!(store.load("ctx-a").await.expect("cleared").is_empty());
    assert_eq!(
        store.load("ctx-b").await.expect("kept"),
        vec![Message::user("other")]
    );

    let reopened = ConversationStore::open(&path, 2).await.expect("reopen");
    assert_eq!(
        reopened.load("ctx-b").await.expect("reloaded"),
        vec![Message::user("other")]
    );
}

#[tokio::test]
async fn agent_answers_text_and_keeps_contexts_apart() {
    let url = mcp_url().await;
    let store = ConversationStore::open(&temp_db(), 40)
        .await
        .expect("store");
    let model = MockCompletionModel::new([
        MockTurn::text("noted alpha"),
        MockTurn::text("noted beta"),
        MockTurn::text("alpha again"),
    ]);
    let watched = model.clone();
    let agent = LabAgent::connect(model, &url, store).await.expect("agent");

    let first = agent
        .handle(request("token-alpha", "ctx-a"))
        .await
        .expect("first");
    assert_eq!(first.text, "noted alpha");
    let second = agent
        .handle(request("token-beta", "ctx-b"))
        .await
        .expect("second");
    assert_eq!(second.text, "noted beta");
    let third = agent
        .handle(request("continue", "ctx-a"))
        .await
        .expect("third");
    assert_eq!(third.text, "alpha again");

    let requests = watched.requests();
    let rendered = requests
        .iter()
        .map(|request| format!("{request:?}"))
        .collect::<Vec<_>>();
    assert!(rendered[1].contains("token-beta"));
    assert!(!rendered[1].contains("token-alpha"));
    assert!(rendered[2].contains("token-alpha"));
    assert!(!rendered[2].contains("token-beta"));
}

#[tokio::test]
async fn agent_calls_mcp_tool_and_reloads_history() {
    let url = mcp_url().await;
    let path = temp_db();
    let model = MockCompletionModel::new([
        MockTurn::tool_call(
            "call-1",
            "list_tasks",
            serde_json::json!({"page": {"limit": 10}}),
        ),
        MockTurn::text("listed home"),
    ]);
    {
        let store = ConversationStore::open(&path, 40).await.expect("store");
        let agent = LabAgent::connect(model, &url, store).await.expect("agent");
        let reply = agent
            .handle(request("which tasks", "ctx-tools"))
            .await
            .expect("reply");
        assert_eq!(reply.text, "listed home");
    }

    let store = ConversationStore::open(&path, 40).await.expect("reopen");
    let history = store.load("ctx-tools").await.expect("history");
    let rendered = format!("{history:?}");
    assert!(rendered.contains("list_tasks"));
    assert!(rendered.contains("which tasks"));
    assert!(rendered.contains("listed home"));

    let model = MockCompletionModel::new([MockTurn::text("still listed")]);
    let watched = model.clone();
    let agent = LabAgent::connect(model, &url, store)
        .await
        .expect("reloaded agent");
    let reply = agent
        .handle(request("again", "ctx-tools"))
        .await
        .expect("follow up");
    assert_eq!(reply.text, "still listed");
    let rendered = format!("{:?}", watched.requests()[0]);
    assert!(rendered.contains("which tasks"));
    assert!(rendered.contains("list_tasks"));
}

#[tokio::test]
async fn agent_serializes_turns_in_one_context() {
    let url = mcp_url().await;
    let store = ConversationStore::open(&temp_db(), 40)
        .await
        .expect("store");
    let model = MockCompletionModel::new([
        MockTurn::text("first reply"),
        MockTurn::text("second reply"),
    ]);
    let watched = model.clone();
    let agent = LabAgent::connect(model, &url, store).await.expect("agent");
    let (left, right) = tokio::join!(
        agent.handle(request("token-one", "ctx-lock")),
        agent.handle(request("token-two", "ctx-lock")),
    );
    left.expect("left");
    right.expect("right");

    let rendered = watched
        .requests()
        .iter()
        .map(|request| format!("{request:?}"))
        .collect::<Vec<_>>();
    assert_eq!(rendered.len(), 2);
    let (first_token, second_token) = if rendered[0].contains("token-one") {
        ("token-one", "token-two")
    } else {
        ("token-two", "token-one")
    };
    assert!(rendered[0].contains(first_token));
    assert!(!rendered[0].contains(second_token));
    assert!(rendered[1].contains(first_token));
    assert!(rendered[1].contains(second_token));
}
