//! Model-backed answers for plain-text A2A messages.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use a2a_lab_dev_kit::{
    A2aLabError, AgentMessageFuture, AgentMessageHandler, AgentMessageReply, AgentMessageRequest,
};
use rig::agent::AgentBuilder;
use rig::bedrock::client::Client as BedrockClient;
use rig::client::{CompletionClient, ProviderClient};
use rig::completion::{CompletionModel, Message, Prompt};
use rig::providers::anthropic::Client as AnthropicClient;
use rig::providers::openai::Client as OpenAiClient;
use rig::providers::openai::GPT_5_6_LUNA;
use rmcp::ServiceExt;
use rmcp::model::{ClientInfo, Implementation, ProtocolVersion};
use rmcp::service::RunningService;
use rmcp::transport::StreamableHttpClientTransport;
use tokio::sync::Mutex;

use crate::memory::ConversationStore;

const PREAMBLE: &str = "\
You operate an Opentrons OT-2 through the lab tools. Answer from tool results and this conversation. \
Call list_tasks before start_task and use only a task id from that list. \
Do not invent task ids, run ids, measurements, or log lines. \
When a tool changes the robot, say what changed.";

const MAX_TURNS: usize = 8;
const CONNECT_TRIES: u32 = 50;
const CONNECT_WAIT: Duration = Duration::from_millis(20);

/// Default Bedrock model for `agent-message`.
///
/// `bedrock-runtime` invokes this through the global inference profile.
pub const DEFAULT_BEDROCK_MODEL: &str = "global.openai.gpt-5.6-luna";

/// Default `OpenAI` model for `agent-message`.
pub const DEFAULT_OPENAI_MODEL: &str = GPT_5_6_LUNA;

/// Default Anthropic model for `agent-message`.
///
/// Sonnet 5 is the current fast model, at $2 per million input tokens.
pub const DEFAULT_ANTHROPIC_MODEL: &str = "claude-sonnet-5";

/// Completion provider for plain-text A2A messages.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ModelProvider {
    /// Amazon Bedrock, using the process AWS credentials and region.
    Bedrock,
    /// `OpenAI`, using `OPENAI_API_KEY`.
    OpenAi,
    /// Anthropic, using `ANTHROPIC_API_KEY`.
    Anthropic,
}

impl ModelProvider {
    /// Parses `bedrock`, `openai`, or `anthropic`.
    pub fn parse(value: &str) -> Result<Self, A2aLabError> {
        match value.trim().to_ascii_lowercase().as_str() {
            "bedrock" => Ok(Self::Bedrock),
            "openai" => Ok(Self::OpenAi),
            "anthropic" => Ok(Self::Anthropic),
            _ => Err(A2aLabError::invalid(
                "model_provider",
                "expected bedrock, openai, or anthropic",
            )),
        }
    }

    /// Small tool-capable model for this provider.
    #[must_use]
    pub const fn default_model(self) -> &'static str {
        match self {
            Self::Bedrock => DEFAULT_BEDROCK_MODEL,
            Self::OpenAi => DEFAULT_OPENAI_MODEL,
            Self::Anthropic => DEFAULT_ANTHROPIC_MODEL,
        }
    }

    /// Stable name used in logs and configuration.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Bedrock => "bedrock",
            Self::OpenAi => "openai",
            Self::Anthropic => "anthropic",
        }
    }
}

/// Chooses an explicit model, then a Bedrock override, then the provider default.
#[must_use]
pub fn selected_model(
    provider: ModelProvider,
    model: Option<&str>,
    bedrock_model: Option<&str>,
) -> String {
    if let Some(model) = nonempty(model) {
        return model.to_owned();
    }
    if provider == ModelProvider::Bedrock
        && let Some(model) = nonempty(bedrock_model)
    {
        return model.to_owned();
    }
    provider.default_model().to_owned()
}

fn nonempty(value: Option<&str>) -> Option<&str> {
    value.filter(|text| !text.trim().is_empty())
}

/// Rig agent that calls the lab MCP server and stores each A2A context in SQLite.
pub struct LabAgent {
    agent: rig::Agent,
    #[allow(dead_code)]
    mcp: RunningService<rmcp::RoleClient, ClientInfo>,
    turns: Mutex<HashMap<String, Arc<Mutex<()>>>>,
}

impl LabAgent {
    /// Connects `model` to the MCP server at `mcp_url` and stores turns in `store`.
    pub async fn connect(
        model: impl CompletionModel + 'static,
        mcp_url: &str,
        store: ConversationStore,
    ) -> Result<Self, A2aLabError> {
        let (tools, sink, mcp) = connect_mcp(mcp_url).await?;
        let agent = AgentBuilder::new(model)
            .preamble(PREAMBLE)
            .default_max_turns(MAX_TURNS)
            .memory(store)
            .rmcp_tools(tools, sink)
            .build();
        Ok(Self {
            agent,
            mcp,
            turns: Mutex::new(HashMap::new()),
        })
    }

    /// Builds an agent for `provider` and `model_id`.
    pub async fn from_provider(
        provider: ModelProvider,
        model_id: &str,
        mcp_url: &str,
        store: ConversationStore,
    ) -> Result<Self, A2aLabError> {
        match provider {
            ModelProvider::Bedrock => {
                let client = BedrockClient::from_env().map_err(provider_error)?;
                Self::connect(client.completion_model(model_id), mcp_url, store).await
            }
            ModelProvider::OpenAi => {
                let client = OpenAiClient::from_env().map_err(provider_error)?;
                Self::connect(client.completion_model(model_id), mcp_url, store).await
            }
            ModelProvider::Anthropic => {
                let client = AnthropicClient::from_env().map_err(provider_error)?;
                Self::connect(client.completion_model(model_id), mcp_url, store).await
            }
        }
    }

    async fn turn_lock(&self, context_id: &str) -> Arc<Mutex<()>> {
        self.turns
            .lock()
            .await
            .entry(context_id.to_owned())
            .or_insert_with(|| Arc::new(Mutex::new(())))
            .clone()
    }
}

impl AgentMessageHandler for LabAgent {
    fn handle(
        &self,
        request: AgentMessageRequest,
    ) -> AgentMessageFuture<'_, Result<AgentMessageReply, A2aLabError>> {
        Box::pin(async move {
            let lock = self.turn_lock(&request.context_id).await;
            let _guard = lock.lock().await;
            let text = self
                .agent
                .prompt(Message::user(request.text))
                .conversation(request.context_id)
                .await
                .map_err(|error| A2aLabError::unavailable(error.to_string()))?;
            Ok(AgentMessageReply { text })
        })
    }
}

fn provider_error(error: impl std::fmt::Display) -> A2aLabError {
    A2aLabError::unavailable(error.to_string())
}

async fn connect_mcp(
    url: &str,
) -> Result<
    (
        Vec<rmcp::model::Tool>,
        rmcp::service::ServerSink,
        RunningService<rmcp::RoleClient, ClientInfo>,
    ),
    A2aLabError,
> {
    let mut last = A2aLabError::transport(format!("mcp not ready at {url}"));
    for _ in 0..CONNECT_TRIES {
        match connect_mcp_once(url).await {
            Ok(connected) => return Ok(connected),
            Err(error) => last = error,
        }
        tokio::time::sleep(CONNECT_WAIT).await;
    }
    Err(last)
}

async fn connect_mcp_once(
    url: &str,
) -> Result<
    (
        Vec<rmcp::model::Tool>,
        rmcp::service::ServerSink,
        RunningService<rmcp::RoleClient, ClientInfo>,
    ),
    A2aLabError,
> {
    let mut info = ClientInfo::default();
    info.protocol_version = ProtocolVersion::V_2026_07_28;
    let mut client_info = Implementation::default();
    "a2a-lab-ot2".clone_into(&mut client_info.name);
    env!("CARGO_PKG_VERSION").clone_into(&mut client_info.version);
    info.client_info = client_info;
    let service = info
        .serve(StreamableHttpClientTransport::from_uri(url))
        .await
        .map_err(|error| A2aLabError::transport(error.to_string()))?;
    let listed = service
        .peer()
        .list_tools(None)
        .await
        .map_err(|error| A2aLabError::transport(error.to_string()))?;
    let sink = service.peer().clone();
    Ok((listed.tools, sink, service))
}
