use std::collections::HashMap;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::core::error::Error;

// Validation constants
const MAX_QUERY_LENGTH: usize = 100_000;
const MAX_SYSTEM_PROMPT_LENGTH: usize = 100_000;
const MIN_TIMEOUT_SECS: u64 = 1;
const MAX_TIMEOUT_SECS: u64 = 14400; // 4 hours
const MAX_TOKENS_LIMIT: usize = 200_000;
const MAX_TOOL_NAME_LENGTH: usize = 100;

/// Configuration options for Claude AI client
///
/// The `Config` struct holds all configuration options for the Claude AI client,
/// including model selection, system prompts, tool permissions, and output formatting.
///
/// # Examples
///
/// ```rust
/// use winter_claude_core::{Config, StreamFormat};
/// use std::path::PathBuf;
///
/// // Default configuration
/// let config = Config::default();
///
/// // Custom configuration with builder pattern
/// let config = Config::builder()
///     .model("claude-3-opus-20240229")
///     .system_prompt("You are a helpful Rust programming assistant")
///     .stream_format(StreamFormat::Json)
///     .timeout_secs(60)
///     .allowed_tools(vec!["bash".to_string(), "filesystem".to_string()])
///     .build();
/// ```
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    /// Optional system prompt to set the assistant's behavior and context
    ///
    /// This prompt is sent with every request to provide consistent context
    /// and instructions to Claude about how it should respond.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub system_prompt: Option<String>,

    /// Claude model to use for requests
    ///
    /// Available models include:
    /// - `claude-3-opus-20240229` - Most capable model
    /// - `claude-3-sonnet-20240229` - Balanced performance and cost
    /// - `claude-3-haiku-20240307` - Fastest and most cost-effective
    ///
    /// If not specified, Claude CLI will use its default model.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,

    /// Path to Model Context Protocol (MCP) configuration file
    ///
    /// MCP allows Claude to interact with external tools and data sources.
    /// This should point to a valid MCP config file containing server definitions.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mcp_config_path: Option<PathBuf>,

    /// List of tools that Claude is allowed to use
    ///
    /// Tools are specified using the format `server_name__tool_name` for MCP tools
    /// or simple names like `bash` for built-in tools. An empty list or `None`
    /// means all available tools are allowed.
    ///
    /// # Examples
    ///
    /// ```rust
    /// let tools = vec![
    ///     "bash".to_string(),
    ///     "filesystem".to_string(),
    ///     "mcp_server__database_query".to_string(),
    /// ];
    /// ```
    #[serde(skip_serializing_if = "Option::is_none")]
    pub allowed_tools: Option<Vec<String>>,

    /// Output format for Claude CLI responses
    ///
    /// - `Text`: Plain text output (default)
    /// - `Json`: Structured JSON with metadata
    /// - `StreamJson`: Line-delimited JSON messages for streaming
    #[serde(default)]
    pub stream_format: StreamFormat,

    /// Whether to run Claude CLI in non-interactive mode
    ///
    /// When `true` (default), Claude CLI won't prompt for user input,
    /// making it suitable for programmatic use.
    #[serde(default)]
    pub non_interactive: bool,

    /// Enable verbose output from Claude CLI
    ///
    /// When `true`, additional debugging information will be included
    /// in the CLI output. Useful for troubleshooting.
    #[serde(default)]
    pub verbose: bool,

    /// Maximum number of tokens to generate in the response
    ///
    /// If not specified, Claude will use its default token limit.
    /// Setting this can help control response length and costs.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_tokens: Option<usize>,

    /// Timeout in seconds for Claude CLI execution (default: 30s)
    ///
    /// How long to wait for Claude CLI to respond before timing out.
    /// Increase this for complex queries that might take longer to process.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub timeout_secs: Option<u64>,

    /// Environment variables to pass to the Claude CLI subprocess
    ///
    /// These variables will be set in the environment when spawning
    /// the Claude CLI process. Useful for passing context like triggers
    /// or authentication tokens that need to flow through to MCP servers.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub env: Option<HashMap<String, String>>,
}

/// Output format for Claude CLI responses
///
/// Controls how the Claude CLI formats its output, affecting both parsing
/// and the amount of metadata available in responses.
///
/// # Examples
///
/// ```rust
/// use winter_claude_core::StreamFormat;
///
/// // For simple text responses (default)
/// let format = StreamFormat::Text;
///
/// // For structured responses with metadata
/// let format = StreamFormat::Json;
///
/// // For streaming applications with line-delimited JSON
/// let format = StreamFormat::StreamJson;
/// ```
#[derive(Debug, Clone, Copy, Serialize, Deserialize, Default, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum StreamFormat {
    /// Plain text output without metadata
    ///
    /// This is the default format. Claude CLI returns only the text content
    /// of the response, making it simple to use but without access to
    /// metadata like costs, session IDs, or token usage.
    #[default]
    Text,

    /// Structured JSON output with full metadata
    ///
    /// Claude CLI returns a complete JSON object containing the response text
    /// along with metadata such as:
    /// - Session ID
    /// - Cost information
    /// - Token usage statistics
    /// - Timing information
    Json,

    /// Line-delimited JSON messages for streaming
    ///
    /// Each line contains a separate JSON message, allowing for real-time
    /// processing of the response as it's generated. Useful for implementing
    /// streaming interfaces or progress indicators.
    StreamJson,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            system_prompt: None,
            model: None,
            mcp_config_path: None,
            allowed_tools: None,
            stream_format: StreamFormat::default(),
            non_interactive: true,
            verbose: false,
            max_tokens: None,
            timeout_secs: Some(30), // Default 30 second timeout
            env: None,
        }
    }
}

impl Config {
    /// Create a new configuration builder
    ///
    /// The builder pattern provides a fluent interface for creating
    /// configurations with custom settings.
    ///
    /// # Examples
    ///
    /// ```rust
    /// use winter_claude_core::{Config, StreamFormat};
    ///
    /// let config = Config::builder()
    ///     .model("claude-3-opus-20240229")
    ///     .system_prompt("You are a helpful assistant")
    ///     .stream_format(StreamFormat::Json)
    ///     .timeout_secs(120)
    ///     .build();
    /// ```
    pub fn builder() -> ConfigBuilder {
        ConfigBuilder::new()
    }

    /// Validate the configuration
    ///
    /// Checks all configuration values for validity according to defined limits
    /// and constraints. Returns an error if any validation fails.
    ///
    /// # Errors
    ///
    /// Returns `Error::InvalidInput` if:
    /// - System prompt exceeds maximum length
    /// - Timeout is outside valid range
    /// - Max tokens exceeds limit
    /// - Tool names are invalid
    pub fn validate(&self) -> Result<(), Error> {
        // Validate system prompt length
        if let Some(prompt) = &self.system_prompt {
            if prompt.len() > MAX_SYSTEM_PROMPT_LENGTH {
                return Err(Error::InvalidInput(format!(
                    "System prompt exceeds maximum length of {} characters (got {})",
                    MAX_SYSTEM_PROMPT_LENGTH,
                    prompt.len()
                )));
            }
        }

        // Validate timeout
        if let Some(timeout) = self.timeout_secs {
            if !(MIN_TIMEOUT_SECS..=MAX_TIMEOUT_SECS).contains(&timeout) {
                return Err(Error::InvalidInput(format!(
                    "Timeout must be between {MIN_TIMEOUT_SECS} and {MAX_TIMEOUT_SECS} seconds (got {timeout})"
                )));
            }
        }

        // Validate max tokens
        if let Some(max_tokens) = self.max_tokens {
            if max_tokens == 0 || max_tokens > MAX_TOKENS_LIMIT {
                return Err(Error::InvalidInput(format!(
                    "Max tokens must be between 1 and {MAX_TOKENS_LIMIT} (got {max_tokens})"
                )));
            }
        }

        // Validate allowed tools
        if let Some(tools) = &self.allowed_tools {
            for tool in tools {
                if tool.is_empty() || tool.len() > MAX_TOOL_NAME_LENGTH {
                    return Err(Error::InvalidInput(format!(
                        "Tool name length must be between 1 and {MAX_TOOL_NAME_LENGTH} characters (got '{tool}')"
                    )));
                }

                // Validate tool name format
                if !is_valid_tool_name(tool) {
                    return Err(Error::InvalidInput(format!(
                        "Invalid tool name format: '{tool}'. Tool names must contain only alphanumeric characters, underscores, hyphens, and colons"
                    )));
                }
            }
        }

        // Validate MCP config path
        if let Some(path) = &self.mcp_config_path {
            if path.as_os_str().is_empty() {
                return Err(Error::InvalidInput(
                    "MCP config path cannot be empty".to_string(),
                ));
            }
        }

        Ok(())
    }
}

/// Builder for creating `Config` instances with fluent configuration
///
/// The `ConfigBuilder` provides a convenient way to construct configuration
/// objects using the builder pattern. All methods are chainable and return
/// `self` for fluent composition.
///
/// # Examples
///
/// ```rust
/// use winter_claude_core::{Config, StreamFormat};
///
/// let config = Config::builder()
///     .model("claude-3-sonnet-20240229")
///     .system_prompt("You are an expert Rust developer")
///     .stream_format(StreamFormat::Json)
///     .max_tokens(4096)
///     .timeout_secs(60)
///     .allowed_tools(vec!["bash".to_string(), "filesystem".to_string()])
///     .build();
/// ```
pub struct ConfigBuilder {
    config: Config,
}

impl Default for ConfigBuilder {
    fn default() -> Self {
        Self::new()
    }
}

impl ConfigBuilder {
    /// Create a new configuration builder with default settings
    pub fn new() -> Self {
        Self {
            config: Config::default(),
        }
    }

    /// Set the system prompt for the assistant
    ///
    /// The system prompt provides context and instructions that influence
    /// how Claude responds to all queries in a session.
    ///
    /// # Examples
    ///
    /// ```rust
    /// use winter_claude_core::Config;
    ///
    /// let config = Config::builder()
    ///     .system_prompt("You are a helpful Rust programming assistant")
    ///     .build();
    /// ```
    #[must_use]
    pub fn system_prompt(mut self, prompt: impl Into<String>) -> Self {
        self.config.system_prompt = Some(prompt.into());
        self
    }

    /// Set the Claude model to use
    ///
    /// Specify which Claude model should handle the requests. Different models
    /// have different capabilities, speed, and cost characteristics.
    ///
    /// # Examples
    ///
    /// ```rust
    /// use winter_claude_core::Config;
    ///
    /// let config = Config::builder()
    ///     .model("claude-3-opus-20240229")  // Most capable
    ///     .build();
    /// ```
    #[must_use]
    pub fn model(mut self, model: impl Into<String>) -> Self {
        self.config.model = Some(model.into());
        self
    }

    /// Set the path to the MCP (Model Context Protocol) configuration file
    ///
    /// MCP allows Claude to interact with external tools and data sources.
    /// The config file should contain server definitions and tool configurations.
    ///
    /// # Examples
    ///
    /// ```rust
    /// use winter_claude_core::Config;
    /// use std::path::PathBuf;
    ///
    /// let config = Config::builder()
    ///     .mcp_config(PathBuf::from("./mcp-config.json"))
    ///     .build();
    /// ```
    #[must_use]
    pub fn mcp_config(mut self, path: impl Into<PathBuf>) -> Self {
        self.config.mcp_config_path = Some(path.into());
        self
    }

    /// Set the list of allowed tools
    ///
    /// Controls which tools Claude can access during execution. Use this
    /// to restrict capabilities for security or to focus on specific tool sets.
    ///
    /// # Examples
    ///
    /// ```rust
    /// use winter_claude_core::Config;
    ///
    /// let config = Config::builder()
    ///     .allowed_tools(vec![
    ///         "bash".to_string(),
    ///         "filesystem".to_string(),
    ///         "calculator".to_string(),
    ///     ])
    ///     .build();
    /// ```
    #[must_use]
    pub fn allowed_tools(mut self, tools: Vec<String>) -> Self {
        self.config.allowed_tools = Some(tools);
        self
    }

    /// Set the output format for Claude CLI responses
    ///
    /// Choose between plain text, structured JSON, or streaming JSON formats
    /// depending on your application's needs.
    ///
    /// # Examples
    ///
    /// ```rust
    /// use winter_claude_core::{Config, StreamFormat};
    ///
    /// let config = Config::builder()
    ///     .stream_format(StreamFormat::Json)
    ///     .build();
    /// ```
    #[must_use]
    pub fn stream_format(mut self, format: StreamFormat) -> Self {
        self.config.stream_format = format;
        self
    }

    /// Set whether to run in non-interactive mode
    ///
    /// When `true`, Claude CLI won't prompt for user input, making it
    /// suitable for programmatic use. This is usually `true` for SDK usage.
    ///
    /// # Examples
    ///
    /// ```rust
    /// use winter_claude_core::Config;
    ///
    /// let config = Config::builder()
    ///     .non_interactive(true)
    ///     .build();
    /// ```
    #[must_use]
    pub fn non_interactive(mut self, non_interactive: bool) -> Self {
        self.config.non_interactive = non_interactive;
        self
    }

    /// Set the maximum number of tokens to generate
    ///
    /// Limits the length of Claude's responses. Useful for controlling
    /// costs and ensuring responses fit within expected bounds.
    ///
    /// # Examples
    ///
    /// ```rust
    /// use winter_claude_core::Config;
    ///
    /// let config = Config::builder()
    ///     .max_tokens(2048)  // Limit to 2K tokens
    ///     .build();
    /// ```
    #[must_use]
    pub fn max_tokens(mut self, max_tokens: usize) -> Self {
        self.config.max_tokens = Some(max_tokens);
        self
    }

    /// Set the timeout in seconds for Claude CLI execution
    ///
    /// How long to wait for Claude CLI to respond before giving up.
    /// Increase for complex queries or slow network conditions.
    ///
    /// # Examples
    ///
    /// ```rust
    /// use winter_claude_core::Config;
    ///
    /// let config = Config::builder()
    ///     .timeout_secs(120)  // 2 minute timeout
    ///     .build();
    /// ```
    #[must_use]
    pub fn timeout_secs(mut self, timeout_secs: u64) -> Self {
        self.config.timeout_secs = Some(timeout_secs);
        self
    }

    /// Set whether to enable verbose output from Claude CLI
    ///
    /// When `true`, additional debugging information will be included
    /// in the CLI output. Useful for troubleshooting.
    ///
    /// # Examples
    ///
    /// ```rust
    /// use winter_claude_core::Config;
    ///
    /// let config = Config::builder()
    ///     .verbose(true)  // Enable verbose output
    ///     .build();
    /// ```
    #[must_use]
    pub fn verbose(mut self, verbose: bool) -> Self {
        self.config.verbose = verbose;
        self
    }

    /// Set environment variables to pass to the Claude CLI subprocess
    ///
    /// These variables will be set in the environment when spawning
    /// the Claude CLI process. Useful for passing context like triggers
    /// that need to flow through to MCP servers via env var substitution
    /// in the MCP config.
    ///
    /// # Examples
    ///
    /// ```rust
    /// use winter_claude_core::Config;
    /// use std::collections::HashMap;
    ///
    /// let mut env = HashMap::new();
    /// env.insert("MY_VAR".to_string(), "my_value".to_string());
    ///
    /// let config = Config::builder()
    ///     .env(env)
    ///     .build();
    /// ```
    #[must_use]
    pub fn env(mut self, env: HashMap<String, String>) -> Self {
        self.config.env = Some(env);
        self
    }

    /// Build the final configuration
    ///
    /// Consumes the builder and returns the constructed `Config` instance.
    /// Validates the configuration before returning it.
    ///
    /// # Examples
    ///
    /// ```rust
    /// use winter_claude_core::{Config, StreamFormat};
    ///
    /// let config = Config::builder()
    ///     .model("claude-3-sonnet-20240229")
    ///     .stream_format(StreamFormat::Json)
    ///     .timeout_secs(60)
    ///     .build()
    ///     .expect("valid configuration");
    /// ```
    ///
    /// # Errors
    ///
    /// Returns `Error::ConfigError` if the configuration is invalid
    pub fn build(self) -> Result<Config, Error> {
        self.config.validate()?;
        Ok(self.config)
    }
}

/// Validate query input
///
/// Checks that a query string meets all validation requirements including
/// length limits and content validation.
///
/// # Errors
///
/// Returns `Error::ConfigError` if:
/// - Query exceeds maximum length
/// - Query contains malicious content
/// - Query is empty
pub fn validate_query(query: &str) -> Result<(), Error> {
    if query.is_empty() {
        return Err(Error::InvalidInput("Query cannot be empty".to_string()));
    }

    if query.len() > MAX_QUERY_LENGTH {
        return Err(Error::InvalidInput(format!(
            "Query exceeds maximum length of {} characters (got {})",
            MAX_QUERY_LENGTH,
            query.len()
        )));
    }

    Ok(())
}

/// Check if a tool name has valid format
fn is_valid_tool_name(name: &str) -> bool {
    // Tool names should only contain alphanumeric, underscores, hyphens, and double underscores
    // Format examples: "bash", "filesystem", "mcp__server__tool"
    name.chars()
        .all(|c| c.is_alphanumeric() || c == '_' || c == '-' || c == ':')
}
