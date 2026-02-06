use std::{
    pin::Pin,
    task::{Context, Poll},
};

use futures::{Stream, StreamExt};
use serde::Deserialize;
use tokio::sync::mpsc;
use tracing::{debug, error};

use crate::core::{
    message::{ConversationStats, TokenUsage},
    Error, Message, MessageMeta, Result, StreamFormat,
};

/// Stream of messages from Claude AI
///
/// `MessageStream` provides real-time access to Claude's responses as they are generated,
/// enabling streaming user interfaces and progressive content display. It implements the
/// `Stream` trait for easy integration with async Rust applications.
///
/// # Examples
///
/// ```rust,no_run
/// # use winter_claude_runtime::{Client, MessageStream};
/// # use crate::core::{Config, Message, Result};
/// # use futures::StreamExt;
/// # #[tokio::main]
/// # async fn main() -> Result<()> {
/// let client = Client::new(Config::default());
/// let mut stream = client.query("Write a story").stream().await?;
///
/// // Process messages as they arrive
/// while let Some(result) = stream.next().await {
///     match result {
///         Ok(Message::Assistant { content, .. }) => {
///             print!("{}", content); // Print content incrementally
///         }
///         Ok(Message::Result { stats, .. }) => {
///             println!("\nCompleted with {} tokens", stats.total_tokens);
///         }
///         Err(e) => eprintln!("Stream error: {}", e),
///         _ => {} // Handle other message types
///     }
/// }
/// # Ok(())
/// # }
/// ```
///
/// # Stream Behavior
///
/// - Messages arrive in real-time as Claude generates the response
/// - Assistant messages may be split across multiple stream items
/// - The stream ends with a Result message containing statistics
/// - Errors are propagated through the stream rather than terminating it
///
/// # Error Handling
///
/// Errors can occur at any point in the stream. Common error scenarios:
/// - Network interruptions
/// - Invalid JSON parsing (for JSON formats)
/// - Process termination
/// - Timeout exceeded
pub struct MessageStream {
    receiver: mpsc::Receiver<Result<Message>>,
}

impl MessageStream {
    /// Create a new `MessageStream` from a channel receiver
    ///
    /// This is typically called internally by the Client. The format parameter
    /// is reserved for future use but currently not utilized.
    pub fn new(receiver: mpsc::Receiver<Result<Message>>, _format: StreamFormat) -> Self {
        Self { receiver }
    }

    /// Helper function to handle StreamJson format line parsing
    async fn handle_stream_json_line(
        parser: &MessageParser,
        line: &str,
        tx: &mpsc::Sender<Result<Message>>,
    ) -> bool {
        if let Ok(Some(message)) = parser.parse_line(line) {
            tx.send(Ok(message)).await.is_err()
        } else {
            if !line.trim().is_empty() {
                debug!("Failed to parse line as message: {}", line);
            }
            false
        }
    }

    /// Helper function to handle final JSON processing
    async fn handle_final_json(
        parser: &MessageParser,
        accumulated_content: &str,
        tx: &mpsc::Sender<Result<Message>>,
    ) {
        if accumulated_content.trim().is_empty() {
            return;
        }

        if let Ok(Some(message)) = parser.parse_accumulated_json(accumulated_content) {
            let _ = tx.send(Ok(message)).await;
        }
    }

    /// Create a `MessageStream` from a line receiver and format
    ///
    /// This function takes a receiver of raw output lines from the Claude CLI
    /// and converts them into a stream of parsed Messages based on the format.
    pub fn from_line_stream(
        mut line_receiver: mpsc::Receiver<Result<String>>,
        format: StreamFormat,
    ) -> Self {
        let config = crate::runtime::stream_config::get_stream_config();
        let (tx, rx) = mpsc::channel::<Result<Message>>(config.channel_buffer_size);

        tokio::spawn(async move {
            let config = crate::runtime::stream_config::get_stream_config();
            let parser = MessageParser::new(format);
            let mut accumulated_content = String::with_capacity(config.string_capacity);

            while let Some(line_result) = line_receiver.recv().await {
                let line = match line_result {
                    Ok(line) => line,
                    Err(e) => {
                        let _ = tx.send(Err(e)).await;
                        break;
                    }
                };

                debug!("Received line: {}", line);

                let should_break = match format {
                    StreamFormat::Text => {
                        accumulated_content.push_str(&line);
                        accumulated_content.push('\n');

                        let message = Message::Assistant {
                            content: line,
                            meta: crate::core::MessageMeta {
                                session_id: "stream-session".to_string(),
                                timestamp: Some(std::time::SystemTime::now()),
                                cost_usd: None,
                                duration_ms: None,
                                tokens_used: None,
                            },
                        };

                        tx.send(Ok(message)).await.is_err()
                    }
                    StreamFormat::Json => {
                        accumulated_content.push_str(&line);
                        accumulated_content.push('\n');
                        false
                    }
                    StreamFormat::StreamJson => {
                        Self::handle_stream_json_line(&parser, &line, &tx).await
                    }
                };

                if should_break {
                    debug!("Message receiver dropped");
                    break;
                }
            }

            // Handle final processing for non-streaming formats
            match format {
                StreamFormat::Json => {
                    // Try to parse the accumulated content as a single JSON response
                    Self::handle_final_json(&parser, &accumulated_content, &tx).await;
                }
                StreamFormat::Text => {
                    // Send a final message indicating completion
                    let final_message = Message::Result {
                        meta: crate::core::MessageMeta {
                            session_id: "stream-session".to_string(),
                            timestamp: Some(std::time::SystemTime::now()),
                            cost_usd: None,
                            duration_ms: None,
                            tokens_used: None,
                        },
                        stats: ConversationStats {
                            total_messages: 1,
                            total_cost_usd: 0.0,
                            total_duration_ms: 0,
                            total_tokens: TokenUsage {
                                input: 0,
                                output: 0,
                                total: 0,
                            },
                        },
                    };
                    let _ = tx.send(Ok(final_message)).await;
                }
                StreamFormat::StreamJson => {
                    // StreamJson messages are sent as they arrive, no final processing needed
                }
            }
        });

        Self { receiver: rx }
    }

    /// Collects all messages from the stream and returns the full response as a single string.
    pub async fn collect_full_response(mut self) -> Result<String> {
        let config = crate::runtime::stream_config::get_stream_config();
        let mut response = String::with_capacity(config.string_capacity);

        while let Some(result) = self.next().await {
            match result? {
                Message::Assistant { content, .. } => {
                    response.push_str(&content);
                }
                Message::Result { .. } => {
                    // End of conversation
                    break;
                }
                _ => {}
            }
        }

        Ok(response)
    }
}

impl Stream for MessageStream {
    type Item = Result<Message>;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        self.receiver.poll_recv(cx)
    }
}

// ============================================================================
// CLI Stream Envelope types
// ============================================================================

/// Top-level envelope from Claude Code CLI's `--output-format stream-json`.
///
/// The CLI wraps API messages in an envelope like:
/// ```json
/// {"type":"assistant","message":{"content":[...],"usage":{...}},"session_id":"..."}
/// ```
#[derive(Debug, Clone, Deserialize)]
#[allow(dead_code)]
struct CliStreamEnvelope {
    #[serde(rename = "type")]
    envelope_type: String,
    /// Nested API message (present for assistant/user types).
    message: Option<CliApiMessage>,
    session_id: Option<String>,
    // Fields for "result" type envelopes (ClaudeCliResponse format)
    subtype: Option<String>,
    result: Option<String>,
    cost_usd: Option<f64>,
    duration_ms: Option<u64>,
    num_turns: Option<u32>,
    is_error: Option<bool>,
}

/// The nested `message` field inside a CLI stream envelope.
#[derive(Debug, Clone, Deserialize)]
struct CliApiMessage {
    content: Vec<CliContentBlock>,
    #[serde(default)]
    usage: Option<CliUsage>,
}

/// A content block inside the API message.
#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
#[allow(dead_code)]
enum CliContentBlock {
    Text {
        text: String,
    },
    ToolUse {
        id: String,
        name: String,
        input: serde_json::Value,
    },
    ToolResult {
        tool_use_id: String,
        content: Option<serde_json::Value>,
    },
}

/// Token usage from the API message.
#[derive(Debug, Clone, Deserialize)]
struct CliUsage {
    #[serde(default)]
    input_tokens: u64,
    #[serde(default)]
    output_tokens: u64,
}

/// Convert a CLI stream envelope into a `Message`.
fn convert_envelope(env: CliStreamEnvelope) -> Option<Message> {
    let session_id = env.session_id.unwrap_or_default();

    match env.envelope_type.as_str() {
        "assistant" => {
            let msg = env.message?;
            let usage = msg.usage.as_ref();
            let meta = MessageMeta {
                session_id: session_id.clone(),
                timestamp: Some(std::time::SystemTime::now()),
                cost_usd: None,
                duration_ms: None,
                tokens_used: usage.map(|u| TokenUsage {
                    input: u.input_tokens,
                    output: u.output_tokens,
                    total: u.input_tokens + u.output_tokens,
                }),
            };

            // Check for tool_use blocks first
            for block in &msg.content {
                if let CliContentBlock::ToolUse {
                    name,
                    input,
                    ..
                } = block
                {
                    return Some(Message::Tool {
                        name: name.clone(),
                        parameters: input.clone(),
                        meta,
                    });
                }
            }

            // Otherwise concatenate text blocks
            let text: String = msg
                .content
                .iter()
                .filter_map(|b| match b {
                    CliContentBlock::Text { text } => Some(text.as_str()),
                    _ => None,
                })
                .collect::<Vec<_>>()
                .join("");

            Some(Message::Assistant {
                content: text,
                meta,
            })
        }
        "user" => {
            let msg = env.message?;
            let meta = MessageMeta {
                session_id,
                timestamp: Some(std::time::SystemTime::now()),
                cost_usd: None,
                duration_ms: None,
                tokens_used: None,
            };

            // Check for tool_result blocks
            for block in &msg.content {
                if let CliContentBlock::ToolResult {
                    tool_use_id,
                    content,
                } = block
                {
                    return Some(Message::ToolResult {
                        tool_name: tool_use_id.clone(),
                        result: content.clone().unwrap_or(serde_json::Value::Null),
                        meta,
                    });
                }
            }

            // Otherwise treat as user text
            let text: String = msg
                .content
                .iter()
                .filter_map(|b| match b {
                    CliContentBlock::Text { text } => Some(text.as_str()),
                    _ => None,
                })
                .collect::<Vec<_>>()
                .join("");

            Some(Message::User {
                content: text,
                meta,
            })
        }
        "result" => {
            // Result envelope — convert from ClaudeCliResponse-like fields
            let meta = MessageMeta {
                session_id,
                timestamp: Some(std::time::SystemTime::now()),
                cost_usd: env.cost_usd,
                duration_ms: env.duration_ms,
                tokens_used: None,
            };

            // If there's a result string, check if it's an error
            if env.is_error.unwrap_or(false) {
                if let Some(result_text) = env.result {
                    return Some(Message::System {
                        content: result_text,
                        meta,
                    });
                }
            }

            Some(Message::Result {
                meta,
                stats: ConversationStats {
                    total_messages: u64::from(env.num_turns.unwrap_or(0)),
                    total_cost_usd: env.cost_usd.unwrap_or(0.0),
                    total_duration_ms: env.duration_ms.unwrap_or(0),
                    total_tokens: TokenUsage {
                        input: 0,
                        output: 0,
                        total: 0,
                    },
                },
            })
        }
        _ => {
            // Unknown envelope type — skip
            debug!("Skipping unknown CLI stream envelope type: {}", env.envelope_type);
            None
        }
    }
}

/// Parses streaming messages from Claude based on the configured format.
pub struct MessageParser {
    format: StreamFormat,
}

impl MessageParser {
    /// Creates a new message parser for the specified format.
    pub fn new(format: StreamFormat) -> Self {
        Self { format }
    }

    /// Parses a single line of output into a Message, returning None if the line should be skipped.
    pub fn parse_line(&self, line: &str) -> Result<Option<Message>> {
        match self.format {
            StreamFormat::Text => {
                // Text format doesn't have structured messages
                Ok(None)
            }
            StreamFormat::Json | StreamFormat::StreamJson => {
                if line.trim().is_empty() {
                    return Ok(None);
                }

                // Try direct Message deserialization first
                match serde_json::from_str::<Message>(line) {
                    Ok(message) => Ok(Some(message)),
                    Err(_direct_err) => {
                        // Fallback: try CLI stream envelope format
                        match serde_json::from_str::<CliStreamEnvelope>(line) {
                            Ok(envelope) => Ok(convert_envelope(envelope)),
                            Err(envelope_err) => {
                                error!(
                                    "Failed to parse message (tried direct and envelope): {}, line: {}",
                                    envelope_err, line
                                );
                                Err(Error::SerializationError(envelope_err))
                            }
                        }
                    }
                }
            }
        }
    }

    /// Parse accumulated JSON content (for Json format)
    pub fn parse_accumulated_json(&self, content: &str) -> Result<Option<Message>> {
        if content.trim().is_empty() {
            return Ok(None);
        }

        // Try to parse as a direct message first
        if let Ok(message) = serde_json::from_str::<Message>(content) {
            return Ok(Some(message));
        }

        // If that fails, try to parse as a Claude CLI response and extract the result
        if let Ok(cli_response) = serde_json::from_str::<crate::core::ClaudeCliResponse>(content) {
            let message = Message::Assistant {
                content: cli_response.result,
                meta: crate::core::MessageMeta {
                    session_id: "json-response".to_string(),
                    timestamp: Some(std::time::SystemTime::now()),
                    cost_usd: None,
                    duration_ms: None,
                    tokens_used: None,
                },
            };
            return Ok(Some(message));
        }

        // If both fail, create a text message from the raw content
        let message = self.parse_text_response(content);
        Ok(Some(message))
    }

    /// Parses plain text into a Message structure for non-JSON responses.
    pub fn parse_text_response(&self, text: &str) -> Message {
        // For text format, create a simple assistant message
        Message::Assistant {
            content: text.to_string(),
            meta: crate::core::MessageMeta {
                session_id: "text-response".to_string(),
                timestamp: Some(std::time::SystemTime::now()),
                cost_usd: None,
                duration_ms: None,
                tokens_used: None,
            },
        }
    }
}
