//! Agent for invoking Claude with MCP tools.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::Duration;

use claude_sdk_rs::{
    ClaudeResponse, Client, Config as ClaudeConfig, StreamFormat, extract_tool_calls,
};
use tracing::{debug, info, warn};
use winter_mcp::ToolRegistry;

use crate::{AgentContext, AgentError, PromptBuilder};

/// Built-in Claude Code tools that we want to log.
const BUILTIN_TOOLS: &[&str] = &["Read", "WebFetch", "WebSearch", "Glob", "Grep"];

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

    /// Log built-in tool calls from the Claude response.
    ///
    /// Extracts tool_use blocks from the stream-json output and sends them
    /// to the MCP server to be recorded as Thought records.
    async fn log_builtin_tool_calls(response: &ClaudeResponse, trigger: Option<String>) {
        let Some(ref raw_json) = response.raw_json else {
            return;
        };

        let tool_calls = extract_tool_calls(raw_json);

        // Get MCP URL from environment (set in Docker via WINTER_MCP_URL)
        let mcp_base_url = std::env::var("WINTER_MCP_URL")
            .ok()
            .and_then(|url| url.strip_suffix("/mcp").map(String::from))
            .unwrap_or_else(|| "http://127.0.0.1:3847".to_string());

        let client = reqwest::Client::new();

        for tc in tool_calls
            .iter()
            .filter(|tc| BUILTIN_TOOLS.contains(&tc.name.as_str()))
        {
            debug!(tool = %tc.name, id = %tc.id, "logging built-in tool call");

            let payload = serde_json::json!({
                "id": tc.id,
                "name": tc.name,
                "input": tc.input,
                "trigger": trigger,
            });

            let url = format!("{}/builtin-tool-call", mcp_base_url);

            // Fire and forget - don't block on the response
            let client = client.clone();
            let trigger_clone = trigger.clone();
            let name = tc.name.clone();
            tokio::spawn(async move {
                if let Err(e) = client.post(&url).json(&payload).send().await {
                    warn!(
                        error = %e,
                        tool = %name,
                        trigger = ?trigger_clone,
                        "failed to log built-in tool call"
                    );
                }
            });
        }
    }

    /// Handle a notification by invoking Claude with context.
    pub async fn handle_notification(
        &self,
        context: AgentContext,
        user_message: &str,
    ) -> Result<String, AgentError> {
        let timeout_duration = Duration::from_secs(900); // 15 minutes
        match tokio::time::timeout(
            timeout_duration,
            self.handle_notification_inner(context, user_message),
        )
        .await
        {
            Ok(result) => result,
            Err(_) => Err(AgentError::Timeout(
                "notification processing timed out after 15 minutes".into(),
            )),
        }
    }

    /// Inner implementation of handle_notification.
    #[tracing::instrument(skip(self, context), fields(trigger = %context.trigger_description()))]
    async fn handle_notification_inner(
        &self,
        context: AgentContext,
        user_message: &str,
    ) -> Result<String, AgentError> {
        info!("processing notification");

        let system_prompt = PromptBuilder::build(&context);
        let env = Self::build_env(&context);
        let trigger = context.trigger.as_ref().and_then(|t| t.trigger_string());

        let claude_config = ClaudeConfig::builder()
            .model("opus")
            .system_prompt(&system_prompt)
            .mcp_config(&self.mcp_config_path)
            .allowed_tools(Self::allowed_tools())
            .env(env)
            .stream_format(StreamFormat::StreamJson)
            .timeout_secs(900) // 15 minutes
            .build()?;

        let client = Client::new(claude_config);
        let response = client.query(user_message).send_full().await?;

        // Log built-in tool calls asynchronously
        Self::log_builtin_tool_calls(&response, trigger).await;

        debug!(
            response_len = response.content.len(),
            "notification processed"
        );

        Ok(response.content)
    }

    /// Handle a direct message by invoking Claude with context.
    pub async fn handle_dm(
        &self,
        context: AgentContext,
        user_message: &str,
    ) -> Result<String, AgentError> {
        let timeout_duration = Duration::from_secs(900); // 15 minutes
        match tokio::time::timeout(
            timeout_duration,
            self.handle_dm_inner(context, user_message),
        )
        .await
        {
            Ok(result) => result,
            Err(_) => Err(AgentError::Timeout(
                "DM processing timed out after 15 minutes".into(),
            )),
        }
    }

    /// Inner implementation of handle_dm.
    #[tracing::instrument(skip(self, context), fields(trigger = %context.trigger_description()))]
    async fn handle_dm_inner(
        &self,
        context: AgentContext,
        user_message: &str,
    ) -> Result<String, AgentError> {
        info!("processing direct message");

        let system_prompt = PromptBuilder::build(&context);
        let env = Self::build_env(&context);
        let trigger = context.trigger.as_ref().and_then(|t| t.trigger_string());

        let claude_config = ClaudeConfig::builder()
            .model("opus")
            .system_prompt(&system_prompt)
            .mcp_config(&self.mcp_config_path)
            .allowed_tools(Self::allowed_tools())
            .env(env)
            .stream_format(StreamFormat::StreamJson)
            .timeout_secs(900) // 15 minutes
            .build()?;

        let client = Client::new(claude_config);
        let response = client.query(user_message).send_full().await?;

        // Log built-in tool calls asynchronously
        Self::log_builtin_tool_calls(&response, trigger).await;

        debug!(response_len = response.content.len(), "DM processed");

        Ok(response.content)
    }

    /// Execute an awaken cycle - autonomous thinking time.
    pub async fn awaken(&self, context: AgentContext) -> Result<String, AgentError> {
        let timeout_duration = Duration::from_secs(1800); // 30 minutes
        match tokio::time::timeout(timeout_duration, self.awaken_inner(context)).await {
            Ok(result) => result,
            Err(_) => Err(AgentError::Timeout(
                "awaken cycle timed out after 30 minutes".into(),
            )),
        }
    }

    /// Inner implementation of awaken.
    #[tracing::instrument(skip(self, context))]
    async fn awaken_inner(&self, context: AgentContext) -> Result<String, AgentError> {
        info!("awaken cycle starting");

        let system_prompt = PromptBuilder::build(&context);
        let env = Self::build_env(&context);
        let trigger = context.trigger.as_ref().and_then(|t| t.trigger_string());

        let claude_config = ClaudeConfig::builder()
            .model("opus")
            .system_prompt(&system_prompt)
            .mcp_config(&self.mcp_config_path)
            .allowed_tools(Self::allowed_tools())
            .env(env)
            .stream_format(StreamFormat::StreamJson)
            .timeout_secs(1800) // 30 minutes
            .build()?;

        let client = Client::new(claude_config);
        let response = client
            .query("Awaken. Review your context, timeline, and thoughts. Decide what to do.")
            .send_full()
            .await?;

        // Log built-in tool calls asynchronously
        Self::log_builtin_tool_calls(&response, trigger).await;

        debug!(
            response_len = response.content.len(),
            "awaken cycle complete"
        );

        Ok(response.content)
    }

    /// Execute a scheduled job.
    pub async fn execute_job(
        &self,
        context: AgentContext,
        instructions: &str,
    ) -> Result<String, AgentError> {
        let timeout_duration = Duration::from_secs(900); // 15 minutes
        match tokio::time::timeout(
            timeout_duration,
            self.execute_job_inner(context, instructions),
        )
        .await
        {
            Ok(result) => result,
            Err(_) => Err(AgentError::Timeout(
                "job execution timed out after 15 minutes".into(),
            )),
        }
    }

    /// Inner implementation of execute_job.
    #[tracing::instrument(skip(self, context, instructions), fields(trigger = %context.trigger_description()))]
    async fn execute_job_inner(
        &self,
        context: AgentContext,
        instructions: &str,
    ) -> Result<String, AgentError> {
        info!("executing scheduled job");

        let system_prompt = PromptBuilder::build(&context);
        let env = Self::build_env(&context);
        let trigger = context.trigger.as_ref().and_then(|t| t.trigger_string());

        let claude_config = ClaudeConfig::builder()
            .model("opus")
            .system_prompt(&system_prompt)
            .mcp_config(&self.mcp_config_path)
            .allowed_tools(Self::allowed_tools())
            .env(env)
            .stream_format(StreamFormat::StreamJson)
            .timeout_secs(900) // 15 minutes
            .build()?;

        let client = Client::new(claude_config);
        let response = client.query(instructions).send_full().await?;

        // Log built-in tool calls asynchronously
        Self::log_builtin_tool_calls(&response, trigger).await;

        debug!(response_len = response.content.len(), "job complete");

        Ok(response.content)
    }

    /// Execute a background session - interruptible free time.
    ///
    /// Background sessions run when the notification queue is empty.
    /// The agent should periodically call `check_interruption` to see if
    /// notifications are waiting and gracefully exit if so.
    pub async fn background_session(&self, context: AgentContext) -> Result<String, AgentError> {
        let timeout_duration = Duration::from_secs(7200); // 2 hours max
        match tokio::time::timeout(timeout_duration, self.background_session_inner(context)).await {
            Ok(result) => result,
            Err(_) => Err(AgentError::Timeout(
                "background session timed out after 2 hours".into(),
            )),
        }
    }

    /// Inner implementation of background_session.
    #[tracing::instrument(skip(self, context))]
    async fn background_session_inner(&self, context: AgentContext) -> Result<String, AgentError> {
        info!("background session starting");

        let system_prompt = PromptBuilder::build(&context);
        let env = Self::build_env(&context);
        let trigger = context.trigger.as_ref().and_then(|t| t.trigger_string());

        let claude_config = ClaudeConfig::builder()
            .model("opus")
            .system_prompt(&system_prompt)
            .mcp_config(&self.mcp_config_path)
            .allowed_tools(Self::allowed_tools())
            .env(env)
            .stream_format(StreamFormat::StreamJson)
            .timeout_secs(7200) // 2 hours
            .build()?;

        let client = Client::new(claude_config);
        let response = client
            .query("This is your free time. Explore, learn, create—whatever interests you. Remember to call check_interruption periodically.")
            .send_full()
            .await?;

        // Log built-in tool calls asynchronously
        Self::log_builtin_tool_calls(&response, trigger).await;

        debug!(
            response_len = response.content.len(),
            "background session complete"
        );

        Ok(response.content)
    }
}
