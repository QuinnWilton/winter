use std::{collections::HashMap, process::Stdio};

use tokio::{
    io::{AsyncBufReadExt, AsyncWriteExt, BufReader},
    process::Command,
    sync::mpsc,
    time::{timeout, Duration},
};
use tracing::{debug, info};

use crate::{
    core::{Config, Error, Result, StreamFormat},
    runtime::{
        error_handling::{log_error_with_context, ErrorContext, ProcessErrorDetails},
        telemetry,
    },
};

/// Execute a one-shot Claude command with timeout
#[allow(clippy::too_many_lines)]
pub async fn execute_claude(config: &Config, query: &str) -> Result<String> {
    let context = ErrorContext::new("execute_claude")
        .with_debug_info("query_length", query.len().to_string())
        .with_debug_info("stream_format", format!("{:?}", config.stream_format))
        .with_debug_info(
            "timeout_secs",
            config.timeout_secs.unwrap_or(30).to_string(),
        );

    let claude_binary = which::which("claude").map_err(|e| {
        let enhanced_context = context
            .clone()
            .with_error_chain(format!("Binary search failed: {e}"))
            .with_debug_info("search_error", e.to_string());
        let error = Error::BinaryNotFound;
        log_error_with_context(&error, &enhanced_context);

        // Record to telemetry
        let mut telemetry_context = HashMap::new();
        telemetry_context.insert("search_error".to_string(), e.to_string());
        telemetry_context.insert(
            "path_env".to_string(),
            std::env::var("PATH").unwrap_or_default(),
        );
        let error_clone = error.clone();
        tokio::spawn(async move {
            telemetry::record_error(&error_clone, "execute_claude", telemetry_context).await;
        });

        error
    })?;

    let mut cmd = Command::new(claude_binary);

    // Always use non-interactive mode for SDK
    cmd.arg("-p");

    // Add format flag
    match config.stream_format {
        StreamFormat::Json => {
            cmd.arg("--output-format").arg("json");
        }
        StreamFormat::StreamJson => {
            cmd.arg("--output-format").arg("stream-json");
            // stream-json requires verbose flag
            cmd.arg("--verbose");
        }
        StreamFormat::Text => {
            // Text is default, no need to specify
        }
    }

    // Add verbose flag if configured (and not already added for stream-json)
    if config.verbose && config.stream_format != StreamFormat::StreamJson {
        cmd.arg("--verbose");
    }

    // Add optional flags
    if let Some(system_prompt) = &config.system_prompt {
        cmd.arg("--system-prompt").arg(system_prompt);
    }

    if let Some(model) = &config.model {
        cmd.arg("--model").arg(model);
    }

    if let Some(mcp_config_path) = &config.mcp_config_path {
        cmd.arg("--mcp-config").arg(mcp_config_path);
    }

    if let Some(allowed_tools) = &config.allowed_tools {
        for tool in allowed_tools {
            cmd.arg("--allowedTools").arg(tool);
        }
        debug!("Added {} allowed tools", allowed_tools.len());
    }

    if let Some(max_tokens) = &config.max_tokens {
        cmd.arg("--max-tokens").arg(max_tokens.to_string());
    }

    // Determine if we should use stdin or command argument
    let use_stdin =
        config.allowed_tools.is_some() && !config.allowed_tools.as_ref().unwrap().is_empty();

    if use_stdin {
        // When tools are present, use stdin for the query
        cmd.stdin(Stdio::piped());
        cmd.stdout(Stdio::piped());
        cmd.stderr(Stdio::piped());

        info!(
            "Executing Claude CLI with MCP tools (query length: {} chars)",
            query.len()
        );
        debug!("Full command: {:?}", cmd);

        let timeout_secs = config.timeout_secs.unwrap_or(30);
        let timeout_duration = Duration::from_secs(timeout_secs);
        let mut child = cmd.spawn().map_err(|e| {
            let cmd_line = format!("{cmd:?}");
            let error_details = ProcessErrorDetails::new(
                format!("Failed to spawn Claude process: {e}"),
                "claude",
                vec![],
            )
            .with_stderr(e.to_string());

            let enhanced_context = context
                .clone()
                .with_error_chain(format!("Process spawn failed: {e}"))
                .with_debug_info("command_line", cmd_line)
                .with_debug_info("spawn_error", e.to_string());

            let process_error = error_details.to_error();
            log_error_with_context(&process_error, &enhanced_context);
            process_error
        })?;

        // Write the query to stdin.
        if let Some(stdin) = child.stdin.take() {
            let mut stdin = stdin;
            stdin
                .write_all(query.as_bytes())
                .await
                .map_err(|e| Error::ProcessError(format!("Failed to write to stdin: {e}")))?;
            stdin
                .flush()
                .await
                .map_err(|e| Error::ProcessError(format!("Failed to flush stdin: {e}")))?;
            drop(stdin); // Close stdin.
        }

        // Take stdout/stderr before waiting so we can read them.
        let stdout_handle = child.stdout.take();
        let stderr_handle = child.stderr.take();

        // Wait for the process to complete with timeout.
        // If timeout occurs, kill the child process to prevent zombies.
        let wait_result = timeout(timeout_duration, child.wait()).await;

        let status = match wait_result {
            Ok(Ok(status)) => status,
            Ok(Err(e)) => {
                return Err(Error::ProcessError(format!(
                    "Failed to wait for process: {e}"
                )));
            }
            Err(_) => {
                // Timeout occurred - kill the child process.
                let _ = child.kill().await;
                let _ = child.wait().await; // Reap the zombie.

                let error = Error::Timeout(timeout_secs);

                // Record timeout to telemetry.
                let mut telemetry_context = HashMap::new();
                telemetry_context.insert("timeout_duration".to_string(), timeout_secs.to_string());
                telemetry_context.insert("query_length".to_string(), query.len().to_string());
                telemetry_context.insert(
                    "stream_format".to_string(),
                    format!("{:?}", config.stream_format),
                );
                let error_clone = error.clone();
                tokio::spawn(async move {
                    telemetry::record_error(&error_clone, "execute_claude", telemetry_context)
                        .await;
                });

                return Err(error);
            }
        };

        // Read stdout and stderr now that process has exited.
        let mut stdout_content = Vec::new();
        let mut stderr_content = Vec::new();
        if let Some(mut stdout) = stdout_handle {
            let _ = tokio::io::AsyncReadExt::read_to_end(&mut stdout, &mut stdout_content).await;
        }
        if let Some(mut stderr) = stderr_handle {
            let _ = tokio::io::AsyncReadExt::read_to_end(&mut stderr, &mut stderr_content).await;
        }

        // Log stderr even on success for debugging
        let stderr = String::from_utf8_lossy(&stderr_content);
        if !stderr.is_empty() {
            debug!("Claude CLI stderr: {}", stderr);
        }

        if !status.success() {
            let stdout = String::from_utf8_lossy(&stdout_content);

            let error_details =
                ProcessErrorDetails::new("Claude command execution failed", "claude", vec![])
                    .with_exit_code(status.code().unwrap_or(-1))
                    .with_stderr(stderr.to_string())
                    .with_stdout_preview(stdout.to_string());

            let enhanced_context = context
                .clone()
                .with_error_chain("Process completed with non-zero exit code".to_string())
                .with_debug_info("exit_code", status.code().unwrap_or(-1).to_string())
                .with_debug_info("stderr_length", stderr.len().to_string())
                .with_debug_info("stdout_length", stdout.len().to_string());

            let process_error = error_details.to_error();
            log_error_with_context(&process_error, &enhanced_context);
            return Err(process_error);
        }

        let stdout = String::from_utf8_lossy(&stdout_content).to_string();
        Ok(stdout)
    } else {
        // Traditional approach - add query as command argument.
        cmd.arg(query);
        cmd.stdout(Stdio::piped());
        cmd.stderr(Stdio::piped());

        debug!("Executing Claude command: {:?}", cmd);

        let timeout_secs = config.timeout_secs.unwrap_or(30);
        let timeout_duration = Duration::from_secs(timeout_secs);

        let mut child = cmd
            .spawn()
            .map_err(|e| Error::ProcessError(format!("Failed to spawn process: {e}")))?;

        // Take stdout/stderr before waiting.
        let stdout_handle = child.stdout.take();
        let stderr_handle = child.stderr.take();

        // Wait with timeout, kill if needed.
        let wait_result = timeout(timeout_duration, child.wait()).await;

        let status = match wait_result {
            Ok(Ok(status)) => status,
            Ok(Err(e)) => {
                return Err(Error::ProcessError(format!(
                    "Failed to wait for process: {e}"
                )));
            }
            Err(_) => {
                // Timeout - kill the process.
                let _ = child.kill().await;
                let _ = child.wait().await;
                return Err(Error::Timeout(timeout_secs));
            }
        };

        // Read output after process exits.
        let mut stdout_content = Vec::new();
        let mut stderr_content = Vec::new();
        if let Some(mut stdout) = stdout_handle {
            let _ = tokio::io::AsyncReadExt::read_to_end(&mut stdout, &mut stdout_content).await;
        }
        if let Some(mut stderr) = stderr_handle {
            let _ = tokio::io::AsyncReadExt::read_to_end(&mut stderr, &mut stderr_content).await;
        }

        // Log stderr even on success for debugging
        let stderr = String::from_utf8_lossy(&stderr_content);
        if !stderr.is_empty() {
            debug!("Claude CLI stderr (traditional mode): {}", stderr);
        }

        if !status.success() {
            let stdout = String::from_utf8_lossy(&stdout_content);

            let error_details = ProcessErrorDetails::new(
                "Claude command execution failed (traditional mode)",
                "claude",
                vec![],
            )
            .with_exit_code(status.code().unwrap_or(-1))
            .with_stderr(stderr.to_string())
            .with_stdout_preview(stdout.to_string());

            let enhanced_context = context
                .clone()
                .with_error_chain(
                    "Process completed with non-zero exit code (traditional mode)".to_string(),
                )
                .with_debug_info("execution_mode", "traditional")
                .with_debug_info("exit_code", status.code().unwrap_or(-1).to_string())
                .with_debug_info("stderr_length", stderr.len().to_string());

            let process_error = error_details.to_error();
            log_error_with_context(&process_error, &enhanced_context);
            return Err(process_error);
        }

        let stdout = String::from_utf8_lossy(&stdout_content).to_string();
        Ok(stdout)
    }
}

/// Execute Claude command with streaming output
///
/// This function spawns a Claude CLI process and returns a stream of output lines.
/// Unlike `execute_claude`, this function provides real-time streaming of the output
/// as it's generated by the CLI process.
///
/// # Streaming Behavior
///
/// The streaming implementation reads Claude CLI output line-by-line and forwards
/// each line as it arrives. This provides true real-time streaming for:
///
/// - **Text format**: Each line is sent as a separate message chunk
/// - **JSON format**: Full response is accumulated and sent once complete
/// - **StreamJson format**: Each JSON line is parsed and sent as individual messages
///
/// # Limitations
///
/// - **Process cleanup**: Child processes are cleaned up when receivers are dropped,
///   but very long-running streams should be manually cancelled
/// - **Buffering**: Output is line-buffered, so partial lines won't be streamed
/// - **Error handling**: Process errors are sent through the stream, but some
///   errors (like authentication failures) may only appear in stderr
/// - **Timeout behavior**: Timeouts apply per-line, not to the entire response
///
/// # Arguments
///
/// * `config` - Configuration for the Claude CLI execution
/// * `query` - The query to send to Claude
///
/// # Returns
///
/// Returns a `Result` containing a receiver that yields `String` lines as they are
/// output by the Claude CLI process.
///
/// # Examples
///
/// ```rust,no_run
/// use crate::core::Config;
/// use claude_sdk_rs_runtime::process::execute_claude_streaming;
/// use futures::StreamExt;
///
/// #[tokio::main]
/// async fn main() -> Result<(), Box<dyn std::error::Error>> {
///     let config = Config::default();
///     let mut stream = execute_claude_streaming(&config, "Tell me a story").await?;
///     
///     while let Some(line_result) = stream.recv().await {
///         match line_result {
///             Ok(line) => println!("Received: {}", line),
///             Err(e) => eprintln!("Error: {}", e),
///         }
///     }
///     
///     Ok(())
/// }
/// ```
#[allow(clippy::too_many_lines)]
pub async fn execute_claude_streaming(
    config: &Config,
    query: &str,
) -> Result<mpsc::Receiver<Result<String>>> {
    let claude_binary = which::which("claude").map_err(|_| Error::BinaryNotFound)?;

    let mut cmd = Command::new(claude_binary);

    // Always use non-interactive mode for SDK
    cmd.arg("-p");

    // Add format flag
    match config.stream_format {
        StreamFormat::Json => {
            cmd.arg("--output-format").arg("json");
        }
        StreamFormat::StreamJson => {
            cmd.arg("--output-format").arg("stream-json");
            // stream-json requires verbose flag
            cmd.arg("--verbose");
        }
        StreamFormat::Text => {
            // Text is default, no need to specify
        }
    }

    // Add verbose flag if configured (and not already added for stream-json)
    if config.verbose && config.stream_format != StreamFormat::StreamJson {
        cmd.arg("--verbose");
    }

    // Add optional flags
    if let Some(system_prompt) = &config.system_prompt {
        cmd.arg("--system-prompt").arg(system_prompt);
    }

    if let Some(model) = &config.model {
        cmd.arg("--model").arg(model);
    }

    if let Some(mcp_config_path) = &config.mcp_config_path {
        cmd.arg("--mcp-config").arg(mcp_config_path);
    }

    if let Some(allowed_tools) = &config.allowed_tools {
        for tool in allowed_tools {
            cmd.arg("--allowedTools").arg(tool);
        }
        debug!("Added {} allowed tools", allowed_tools.len());
    }

    if let Some(max_tokens) = &config.max_tokens {
        cmd.arg("--max-tokens").arg(max_tokens.to_string());
    }

    // Set up stdio for streaming
    cmd.stdin(Stdio::piped());
    cmd.stdout(Stdio::piped());
    cmd.stderr(Stdio::piped());

    debug!("Executing Claude command for streaming: {:?}", cmd);

    let mut child = cmd
        .spawn()
        .map_err(|e| Error::ProcessError(format!("Failed to spawn process: {e}")))?;

    // Write the query to stdin synchronously before returning stream.
    // This ensures write errors are propagated to the caller.
    if let Some(mut stdin) = child.stdin.take() {
        stdin
            .write_all(query.as_bytes())
            .await
            .map_err(|e| Error::ProcessError(format!("Failed to write to stdin: {e}")))?;
        stdin
            .flush()
            .await
            .map_err(|e| Error::ProcessError(format!("Failed to flush stdin: {e}")))?;
        // Drop stdin to close it and signal EOF to the child.
        drop(stdin);
    }

    // Create channel for streaming output.
    let stream_config = crate::runtime::stream_config::get_stream_config();
    let (tx, rx) = mpsc::channel::<Result<String>>(stream_config.channel_buffer_size);

    // Wrap child in Arc<Mutex> so it can be killed from the timeout handler.
    let child = std::sync::Arc::new(tokio::sync::Mutex::new(child));
    let child_for_reader = child.clone();
    let child_for_monitor = child.clone();

    let timeout_secs = config.timeout_secs.unwrap_or(30);
    let timeout_duration = Duration::from_secs(timeout_secs);

    // Spawn task to read stdout line by line.
    let stdout = {
        let mut guard = child.lock().await;
        guard.stdout.take()
    };

    if let Some(stdout) = stdout {
        let tx_clone = tx.clone();

        tokio::spawn(async move {
            let reader = BufReader::new(stdout);
            let mut lines = reader.lines();

            loop {
                let line_result = timeout(timeout_duration, lines.next_line()).await;

                let (should_continue, was_successful_line) = match line_result {
                    Ok(Ok(Some(line))) => {
                        let can_send = tx_clone.send(Ok(line)).await.is_ok();
                        (can_send, true)
                    }
                    Ok(Ok(None)) => {
                        debug!("Reached EOF on stdout");
                        (false, false)
                    }
                    Ok(Err(e)) => {
                        let _ = tx_clone
                            .send(Err(Error::ProcessError(format!(
                                "Failed to read line: {e}"
                            ))))
                            .await;
                        (false, false)
                    }
                    Err(_) => {
                        let mut guard = child_for_reader.lock().await;
                        let _ = guard.kill().await;
                        let _ = tx_clone.send(Err(Error::Timeout(timeout_secs))).await;
                        (false, false)
                    }
                };

                // Log if receiver dropped mid-line, then break if needed
                if !should_continue && was_successful_line {
                    debug!("Receiver dropped, stopping stdout reading");
                }

                if !should_continue {
                    break;
                }
            }
        });
    }

    // Spawn task to monitor process completion and handle errors.
    // This task waits for the process to exit and reports non-zero exit codes.
    tokio::spawn(async move {
        let status = {
            let mut guard = child_for_monitor.lock().await;
            guard.wait().await
        };

        match status {
            Ok(status) if !status.success() => {
                let exit_code = status.code().unwrap_or(-1);
                let _ = tx
                    .send(Err(Error::ProcessError(format!(
                        "Claude command failed with exit code {exit_code}"
                    ))))
                    .await;
            }
            Err(e) => {
                let _ = tx
                    .send(Err(Error::ProcessError(format!("Process error: {e}"))))
                    .await;
            }
            Ok(_) => {
                // Process completed successfully, stdout task handles EOF.
                debug!("Claude process completed successfully");
            }
        }
    });

    Ok(rx)
}

/// Execute a Claude command with automatic retry for recoverable errors.
///
/// This wraps `execute_claude` with exponential backoff retry logic for
/// errors that are marked as recoverable (timeouts, rate limits, etc.).
///
/// # Arguments
///
/// * `config` - Configuration for the Claude CLI execution
/// * `query` - The query to send to Claude
/// * `max_attempts` - Maximum number of retry attempts (default: 3)
///
/// # Example
///
/// ```rust,no_run
/// use claude_sdk_rs::core::Config;
/// use claude_sdk_rs::runtime::process::execute_claude_with_retry;
///
/// #[tokio::main]
/// async fn main() -> Result<(), Box<dyn std::error::Error>> {
///     let config = Config::default();
///     let response = execute_claude_with_retry(&config, "Hello", 3).await?;
///     println!("{}", response);
///     Ok(())
/// }
/// ```
pub async fn execute_claude_with_retry(
    config: &Config,
    query: &str,
    max_attempts: usize,
) -> Result<String> {
    use crate::runtime::error_handling::{retry_with_backoff, RetryConfig};

    let retry_config = RetryConfig {
        max_attempts,
        base_delay: Duration::from_millis(500),
        max_delay: Duration::from_secs(30),
        backoff_multiplier: 2.0,
        add_jitter: true,
    };

    // Clone config and query for the closure.
    let config = config.clone();
    let query = query.to_string();

    retry_with_backoff(
        || {
            let config = config.clone();
            let query = query.clone();
            async move { execute_claude(&config, &query).await }
        },
        retry_config,
        "execute_claude",
    )
    .await
}
