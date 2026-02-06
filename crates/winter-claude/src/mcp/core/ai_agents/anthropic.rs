use async_trait::async_trait;
use serde_json::Value;

use crate::mcp::core::{
    error::WorkflowError,
    nodes::{agent::AgentConfig, Node},
    task::TaskContext,
};

pub struct AnthropicAgentNode {
    pub config: AgentConfig,
    pub name: String,
    pub description: String,
}

impl AnthropicAgentNode {
    pub fn new(name: String, config: AgentConfig) -> Self {
        Self {
            config,
            name,
            description: String::new(),
        }
    }
}

#[async_trait]
impl Node for AnthropicAgentNode {
    async fn execute(&self, input: Value, _context: &TaskContext) -> Result<Value, WorkflowError> {
        // Stub implementation - in real use, this would call Claude API
        Ok(serde_json::json!({
            "response": "AI response placeholder",
            "model": self.config.model,
            "input": input
        }))
    }

    fn name(&self) -> &str {
        &self.name
    }

    fn description(&self) -> &str {
        &self.description
    }
}
