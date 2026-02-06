//! Agent for invoking Claude with MCP tools.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::Duration;

use futures_util::StreamExt;
use winter_claude::{Client, Config as ClaudeConfig, Message, StreamFormat};
use tracing::{debug, info, warn};
use winter_mcp::ToolRegistry;

use crate::{AgentContext, AgentError, PromptBuilder};

const DEFAULT_MODEL: &str = "claude-opus-4-6";

/// Agent that wraps the Claude SDK for Winter.
pub struct Agent {
    mcp_config_path: PathBuf,
}

impl Agent {
    /// Create a new agent with the path to the MCP config file.
    pub fn new(mcp_config_path: impl AsRef<Path>) -> Self {
        Self {
            mcp_config_path: mcp_config_path.as_ref().to_path_buf(),
        }
    }

    /// Get the allowed tools list for Winter's MCP server.
    ///
    /// This combines the MCP tools from winter-mcp (using the colocated permission
    /// metadata) with the built-in Claude Code tools.
    fn allowed_tools() -> Vec<String> {
        // Get MCP tools from the registry (permissions are colocated with definitions)
        let mut tools = ToolRegistry::agent_allowed_tools();

        // Add built-in Claude Code tools
        tools.extend([
            "Read".to_string(),
            "WebFetch".to_string(),
            "WebSearch".to_string(),
        ]);

        tools
    }

    /// Build environment variables for the Claude subprocess.
    ///
    /// This includes the WINTER_TRIGGER variable for HTTP header substitution,
    /// allowing tool calls to be associated with their originating session.
    fn build_env(context: &AgentContext) -> HashMap<String, String> {
        let mut env = HashMap::new();

        // Set trigger for MCP HTTP header substitution
        if let Some(ref trigger) = context.trigger
            && let Some(trigger_str) = trigger.trigger_string()
        {
            env.insert("WINTER_TRIGGER".to_string(), trigger_str);
        }

        env
    }

    /// Execute a persistent session - inbox-driven model.
    ///
    /// The persistent session runs for up to 4 hours. Winter polls the inbox
    /// for work, handles items, and uses free time between items as she sees fit.
    /// She self-manages session lifecycle based on context window usage.
    pub async fn persistent_session(&self, context: AgentContext) -> Result<String, AgentError> {
        let timeout_duration = Duration::from_secs(14400); // 4 hours max
        match tokio::time::timeout(timeout_duration, self.persistent_session_inner(context)).await {
            Ok(result) => result,
            Err(_) => Err(AgentError::Timeout(
                "persistent session timed out after 4 hours".into(),
            )),
        }
    }

    /// Inner implementation of persistent_session.
    #[tracing::instrument(skip(self, context))]
    async fn persistent_session_inner(
        &self,
        context: AgentContext,
    ) -> Result<String, AgentError> {
        info!("persistent session starting");

        let system_prompt = PromptBuilder::build(&context);
        let env = Self::build_env(&context);
        let _trigger = context.trigger.as_ref().and_then(|t| t.trigger_string());

        let claude_config = ClaudeConfig::builder()
            .model(DEFAULT_MODEL)
            .system_prompt(&system_prompt)
            .mcp_config(&self.mcp_config_path)
            .allowed_tools(Self::allowed_tools())
            .env(env)
            .stream_format(StreamFormat::StreamJson)
            .timeout_secs(14400) // 4 hours
            .build()?;

        let client = Client::new(claude_config);
        let mut stream = client
            .query("You are now active. Check your inbox for pending items, then use your free time as you see fit. Call check_inbox regularly.")
            .stream()
            .await?;

        // Get MCP URL for pushing metrics
        let mcp_base_url = std::env::var("WINTER_MCP_URL")
            .ok()
            .and_then(|url| url.strip_suffix("/mcp").map(String::from))
            .unwrap_or_else(|| "http://127.0.0.1:3847".to_string());
        let metrics_url = format!("{}/session-metrics", mcp_base_url);
        let http_client = reqwest::Client::new();

        let mut content = String::new();

        while let Some(msg_result) = stream.next().await {
            match msg_result {
                Ok(Message::Assistant { content: text, meta }) => {
                    content.push_str(&text);

                    // Push per-turn metrics
                    if let Some(tokens) = meta.tokens_used {
                        let payload = serde_json::json!({
                            "input_tokens": tokens.input,
                            "output_tokens": tokens.output,
                            "total_tokens": tokens.total,
                            "cost_usd": meta.cost_usd.unwrap_or(0.0),
                            "is_turn": true,
                        });
                        let client = http_client.clone();
                        let url = metrics_url.clone();
                        tokio::spawn(async move {
                            if let Err(e) = client.post(&url).json(&payload).send().await {
                                warn!(error = %e, "failed to push session metrics");
                            }
                        });
                    }
                }
                Ok(Message::Result { stats, .. }) => {
                    debug!(
                        total_tokens = stats.total_tokens.total,
                        cost = stats.total_cost_usd,
                        messages = stats.total_messages,
                        "persistent session final stats"
                    );
                }
                Ok(_) => {} // Init, User, System, Tool, ToolResult — ignore
                Err(e) => {
                    warn!(error = %e, "stream error during persistent session");
                }
            }
        }

        debug!(
            response_len = content.len(),
            "persistent session complete"
        );

        Ok(content)
    }

}
