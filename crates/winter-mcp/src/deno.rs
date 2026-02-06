//! Deno executor for sandboxed custom tool execution.
//!
//! This module provides a secure sandbox for running custom JavaScript/TypeScript
//! tools using Deno's permission model.

use std::collections::HashMap;
use std::path::PathBuf;
use std::process::Stdio;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use serde_json::Value;
use tempfile::NamedTempFile;
use thiserror::Error;
use tokio::io::AsyncWriteExt;
use tokio::process::Command;
use tracing::{debug, warn};

/// Errors from Deno execution.
#[derive(Debug, Error)]
pub enum DenoError {
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    #[error("JSON error: {0}")]
    Json(#[from] serde_json::Error),

    #[error("execution timeout after {0}ms")]
    Timeout(u64),

    #[error("Deno execution failed: {0}")]
    ExecutionFailed(String),

    #[error("Deno not found - is Deno installed?")]
    DenoNotFound,

    #[error("invalid tool output: {0}")]
    InvalidOutput(String),
}

/// Workspace access permissions.
#[derive(Debug, Clone)]
pub struct WorkspacePermission {
    /// The workspace directory path.
    pub path: PathBuf,
    /// Whether read access is granted.
    pub read: bool,
    /// Whether write access is granted.
    pub write: bool,
}

/// Permissions granted to a Deno tool.
#[derive(Debug, Clone, Default)]
pub struct DenoPermissions {
    /// Whether the tool can access the network.
    pub network: bool,
    /// Secrets to expose as environment variables.
    /// Keys are env var names (e.g., "WINTER_SECRET_API_KEY").
    pub secrets: HashMap<String, String>,
    /// Workspace directory access.
    pub workspace: Option<WorkspacePermission>,
    /// Subprocess commands the tool can run (e.g., ["git"]).
    pub allowed_commands: Vec<String>,
    /// MCP tools this tool is allowed to call via chaining.
    /// When non-empty, a helper module is generated and MCP URL/token are passed.
    pub allowed_tools: Vec<String>,
    /// Mapping from tool name to AT URI for custom tools in allowed_tools.
    /// Allows calling tools by name (e.g., "color_namer") instead of AT URI.
    pub tool_name_map: HashMap<String, String>,
    /// Token for authenticating to the /mcp/internal endpoint.
    pub tool_token: Option<String>,
    /// URL of the MCP server's internal endpoint.
    pub mcp_url: Option<String>,
}

/// Output from a Deno tool execution.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DenoOutput {
    /// The parsed JSON result from the tool.
    pub result: Value,
    /// Standard output from the tool (for debugging).
    pub stdout: String,
    /// Standard error from the tool (for debugging).
    pub stderr: String,
    /// Execution duration in milliseconds.
    pub duration_ms: u64,
}

/// Executor for Deno-based custom tools.
#[derive(Debug, Clone)]
pub struct DenoExecutor {
    /// Default timeout for tool execution.
    timeout: Duration,
}

impl Default for DenoExecutor {
    fn default() -> Self {
        Self {
            timeout: Duration::from_secs(30),
        }
    }
}

impl DenoExecutor {
    /// Create a new executor with custom timeout.
    pub fn new(timeout: Duration) -> Self {
        Self { timeout }
    }

    /// Execute a tool with the given code, input, and permissions.
    ///
    /// The tool code should export a default async function:
    /// ```typescript
    /// export default async function(input: T): Promise<R> {
    ///   // implementation
    /// }
    /// ```
    pub async fn execute(
        &self,
        code: &str,
        input: &Value,
        permissions: DenoPermissions,
    ) -> Result<DenoOutput, DenoError> {
        let start = Instant::now();

        // Create temp file for the tool code
        let tool_file = NamedTempFile::new()?;
        tokio::fs::write(tool_file.path(), code).await?;

        // Build list of secret env var names for the context object
        let secret_names: Vec<&str> = permissions.secrets.keys().map(|s| s.as_str()).collect();
        let secret_names_json =
            serde_json::to_string(&secret_names).unwrap_or_else(|_| "[]".to_string());

        // Generate tool chaining helper if allowed_tools is non-empty
        let tool_chaining_code = if !permissions.allowed_tools.is_empty() {
            let allowed_tools_json = serde_json::to_string(&permissions.allowed_tools)
                .unwrap_or_else(|_| "[]".to_string());
            let name_map_json = serde_json::to_string(&permissions.tool_name_map)
                .unwrap_or_else(|_| "{{}}".to_string());
            format!(
                r#"
// Tool chaining helper - allows calling other MCP tools
const _mcpUrl = Deno.env.get("WINTER_MCP_URL") || "";
const _toolToken = Deno.env.get("WINTER_TOOL_TOKEN") || "";
const _allowedTools: string[] = {allowed_tools};
const _toolNameMap: Record<string, string> = {name_map};

// Resolve a tool reference: names get mapped to AT URIs if known.
function _resolveToolRef(toolRef: string): string {{
    if (_toolNameMap[toolRef]) return _toolNameMap[toolRef];
    return toolRef;
}}

// Call a tool by name or AT URI.
// Custom tools can be called by name (e.g., "color_namer")
// or AT URI (e.g., "at://did:plc:xxx/diy.razorgirl.winter.tool/rkey").
// Built-in MCP tools use plain names (e.g., "query_facts").
async function callTool(toolRef: string, args: Record<string, unknown>): Promise<unknown> {{
    const resolved = _resolveToolRef(toolRef);
    if (!_allowedTools.includes(resolved)) {{
        throw new Error(`Tool '${{toolRef}}' is not in the allowed tools list: ${{_allowedTools.join(", ")}}`);
    }}
    if (!_mcpUrl || !_toolToken) {{
        throw new Error("Tool chaining not configured (missing MCP URL or token)");
    }}
    const resp = await fetch(`${{_mcpUrl}}/mcp/internal`, {{
        method: "POST",
        headers: {{
            "Content-Type": "application/json",
            "X-Tool-Token": _toolToken,
        }},
        body: JSON.stringify({{ tool_ref: resolved, arguments: args }}),
    }});
    if (!resp.ok) {{
        const text = await resp.text();
        throw new Error(`Tool call failed (${{resp.status}}): ${{text}}`);
    }}
    const result = await resp.json();
    if (!result.success) {{
        throw new Error(result.error || "Tool call failed");
    }}
    return result.result;
}}
"#,
                allowed_tools = allowed_tools_json,
                name_map = name_map_json,
            )
        } else {
            String::new()
        };

        // Create wrapper that imports the tool and handles stdin/stdout
        let wrapper_code = format!(
            r#"
import tool from "file://{}";
{tool_chaining}
async function readStdin(): Promise<string> {{
    const buf = new Uint8Array(1024 * 1024); // 1MB buffer
    let totalRead = 0;
    const chunks: Uint8Array[] = [];

    while (true) {{
        const n = await Deno.stdin.read(buf);
        if (n === null) break;
        chunks.push(buf.slice(0, n));
        totalRead += n;
    }}

    const combined = new Uint8Array(totalRead);
    let offset = 0;
    for (const chunk of chunks) {{
        combined.set(chunk, offset);
        offset += chunk.length;
    }}

    return new TextDecoder().decode(combined);
}}

// Build context object with secrets
const secretNames: string[] = {secret_names};
const secrets: Record<string, string> = {{}};
for (const name of secretNames) {{
    const value = Deno.env.get(name);
    if (value !== undefined) {{
        // Strip WINTER_SECRET_ prefix for cleaner access
        const shortName = name.replace(/^WINTER_SECRET_/, "");
        secrets[shortName] = value;
    }}
}}

const context = {{
    secrets,
    workspace: Deno.env.get("WINTER_WORKSPACE") || null,
    callTool: typeof callTool !== "undefined" ? callTool : undefined,
}};

const inputText = await readStdin();
const input = JSON.parse(inputText);

try {{
    const result = await tool(input, context);
    console.log(JSON.stringify({{ success: true, result }}));
}} catch (error) {{
    console.log(JSON.stringify({{ success: false, error: error.message || String(error) }}));
}}
"#,
            tool_file.path().display(),
            tool_chaining = tool_chaining_code,
            secret_names = secret_names_json
        );

        let wrapper_file = NamedTempFile::new()?;
        tokio::fs::write(wrapper_file.path(), &wrapper_code).await?;

        // Build Deno command with permissions
        let mut cmd = Command::new("deno");
        cmd.arg("run");

        // Always deny by default
        cmd.arg("--no-prompt");

        if permissions.network {
            cmd.arg("--allow-net");
        } else if !permissions.allowed_tools.is_empty() {
            // Tool chaining needs localhost access even without general network
            cmd.arg("--allow-net=127.0.0.1,localhost");
        }

        // Build environment variable permissions
        let mut env_vars: Vec<&str> = Vec::new();

        // Network operations need access to proxy env vars
        if permissions.network {
            env_vars.extend(&[
                "HTTP_PROXY",
                "HTTPS_PROXY",
                "NO_PROXY",
                "http_proxy",
                "https_proxy",
                "no_proxy",
            ]);
        }

        // Add secret env vars
        for key in permissions.secrets.keys() {
            env_vars.push(key.as_str());
        }

        // Always allow WINTER_WORKSPACE read (will be empty if not granted)
        env_vars.push("WINTER_WORKSPACE");

        // Tool chaining env vars
        if !permissions.allowed_tools.is_empty() {
            env_vars.push("WINTER_MCP_URL");
            env_vars.push("WINTER_TOOL_TOKEN");
        }

        if !env_vars.is_empty() {
            cmd.arg(format!("--allow-env={}", env_vars.join(",")));
        }

        // Build --allow-read paths
        let cert_paths = if cfg!(target_os = "linux") {
            ",/etc/ssl/certs,/etc/pki/tls/certs"
        } else if cfg!(target_os = "macos") {
            ",/etc/ssl/cert.pem,/private/etc/ssl"
        } else {
            ""
        };

        let mut read_paths = format!(
            "{},{}{}",
            tool_file.path().display(),
            wrapper_file.path().display(),
            if permissions.network { cert_paths } else { "" }
        );

        // Add workspace read permission if granted
        if let Some(ref workspace) = permissions.workspace
            && workspace.read
        {
            read_paths.push(',');
            read_paths.push_str(&workspace.path.display().to_string());
        }

        cmd.arg(format!("--allow-read={}", read_paths));

        // Add workspace write permission if granted
        if let Some(ref workspace) = permissions.workspace
            && workspace.write
        {
            cmd.arg(format!("--allow-write={}", workspace.path.display()));
        }

        // Add subprocess command permissions if granted
        if !permissions.allowed_commands.is_empty() {
            cmd.arg(format!(
                "--allow-run={}",
                permissions.allowed_commands.join(",")
            ));
        }

        cmd.arg(wrapper_file.path());

        // Clear inherited environment for isolation, then add only what's needed
        cmd.env_clear();

        // Add secrets
        cmd.envs(&permissions.secrets);

        // Add workspace path as environment variable if workspace access is granted
        if let Some(ref workspace) = permissions.workspace {
            cmd.env("WINTER_WORKSPACE", &workspace.path);
        }

        // Add tool chaining env vars
        if let Some(ref mcp_url) = permissions.mcp_url {
            cmd.env("WINTER_MCP_URL", mcp_url);
        }
        if let Some(ref token) = permissions.tool_token {
            cmd.env("WINTER_TOOL_TOKEN", token);
        }

        // Deno-specific settings
        cmd.env("DENO_NO_UPDATE_CHECK", "1");
        // Hint to use single DNS requests (may help with IPv4/IPv6 race conditions)
        cmd.env("RES_OPTIONS", "single-request");

        // Preserve necessary system paths
        if let Ok(path) = std::env::var("PATH") {
            cmd.env("PATH", path);
        }
        if let Ok(home) = std::env::var("HOME") {
            cmd.env("HOME", home);
        }
        // TLS needs to find system certs
        if cfg!(target_os = "linux") {
            cmd.env("SSL_CERT_DIR", "/etc/ssl/certs");
        }

        // Configure stdio
        cmd.stdin(Stdio::piped());
        cmd.stdout(Stdio::piped());
        cmd.stderr(Stdio::piped());

        debug!(tool_path = %tool_file.path().display(), "executing Deno tool");

        // Spawn process
        let mut child = cmd.spawn().map_err(|e| {
            if e.kind() == std::io::ErrorKind::NotFound {
                DenoError::DenoNotFound
            } else {
                DenoError::Io(e)
            }
        })?;

        // Write input to stdin
        let input_json = serde_json::to_string(input)?;
        if let Some(mut stdin) = child.stdin.take() {
            stdin.write_all(input_json.as_bytes()).await?;
            stdin.shutdown().await?;
        }

        // Wait with timeout
        let output = tokio::time::timeout(self.timeout, child.wait_with_output())
            .await
            .map_err(|_| DenoError::Timeout(self.timeout.as_millis() as u64))??;

        let duration_ms = start.elapsed().as_millis() as u64;

        let stdout = String::from_utf8_lossy(&output.stdout).to_string();
        let stderr = String::from_utf8_lossy(&output.stderr).to_string();

        if !output.status.success() {
            warn!(
                exit_code = ?output.status.code(),
                stderr = %stderr,
                "Deno tool execution failed"
            );
            return Err(DenoError::ExecutionFailed(stderr));
        }

        // Parse the wrapper's JSON output
        let wrapper_output: WrapperOutput = serde_json::from_str(&stdout).map_err(|e| {
            DenoError::InvalidOutput(format!(
                "failed to parse tool output: {} (stdout: {})",
                e, stdout
            ))
        })?;

        if !wrapper_output.success {
            return Err(DenoError::ExecutionFailed(
                wrapper_output
                    .error
                    .unwrap_or_else(|| "unknown error".to_string()),
            ));
        }

        Ok(DenoOutput {
            result: wrapper_output.result.unwrap_or(Value::Null),
            stdout,
            stderr,
            duration_ms,
        })
    }

    /// Check if Deno is available on the system.
    pub async fn is_available() -> bool {
        Command::new("deno")
            .arg("--version")
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .await
            .map(|s| s.success())
            .unwrap_or(false)
    }
}

/// Internal wrapper output format.
#[derive(Debug, Deserialize)]
struct WrapperOutput {
    success: bool,
    result: Option<Value>,
    error: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    async fn deno_available() -> bool {
        DenoExecutor::is_available().await
    }

    #[tokio::test]
    async fn simple_tool_execution() {
        if !deno_available().await {
            eprintln!("Skipping test - Deno not available");
            return;
        }

        let executor = DenoExecutor::default();

        let code = r#"
export default async function(input: { x: number, y: number }): Promise<{ sum: number }> {
    return { sum: input.x + input.y };
}
"#;

        let input = json!({ "x": 2, "y": 3 });
        let result = executor
            .execute(code, &input, DenoPermissions::default())
            .await
            .unwrap();

        assert_eq!(result.result, json!({ "sum": 5 }));
    }

    #[tokio::test]
    async fn tool_with_env_secrets() {
        if !deno_available().await {
            eprintln!("Skipping test - Deno not available");
            return;
        }

        let executor = DenoExecutor::default();

        let code = r#"
export default async function(_input: {}): Promise<{ key: string }> {
    const key = Deno.env.get("WINTER_SECRET_TEST_KEY") || "not_found";
    return { key };
}
"#;

        let mut secrets = HashMap::new();
        secrets.insert(
            "WINTER_SECRET_TEST_KEY".to_string(),
            "secret123".to_string(),
        );

        let permissions = DenoPermissions {
            network: false,
            secrets,
            workspace: None,
            allowed_commands: Vec::new(),
            ..Default::default()
        };

        let result = executor
            .execute(code, &json!({}), permissions)
            .await
            .unwrap();

        assert_eq!(result.result, json!({ "key": "secret123" }));
    }

    #[tokio::test]
    async fn tool_with_context_secrets() {
        if !deno_available().await {
            eprintln!("Skipping test - Deno not available");
            return;
        }

        let executor = DenoExecutor::default();

        // Tool uses context.secrets with stripped prefix
        let code = r#"
export default async function(_input: {}, context: { secrets: Record<string, string> }): Promise<{ key: string }> {
    return { key: context.secrets["TEST_KEY"] || "not_found" };
}
"#;

        let mut secrets = HashMap::new();
        secrets.insert(
            "WINTER_SECRET_TEST_KEY".to_string(),
            "secret456".to_string(),
        );

        let permissions = DenoPermissions {
            network: false,
            secrets,
            workspace: None,
            allowed_commands: Vec::new(),
            ..Default::default()
        };

        let result = executor
            .execute(code, &json!({}), permissions)
            .await
            .unwrap();

        assert_eq!(result.result, json!({ "key": "secret456" }));
    }

    #[tokio::test]
    async fn tool_without_network_fails_fetch() {
        if !deno_available().await {
            eprintln!("Skipping test - Deno not available");
            return;
        }

        let executor = DenoExecutor::default();

        let code = r#"
export default async function(_input: {}): Promise<{ status: number }> {
    const resp = await fetch("https://example.com");
    return { status: resp.status };
}
"#;

        let result = executor
            .execute(code, &json!({}), DenoPermissions::default())
            .await;

        // Should fail because network is not allowed
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn tool_error_handling() {
        if !deno_available().await {
            eprintln!("Skipping test - Deno not available");
            return;
        }

        let executor = DenoExecutor::default();

        let code = r#"
export default async function(_input: {}): Promise<never> {
    throw new Error("intentional error");
}
"#;

        let result = executor
            .execute(code, &json!({}), DenoPermissions::default())
            .await;

        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(matches!(err, DenoError::ExecutionFailed(_)));
    }

    #[tokio::test]
    async fn tool_timeout() {
        if !deno_available().await {
            eprintln!("Skipping test - Deno not available");
            return;
        }

        let executor = DenoExecutor::new(Duration::from_millis(100));

        let code = r#"
export default async function(_input: {}): Promise<{ done: boolean }> {
    await new Promise(resolve => setTimeout(resolve, 5000));
    return { done: true };
}
"#;

        let result = executor
            .execute(code, &json!({}), DenoPermissions::default())
            .await;

        assert!(matches!(result, Err(DenoError::Timeout(_))));
    }
}
