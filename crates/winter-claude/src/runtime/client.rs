use std::sync::Arc;

use crate::{
    core::{
        validate_query, ClaudeCliResponse, ClaudeResponse, Config, ExtractedToolCall, Result,
        SessionId, StreamFormat,
    },
    runtime::{process::execute_claude, stream::MessageStream},
};

/// High-level client for interacting with Claude Code CLI
///
/// The `Client` provides a type-safe, async interface to Claude Code with support
/// for different output formats, configuration options, and both simple and advanced
/// response handling.
///
/// # Examples
///
/// Basic usage:
/// ```rust,no_run
/// # use crate::core::*;
/// # use winter_claude_runtime::Client;
/// # #[tokio::main]
/// # async fn main() -> crate::core::Result<()> {
/// let client = Client::new(Config::default());
/// let response = client.query("Hello").send().await?;
/// println!("{}", response);
/// # Ok(())
/// # }
/// ```
///
/// With configuration:
/// ```rust,no_run
/// # use crate::core::*;
/// # use winter_claude_runtime::Client;
/// # #[tokio::main]
/// # async fn main() -> crate::core::Result<()> {
/// let client = Client::builder()
///     .model("claude-3-opus-20240229")
///     .stream_format(StreamFormat::Json)
///     .timeout_secs(60)
///     .build();
/// # Ok(())
/// # }
/// ```
#[derive(Debug, Clone)]
pub struct Client {
    config: Arc<Config>,
}

/// Helper function to extract text from assistant message content
fn extract_assistant_text(msg: &serde_json::Value, result: &mut String) {
    let Some(message) = msg.get("message") else {
        return;
    };
    let Some(content_array) = message.get("content").and_then(|v| v.as_array()) else {
        return;
    };

    for content_item in content_array {
        if content_item.get("type").and_then(|v| v.as_str()) == Some("text") {
            if let Some(text) = content_item.get("text").and_then(|v| v.as_str()) {
                result.push_str(text);
            }
        }
    }
}

/// Extract tool calls from stream-json output.
///
/// When using `StreamFormat::StreamJson`, the `raw_json` field contains an array
/// of all JSON messages. This function extracts `tool_use` content blocks from
/// assistant messages, returning a list of tool calls.
///
/// # Examples
///
/// ```rust
/// use winter_claude::extract_tool_calls;
/// use serde_json::json;
///
/// let raw_json = json!([
///     {
///         "type": "assistant",
///         "message": {
///             "content": [
///                 {"type": "tool_use", "id": "toolu_01", "name": "WebSearch", "input": {"query": "rust"}}
///             ]
///         }
///     }
/// ]);
///
/// let tool_calls = extract_tool_calls(&raw_json);
/// assert_eq!(tool_calls.len(), 1);
/// assert_eq!(tool_calls[0].name, "WebSearch");
/// ```
pub fn extract_tool_calls(raw_json: &serde_json::Value) -> Vec<ExtractedToolCall> {
    let mut tool_calls = Vec::new();
    let Some(messages) = raw_json.as_array() else {
        return tool_calls;
    };

    for msg in messages {
        if msg.get("type").and_then(|v| v.as_str()) != Some("assistant") {
            continue;
        }

        let Some(content) = msg
            .get("message")
            .and_then(|m| m.get("content"))
            .and_then(|c| c.as_array())
        else {
            continue;
        };

        for item in content {
            if item.get("type").and_then(|v| v.as_str()) == Some("tool_use") {
                if let (Some(id), Some(name), Some(input)) = (
                    item.get("id").and_then(|v| v.as_str()),
                    item.get("name").and_then(|v| v.as_str()),
                    item.get("input"),
                ) {
                    tool_calls.push(ExtractedToolCall {
                        id: id.to_string(),
                        name: name.to_string(),
                        input: input.clone(),
                    });
                }
            }
        }
    }
    tool_calls
}

impl Client {
    /// Create a new client with the given configuration
    pub fn new(config: Config) -> Self {
        Self {
            config: Arc::new(config),
        }
    }

    /// Create a new client builder for fluent configuration
    pub fn builder() -> ClientBuilder {
        ClientBuilder::new()
    }

    /// Create a query builder for the given query string
    ///
    /// # Examples
    ///
    /// ```rust,no_run
    /// # use crate::core::*;
    /// # use winter_claude_runtime::Client;
    /// # #[tokio::main]
    /// # async fn main() -> crate::core::Result<()> {
    /// let client = Client::new(Config::default());
    /// let response = client
    ///     .query("Explain Rust ownership")
    ///     .send()
    ///     .await?;
    /// # Ok(())
    /// # }
    /// ```
    pub fn query(&self, query: impl Into<String>) -> QueryBuilder {
        QueryBuilder::new(self.clone(), query.into())
    }

    /// Send a query and return just the text content (backwards compatible)
    ///
    /// This is the simplest way to get a response from Claude. For access to
    /// metadata, costs, and raw JSON, use [`send_full`](Self::send_full).
    ///
    /// # Examples
    ///
    /// ```rust,no_run
    /// # use crate::core::*;
    /// # use winter_claude_runtime::Client;
    /// # #[tokio::main]
    /// # async fn main() -> crate::core::Result<()> {
    /// let client = Client::new(Config::default());
    /// let answer = client.send("What is 2 + 2?").await?;
    /// assert_eq!(answer.trim(), "4");
    /// # Ok(())
    /// # }
    /// ```
    pub async fn send(&self, query: &str) -> Result<String> {
        validate_query(query)?;
        let response = self.send_full(query).await?;
        Ok(response.content)
    }

    /// Send a query and return the full response with metadata and raw JSON
    ///
    /// This method provides access to the complete response from Claude Code,
    /// including metadata like costs, session IDs, and the raw JSON for
    /// advanced parsing or storage.
    ///
    /// # Examples
    ///
    /// ```rust,no_run
    /// # use crate::core::*;
    /// # use winter_claude_runtime::Client;
    /// # #[tokio::main]
    /// # async fn main() -> crate::core::Result<()> {
    /// let client = Client::builder()
    ///     .stream_format(StreamFormat::Json)
    ///     .build();
    ///
    /// let response = client.send_full("Hello").await?;
    /// println!("Content: {}", response.content);
    ///
    /// if let Some(metadata) = &response.metadata {
    ///     println!("Cost: ${:.6}", metadata.cost_usd.unwrap_or(0.0));
    ///     println!("Session: {}", metadata.session_id);
    /// }
    ///
    /// // Access raw JSON for custom parsing
    /// if let Some(raw) = &response.raw_json {
    ///     // Custom field extraction
    ///     let custom_field = raw.get("custom_field");
    /// }
    /// # Ok(())
    /// # }
    /// ```
    pub async fn send_full(&self, query: &str) -> Result<ClaudeResponse> {
        validate_query(query)?;
        let output = execute_claude(&self.config, query).await?;

        // Parse response based on format
        match self.config.stream_format {
            StreamFormat::Text => Ok(ClaudeResponse::text(output.trim().to_string())),
            StreamFormat::Json => {
                // Parse the JSON response from claude CLI
                let json_value: serde_json::Value = serde_json::from_str(&output)?;
                let claude_response: ClaudeCliResponse =
                    serde_json::from_value(json_value.clone())?;
                Ok(ClaudeResponse::with_json(
                    claude_response.result,
                    json_value,
                ))
            }
            StreamFormat::StreamJson => {
                // For stream-json, we need to parse multiple JSON lines
                let mut result = String::new();
                let all_json: Vec<serde_json::Value> = output
                    .lines()
                    .filter(|line| !line.trim().is_empty())
                    .filter_map(|line| serde_json::from_str::<serde_json::Value>(line).ok())
                    .inspect(|msg| {
                        // Check if it's an assistant message and extract text
                        if msg.get("type").and_then(|v| v.as_str()) == Some("assistant") {
                            extract_assistant_text(msg, &mut result);
                        }
                    })
                    .collect();

                // Return the response with all JSON messages as an array
                let raw_json = serde_json::Value::Array(all_json);
                Ok(ClaudeResponse::with_json(result, raw_json))
            }
        }
    }
}

/// Builder for creating `Client` instances with fluent configuration
///
/// The `ClientBuilder` provides a convenient way to construct client instances
/// using the builder pattern. All methods are chainable and return `self` for
/// fluent composition.
///
/// # Examples
///
/// ```rust,no_run
/// # use crate::core::*;
/// # use winter_claude_runtime::Client;
/// let client = Client::builder()
///     .model("claude-3-sonnet-20240229")
///     .system_prompt("You are a helpful assistant")
///     .stream_format(StreamFormat::Json)
///     .timeout_secs(60)
///     .build();
/// ```
pub struct ClientBuilder {
    config: Config,
}

impl Default for ClientBuilder {
    fn default() -> Self {
        Self::new()
    }
}

impl ClientBuilder {
    /// Create a new client builder with default configuration
    pub fn new() -> Self {
        Self {
            config: Config::default(),
        }
    }

    /// Set the configuration directly
    ///
    /// This allows you to use a pre-built `Config` instance instead of
    /// configuring individual options.
    #[must_use]
    pub fn config(mut self, config: Config) -> Self {
        self.config = config;
        self
    }

    /// Set the system prompt for the assistant
    ///
    /// # Examples
    ///
    /// ```rust,no_run
    /// # use winter_claude_runtime::Client;
    /// let client = Client::builder()
    ///     .system_prompt("You are a Rust expert")
    ///     .build();
    /// ```
    #[must_use]
    pub fn system_prompt(mut self, prompt: impl Into<String>) -> Self {
        self.config.system_prompt = Some(prompt.into());
        self
    }

    /// Set the Claude model to use
    ///
    /// # Examples
    ///
    /// ```rust,no_run
    /// # use winter_claude_runtime::Client;
    /// let client = Client::builder()
    ///     .model("claude-3-opus-20240229")
    ///     .build();
    /// ```
    #[must_use]
    pub fn model(mut self, model: impl Into<String>) -> Self {
        self.config.model = Some(model.into());
        self
    }

    /// Set the list of allowed tools
    ///
    /// # Examples
    ///
    /// ```rust,no_run
    /// # use winter_claude_runtime::Client;
    /// let client = Client::builder()
    ///     .allowed_tools(vec!["bash".to_string(), "filesystem".to_string()])
    ///     .build();
    /// ```
    #[must_use]
    pub fn allowed_tools(mut self, tools: Vec<String>) -> Self {
        self.config.allowed_tools = Some(tools);
        self
    }

    /// Set the output format for responses
    ///
    /// # Examples
    ///
    /// ```rust,no_run
    /// # use winter_claude_runtime::Client;
    /// # use crate::core::StreamFormat;
    /// let client = Client::builder()
    ///     .stream_format(StreamFormat::Json)
    ///     .build();
    /// ```
    #[must_use]
    pub fn stream_format(mut self, format: StreamFormat) -> Self {
        self.config.stream_format = format;
        self
    }

    /// Enable or disable verbose output
    ///
    /// # Examples
    ///
    /// ```rust,no_run
    /// # use winter_claude_runtime::Client;
    /// let client = Client::builder()
    ///     .verbose(true)
    ///     .build();
    /// ```
    #[must_use]
    pub fn verbose(mut self, verbose: bool) -> Self {
        self.config.verbose = verbose;
        self
    }

    /// Set the timeout in seconds
    ///
    /// # Examples
    ///
    /// ```rust,no_run
    /// # use winter_claude_runtime::Client;
    /// let client = Client::builder()
    ///     .timeout_secs(120)  // 2 minute timeout
    ///     .build();
    /// ```
    #[must_use]
    pub fn timeout_secs(mut self, timeout_secs: u64) -> Self {
        self.config.timeout_secs = Some(timeout_secs);
        self
    }

    /// Build the final client instance
    ///
    /// # Examples
    ///
    /// ```rust,no_run
    /// # use winter_claude_runtime::Client;
    /// # use crate::core::StreamFormat;
    /// let client = Client::builder()
    ///     .model("claude-3-sonnet-20240229")
    ///     .stream_format(StreamFormat::Json)
    ///     .build()
    ///     .expect("valid configuration");
    /// ```
    ///
    /// # Errors
    ///
    /// Returns an error if the configuration is invalid
    pub fn build(self) -> Result<Client> {
        self.config.validate()?;
        Ok(Client::new(self.config))
    }
}

/// Builder for constructing and executing Claude queries
///
/// The `QueryBuilder` provides a fluent interface for configuring queries
/// before sending them to Claude. It supports different response formats
/// and execution modes.
///
/// # Examples
///
/// ```rust,no_run
/// # use crate::core::*;
/// # use winter_claude_runtime::Client;
/// # #[tokio::main]
/// # async fn main() -> crate::core::Result<()> {
/// # let client = Client::new(Config::default());
/// // Simple query
/// let response = client
///     .query("What is Rust?")
///     .send()
///     .await?;
///
/// // Query with session and custom format
/// let response = client
///     .query("Continue the conversation")
///     .session("my-session".to_string())
///     .format(StreamFormat::Json)
///     .send_full()
///     .await?;
/// # Ok(())
/// # }
/// ```
#[derive(Debug)]
pub struct QueryBuilder {
    client: Client,
    query: String,
    session_id: Option<SessionId>,
    format: Option<StreamFormat>,
}

impl QueryBuilder {
    /// Create a new query builder (internal use)
    fn new(client: Client, query: String) -> Self {
        Self {
            client,
            query,
            session_id: None,
            format: None,
        }
    }

    /// Specify a session ID for this query
    ///
    /// This allows the query to be part of an ongoing conversation
    /// with maintained context.
    ///
    /// # Examples
    ///
    /// ```rust,no_run
    /// # use crate::core::*;
    /// # use winter_claude_runtime::Client;
    /// # #[tokio::main]
    /// # async fn main() -> crate::core::Result<()> {
    /// # let client = Client::new(Config::default());
    /// let response = client
    ///     .query("Remember this: the key is 42")
    ///     .session("my-session".to_string())
    ///     .send()
    ///     .await?;
    /// # Ok(())
    /// # }
    /// ```
    #[must_use]
    pub fn session(mut self, session_id: SessionId) -> Self {
        self.session_id = Some(session_id);
        self
    }

    /// Override the output format for this specific query
    ///
    /// This allows you to use a different format than the client's
    /// default configuration for this specific query.
    ///
    /// # Examples
    ///
    /// ```rust,no_run
    /// # use crate::core::*;
    /// # use winter_claude_runtime::Client;
    /// # #[tokio::main]
    /// # async fn main() -> crate::core::Result<()> {
    /// # let client = Client::new(Config::default());
    /// let response = client
    ///     .query("What is the weather?")
    ///     .format(StreamFormat::Json)
    ///     .send_full()
    ///     .await?;
    /// # Ok(())
    /// # }
    /// ```
    #[must_use]
    pub fn format(mut self, format: StreamFormat) -> Self {
        self.format = Some(format);
        self
    }

    /// Send the query and return just the text content
    ///
    /// This is the simplest way to get a response from Claude,
    /// returning only the text without metadata.
    ///
    /// # Examples
    ///
    /// ```rust,no_run
    /// # use crate::core::*;
    /// # use winter_claude_runtime::Client;
    /// # #[tokio::main]
    /// # async fn main() -> crate::core::Result<()> {
    /// # let client = Client::new(Config::default());
    /// let answer = client
    ///     .query("What is 2 + 2?")
    ///     .send()
    ///     .await?;
    /// println!("Answer: {}", answer);
    /// # Ok(())
    /// # }
    /// ```
    pub async fn send(self) -> Result<String> {
        self.client.send(&self.query).await
    }

    /// Send the query and return the full response with metadata
    ///
    /// This provides access to cost information, session IDs, token usage,
    /// and the raw JSON response for advanced use cases.
    ///
    /// # Examples
    ///
    /// ```rust,no_run
    /// # use crate::core::*;
    /// # use winter_claude_runtime::Client;
    /// # #[tokio::main]
    /// # async fn main() -> crate::core::Result<()> {
    /// # let client = Client::new(Config::default());
    /// let response = client
    ///     .query("Explain quantum computing")
    ///     .send_full()
    ///     .await?;
    ///
    /// println!("Response: {}", response.content);
    /// if let Some(metadata) = &response.metadata {
    ///     if let Some(cost) = metadata.cost_usd {
    ///         println!("Cost: ${:.6}", cost);
    ///     }
    /// }
    /// # Ok(())
    /// # }
    /// ```
    pub async fn send_full(self) -> Result<ClaudeResponse> {
        self.client.send_full(&self.query).await
    }

    /// Send the query and return a stream of messages
    ///
    /// This allows for real-time processing of Claude's response as it's
    /// being generated, useful for implementing streaming UIs.
    ///
    /// # Examples
    ///
    /// ```rust,no_run
    /// # use crate::core::*;
    /// # use winter_claude_runtime::Client;
    /// # use futures::StreamExt;
    /// # #[tokio::main]
    /// # async fn main() -> crate::core::Result<()> {
    /// # let client = Client::new(Config::default());
    /// let mut stream = client
    ///     .query("Write a short story")
    ///     .stream()
    ///     .await?;
    ///
    /// while let Some(message_result) = stream.next().await {
    ///     match message_result {
    ///         Ok(message) => {
    ///             // Process each message as it arrives
    ///             println!("Message: {:?}", message);
    ///         }
    ///         Err(e) => eprintln!("Stream error: {}", e),
    ///     }
    /// }
    /// # Ok(())
    /// # }
    /// ```
    pub async fn stream(self) -> Result<MessageStream> {
        use crate::runtime::process::execute_claude_streaming;

        let format = self.format.unwrap_or(self.client.config.stream_format);

        // Use real streaming by calling the new streaming execute function
        let line_receiver = execute_claude_streaming(&self.client.config, &self.query).await?;

        // Convert the line stream to a message stream
        Ok(MessageStream::from_line_stream(line_receiver, format))
    }

    /// Send the query and parse the response as JSON
    ///
    /// This is a convenience method for when you expect Claude to return
    /// structured data that can be deserialized into a specific type.
    ///
    /// # Examples
    ///
    /// ```rust,no_run
    /// # use crate::core::*;
    /// # use winter_claude_runtime::Client;
    /// # use serde::Deserialize;
    /// # #[tokio::main]
    /// # async fn main() -> crate::core::Result<()> {
    /// #[derive(Deserialize)]
    /// struct WeatherData {
    ///     temperature: f64,
    ///     humidity: f64,
    /// }
    ///
    /// # let client = Client::new(Config::default());
    /// let weather: WeatherData = client
    ///     .query("Return weather data as JSON: {\"temperature\": 22.5, \"humidity\": 65}")
    ///     .parse_output()
    ///     .await?;
    ///
    /// println!("Temperature: {}°C", weather.temperature);
    /// # Ok(())
    /// # }
    /// ```
    pub async fn parse_output<T: serde::de::DeserializeOwned>(self) -> Result<T> {
        let response = self.send().await?;
        serde_json::from_str(&response).map_err(Into::into)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn test_extract_tool_calls_basic() {
        let raw_json = json!([
            {
                "type": "assistant",
                "message": {
                    "content": [
                        {
                            "type": "tool_use",
                            "id": "toolu_01ABC",
                            "name": "WebSearch",
                            "input": {"query": "rust programming"}
                        }
                    ]
                }
            }
        ]);

        let tool_calls = extract_tool_calls(&raw_json);
        assert_eq!(tool_calls.len(), 1);
        assert_eq!(tool_calls[0].id, "toolu_01ABC");
        assert_eq!(tool_calls[0].name, "WebSearch");
        assert_eq!(tool_calls[0].input["query"], "rust programming");
    }

    #[test]
    fn test_extract_tool_calls_multiple() {
        let raw_json = json!([
            {
                "type": "assistant",
                "message": {
                    "content": [
                        {
                            "type": "tool_use",
                            "id": "toolu_01",
                            "name": "Read",
                            "input": {"path": "/foo/bar.rs"}
                        },
                        {
                            "type": "text",
                            "text": "Let me check that file"
                        },
                        {
                            "type": "tool_use",
                            "id": "toolu_02",
                            "name": "WebFetch",
                            "input": {"url": "https://example.com"}
                        }
                    ]
                }
            }
        ]);

        let tool_calls = extract_tool_calls(&raw_json);
        assert_eq!(tool_calls.len(), 2);
        assert_eq!(tool_calls[0].name, "Read");
        assert_eq!(tool_calls[1].name, "WebFetch");
    }

    #[test]
    fn test_extract_tool_calls_empty() {
        let raw_json = json!([
            {
                "type": "assistant",
                "message": {
                    "content": [
                        {"type": "text", "text": "Hello!"}
                    ]
                }
            }
        ]);

        let tool_calls = extract_tool_calls(&raw_json);
        assert!(tool_calls.is_empty());
    }

    #[test]
    fn test_extract_tool_calls_ignores_non_assistant() {
        let raw_json = json!([
            {
                "type": "user",
                "message": {
                    "content": [
                        {
                            "type": "tool_use",
                            "id": "toolu_01",
                            "name": "Read",
                            "input": {}
                        }
                    ]
                }
            }
        ]);

        let tool_calls = extract_tool_calls(&raw_json);
        assert!(tool_calls.is_empty());
    }

    #[test]
    fn test_extract_tool_calls_not_array() {
        let raw_json = json!({"type": "assistant"});
        let tool_calls = extract_tool_calls(&raw_json);
        assert!(tool_calls.is_empty());
    }

    #[test]
    fn test_extract_tool_calls_across_multiple_messages() {
        let raw_json = json!([
            {
                "type": "assistant",
                "message": {
                    "content": [
                        {"type": "tool_use", "id": "t1", "name": "Read", "input": {}}
                    ]
                }
            },
            {
                "type": "tool_result",
                "content": "file contents..."
            },
            {
                "type": "assistant",
                "message": {
                    "content": [
                        {"type": "tool_use", "id": "t2", "name": "WebSearch", "input": {"query": "test"}}
                    ]
                }
            }
        ]);

        let tool_calls = extract_tool_calls(&raw_json);
        assert_eq!(tool_calls.len(), 2);
        assert_eq!(tool_calls[0].name, "Read");
        assert_eq!(tool_calls[1].name, "WebSearch");
    }
}
