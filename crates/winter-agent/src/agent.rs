//! Agent for invoking Claude with MCP tools.

use std::path::{Path, PathBuf};
use std::time::Duration;

use claude_sdk_rs::{Client, Config as ClaudeConfig, ToolPermission};
use tracing::{debug, info};

use crate::{AgentContext, AgentError, PromptBuilder};

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
    fn allowed_tools() -> Vec<String> {
        vec![
            // Winter MCP tools - Bluesky
            ToolPermission::mcp("winter", "post_to_bluesky").to_cli_format(),
            ToolPermission::mcp("winter", "reply_to_bluesky").to_cli_format(),
            ToolPermission::mcp("winter", "send_bluesky_dm").to_cli_format(),
            ToolPermission::mcp("winter", "reply_to_dm").to_cli_format(),
            ToolPermission::mcp("winter", "like_post").to_cli_format(),
            ToolPermission::mcp("winter", "follow_user").to_cli_format(),
            ToolPermission::mcp("winter", "get_timeline").to_cli_format(),
            ToolPermission::mcp("winter", "get_notifications").to_cli_format(),
            // Winter MCP tools - Facts
            ToolPermission::mcp("winter", "create_fact").to_cli_format(),
            ToolPermission::mcp("winter", "update_fact").to_cli_format(),
            ToolPermission::mcp("winter", "delete_fact").to_cli_format(),
            ToolPermission::mcp("winter", "query_facts").to_cli_format(),
            // Winter MCP tools - Rules
            ToolPermission::mcp("winter", "create_rule").to_cli_format(),
            ToolPermission::mcp("winter", "list_rules").to_cli_format(),
            ToolPermission::mcp("winter", "toggle_rule").to_cli_format(),
            // Winter MCP tools - Notes
            ToolPermission::mcp("winter", "create_note").to_cli_format(),
            ToolPermission::mcp("winter", "get_note").to_cli_format(),
            ToolPermission::mcp("winter", "list_notes").to_cli_format(),
            // Winter MCP tools - Blog
            ToolPermission::mcp("winter", "publish_blog_post").to_cli_format(),
            ToolPermission::mcp("winter", "list_blog_posts").to_cli_format(),
            ToolPermission::mcp("winter", "update_blog_post").to_cli_format(),
            // Winter MCP tools - Jobs
            ToolPermission::mcp("winter", "schedule_job").to_cli_format(),
            ToolPermission::mcp("winter", "schedule_recurring").to_cli_format(),
            ToolPermission::mcp("winter", "list_jobs").to_cli_format(),
            ToolPermission::mcp("winter", "cancel_job").to_cli_format(),
            // Winter MCP tools - Self
            ToolPermission::mcp("winter", "record_thought").to_cli_format(),
            ToolPermission::mcp("winter", "get_identity").to_cli_format(),
            ToolPermission::mcp("winter", "update_identity").to_cli_format(),
            // Built-in Claude Code tools
            "Read".to_string(),
            "WebFetch".to_string(),
            "WebSearch".to_string(),
        ]
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

        let claude_config = ClaudeConfig::builder()
            .model("opus")
            .system_prompt(&system_prompt)
            .mcp_config(&self.mcp_config_path)
            .allowed_tools(Self::allowed_tools())
            .timeout_secs(900) // 15 minutes
            .build()?;

        let client = Client::new(claude_config);
        let response = client.query(user_message).send().await?;

        debug!(response_len = response.len(), "notification processed");

        Ok(response)
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

        let claude_config = ClaudeConfig::builder()
            .model("opus")
            .system_prompt(&system_prompt)
            .mcp_config(&self.mcp_config_path)
            .allowed_tools(Self::allowed_tools())
            .timeout_secs(900) // 15 minutes
            .build()?;

        let client = Client::new(claude_config);
        let response = client.query(user_message).send().await?;

        debug!(response_len = response.len(), "DM processed");

        Ok(response)
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

        let claude_config = ClaudeConfig::builder()
            .model("opus")
            .system_prompt(&system_prompt)
            .mcp_config(&self.mcp_config_path)
            .allowed_tools(Self::allowed_tools())
            .timeout_secs(1800) // 30 minutes
            .build()?;

        let client = Client::new(claude_config);
        let response = client
            .query("Awaken. Review your context, timeline, and thoughts. Decide what to do.")
            .send()
            .await?;

        debug!(response_len = response.len(), "awaken cycle complete");

        Ok(response)
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

        let claude_config = ClaudeConfig::builder()
            .model("opus")
            .system_prompt(&system_prompt)
            .mcp_config(&self.mcp_config_path)
            .allowed_tools(Self::allowed_tools())
            .timeout_secs(900) // 15 minutes
            .build()?;

        let client = Client::new(claude_config);
        let response = client.query(instructions).send().await?;

        debug!(response_len = response.len(), "job complete");

        Ok(response)
    }
}
