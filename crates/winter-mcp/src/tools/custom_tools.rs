//! Custom tools for MCP.
//!
//! This module provides tools for creating, managing, and running
//! custom JavaScript/TypeScript tools in a sandboxed Deno environment.

use std::collections::HashMap;
use std::sync::Arc;

use chrono::Utc;
use serde_json::{Value, json};
use tokio::sync::RwLock;
use tracing::{info, warn};

use crate::deno::{DenoExecutor, DenoPermissions, WorkspacePermission};
use crate::protocol::{CallToolResult, ToolDefinition};
use crate::secrets::SecretManager;
use winter_atproto::{
    CustomTool, IDENTITY_COLLECTION, IDENTITY_KEY, Identity, SECRET_META_COLLECTION,
    SECRET_META_KEY, SecretEntry, SecretMeta, TOOL_APPROVAL_COLLECTION, TOOL_COLLECTION, Tid,
    ToolApproval, ToolApprovalStatus,
};

use super::ToolState;

/// Maximum code size (64KB).
const MAX_CODE_SIZE: usize = 64 * 1024;

/// Get the web URL for tool approval (from env or default).
fn web_url() -> String {
    std::env::var("WINTER_WEB_URL").unwrap_or_else(|_| "http://localhost:8080".to_string())
}

pub fn definitions() -> Vec<ToolDefinition> {
    vec![
        ToolDefinition {
            name: "create_custom_tool".to_string(),
            description: "Create a new custom JavaScript/TypeScript tool. The operator will be notified for approval. Tools run sandboxed until approved.".to_string(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "name": {
                        "type": "string",
                        "description": "Tool name (used to invoke the tool, alphanumeric and underscores)"
                    },
                    "description": {
                        "type": "string",
                        "description": "Human-readable description of what the tool does"
                    },
                    "code": {
                        "type": "string",
                        "description": "TypeScript/JavaScript source code. Must export a default async function."
                    },
                    "input_schema": {
                        "type": "object",
                        "description": "JSON Schema for the tool's input parameters"
                    },
                    "required_secrets": {
                        "type": "array",
                        "items": { "type": "string" },
                        "description": "Names of secrets this tool needs access to"
                    },
                    "requires_workspace": {
                        "type": "boolean",
                        "description": "Whether this tool needs access to the workspace directory for file operations"
                    },
                    "required_commands": {
                        "type": "array",
                        "items": { "type": "string" },
                        "description": "Subprocess commands this tool needs to run (e.g., ['git'])"
                    }
                },
                "required": ["name", "description", "code", "input_schema"]
            }),
        },
        ToolDefinition {
            name: "update_custom_tool".to_string(),
            description: "Update an existing custom tool's code. This invalidates any existing approval and requires re-approval.".to_string(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "name": {
                        "type": "string",
                        "description": "Name of the tool to update"
                    },
                    "description": {
                        "type": "string",
                        "description": "New description (optional)"
                    },
                    "code": {
                        "type": "string",
                        "description": "New TypeScript/JavaScript source code"
                    },
                    "input_schema": {
                        "type": "object",
                        "description": "New input schema (optional)"
                    },
                    "required_secrets": {
                        "type": "array",
                        "items": { "type": "string" },
                        "description": "New list of required secrets (optional)"
                    },
                    "requires_workspace": {
                        "type": "boolean",
                        "description": "Whether this tool needs access to the workspace directory (optional)"
                    },
                    "required_commands": {
                        "type": "array",
                        "items": { "type": "string" },
                        "description": "Subprocess commands this tool needs to run (optional)"
                    }
                },
                "required": ["name", "code"]
            }),
        },
        ToolDefinition {
            name: "list_custom_tools".to_string(),
            description: "List all custom tools with their approval status.".to_string(),
            input_schema: json!({
                "type": "object",
                "properties": {}
            }),
        },
        ToolDefinition {
            name: "get_custom_tool".to_string(),
            description: "Get details of a specific custom tool including its code.".to_string(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "name": {
                        "type": "string",
                        "description": "Name of the tool"
                    }
                },
                "required": ["name"]
            }),
        },
        ToolDefinition {
            name: "run_custom_tool".to_string(),
            description: "Execute a custom tool. Unapproved tools run sandboxed (no network, no secrets). Approved tools run with granted permissions.".to_string(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "name": {
                        "type": "string",
                        "description": "Name of the tool to run"
                    },
                    "input": {
                        "type": "object",
                        "description": "Input parameters for the tool"
                    }
                },
                "required": ["name", "input"]
            }),
        },
        ToolDefinition {
            name: "delete_custom_tool".to_string(),
            description: "Delete a custom tool.".to_string(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "name": {
                        "type": "string",
                        "description": "Name of the tool to delete"
                    }
                },
                "required": ["name"]
            }),
        },
        ToolDefinition {
            name: "request_secret".to_string(),
            description: "Request a new secret from the operator. The operator will be notified to provide the secret value.".to_string(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "name": {
                        "type": "string",
                        "description": "Secret name (alphanumeric and underscores, used as WINTER_SECRET_{name} in tools)"
                    },
                    "description": {
                        "type": "string",
                        "description": "Description of what the secret is for"
                    }
                },
                "required": ["name", "description"]
            }),
        },
        ToolDefinition {
            name: "list_secrets".to_string(),
            description: "List available secret names (not values).".to_string(),
            input_schema: json!({
                "type": "object",
                "properties": {}
            }),
        },
    ]
}

/// Find a tool by name.
async fn find_tool_by_name(
    state: &ToolState,
    name: &str,
) -> Result<Option<(String, CustomTool)>, String> {
    let tools = state
        .atproto
        .list_all_records::<CustomTool>(TOOL_COLLECTION)
        .await
        .map_err(|e| format!("Failed to list tools: {}", e))?;

    for item in tools {
        if item.value.name == name {
            let rkey = item.uri.split('/').next_back().unwrap_or("").to_string();
            return Ok(Some((rkey, item.value)));
        }
    }
    Ok(None)
}

/// Get approval for a tool.
async fn get_approval(state: &ToolState, tool_rkey: &str) -> Option<ToolApproval> {
    // Approvals are keyed by tool_rkey
    state
        .atproto
        .get_record::<ToolApproval>(TOOL_APPROVAL_COLLECTION, tool_rkey)
        .await
        .ok()
        .map(|r| r.value)
}

/// Check if a tool is approved for its current version.
fn is_approved(approval: &Option<ToolApproval>, tool_version: i32) -> bool {
    approval
        .as_ref()
        .map(|a| a.status == ToolApprovalStatus::Approved && a.tool_version == tool_version)
        .unwrap_or(false)
}

/// Send a DM to the operator about a tool needing approval.
async fn notify_operator(
    state: &ToolState,
    tool_name: &str,
    tool_rkey: &str,
    required_secrets: &[String],
    requires_workspace: bool,
    required_commands: &[String],
) {
    // Get operator DID from identity
    let operator_did = match state
        .atproto
        .get_record::<Identity>(IDENTITY_COLLECTION, IDENTITY_KEY)
        .await
    {
        Ok(record) => record.value.operator_did,
        Err(e) => {
            warn!(error = %e, "Failed to get operator DID for notification");
            return;
        }
    };

    let Some(ref bluesky) = state.bluesky else {
        warn!("Bluesky client not configured, cannot notify operator");
        return;
    };

    let secrets_list = if required_secrets.is_empty() {
        "None".to_string()
    } else {
        required_secrets.join(", ")
    };

    let workspace_info = if requires_workspace {
        "\nRequires workspace: Yes"
    } else {
        ""
    };

    let commands_info = if required_commands.is_empty() {
        String::new()
    } else {
        format!("\nRequired commands: {}", required_commands.join(", "))
    };

    let message = format!(
        "I created/updated a tool \"{}\" that needs your approval.\n\nRequired secrets: {}{}{}\n\nPlease review at {}/tools/{}",
        tool_name,
        secrets_list,
        workspace_info,
        commands_info,
        web_url(),
        tool_rkey
    );

    if let Err(e) = bluesky.send_dm(&operator_did, &message).await {
        warn!(error = %e, "Failed to notify operator about tool");
    } else {
        info!(tool = %tool_name, "Notified operator about tool needing approval");
    }
}

pub async fn create_custom_tool(
    state: &ToolState,
    arguments: &HashMap<String, Value>,
) -> CallToolResult {
    let name = match arguments.get("name").and_then(|v| v.as_str()) {
        Some(n) => n,
        None => return CallToolResult::error("Missing required parameter: name"),
    };

    // Validate name
    if !name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
        return CallToolResult::error("Tool name must be alphanumeric with underscores only");
    }

    if name.len() > 64 {
        return CallToolResult::error("Tool name too long (max 64 chars)");
    }

    // Check if tool already exists
    if let Ok(Some(_)) = find_tool_by_name(state, name).await {
        return CallToolResult::error(format!(
            "Tool '{}' already exists. Use update_custom_tool to modify it.",
            name
        ));
    }

    let description = match arguments.get("description").and_then(|v| v.as_str()) {
        Some(d) => d,
        None => return CallToolResult::error("Missing required parameter: description"),
    };

    let code = match arguments.get("code").and_then(|v| v.as_str()) {
        Some(c) => c,
        None => return CallToolResult::error("Missing required parameter: code"),
    };

    if code.len() > MAX_CODE_SIZE {
        return CallToolResult::error("Code exceeds maximum size of 64KB");
    }

    let input_schema = match arguments.get("input_schema") {
        Some(s) => s.clone(),
        None => return CallToolResult::error("Missing required parameter: input_schema"),
    };

    let required_secrets: Vec<String> = arguments
        .get("required_secrets")
        .and_then(|v| v.as_array())
        .map(|a| {
            a.iter()
                .filter_map(|v| v.as_str().map(String::from))
                .collect()
        })
        .unwrap_or_default();

    let requires_workspace = arguments
        .get("requires_workspace")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);

    let required_commands: Vec<String> = arguments
        .get("required_commands")
        .and_then(|v| v.as_array())
        .map(|a| {
            a.iter()
                .filter_map(|v| v.as_str().map(String::from))
                .collect()
        })
        .unwrap_or_default();

    let now = Utc::now();
    let tool = CustomTool {
        name: name.to_string(),
        description: description.to_string(),
        code: code.to_string(),
        input_schema,
        required_secrets: required_secrets.clone(),
        requires_workspace: if requires_workspace { Some(true) } else { None },
        required_commands: required_commands.clone(),
        version: 1,
        created_at: now,
        last_updated: Some(now),
    };

    let rkey = Tid::now().to_string();

    match state
        .atproto
        .create_record(TOOL_COLLECTION, Some(&rkey), &tool)
        .await
    {
        Ok(response) => {
            // Update cache so subsequent queries see the change immediately
            if let Some(cache) = &state.cache {
                cache.upsert_tool(rkey.clone(), tool.clone(), response.cid.clone());
            }

            // Notify operator
            notify_operator(
                state,
                name,
                &rkey,
                &required_secrets,
                requires_workspace,
                &required_commands,
            )
            .await;

            CallToolResult::success(
                json!({
                    "rkey": rkey,
                    "uri": response.uri,
                    "cid": response.cid,
                    "name": name,
                    "version": 1,
                    "status": "pending_approval",
                    "message": "Tool created. The operator has been notified for approval. You can test it sandboxed with run_custom_tool."
                })
                .to_string(),
            )
        }
        Err(e) => CallToolResult::error(format!("Failed to create tool: {}", e)),
    }
}

pub async fn update_custom_tool(
    state: &ToolState,
    arguments: &HashMap<String, Value>,
) -> CallToolResult {
    let name = match arguments.get("name").and_then(|v| v.as_str()) {
        Some(n) => n,
        None => return CallToolResult::error("Missing required parameter: name"),
    };

    let code = match arguments.get("code").and_then(|v| v.as_str()) {
        Some(c) => c,
        None => return CallToolResult::error("Missing required parameter: code"),
    };

    if code.len() > MAX_CODE_SIZE {
        return CallToolResult::error("Code exceeds maximum size of 64KB");
    }

    // Find existing tool
    let (rkey, mut tool) = match find_tool_by_name(state, name).await {
        Ok(Some(t)) => t,
        Ok(None) => return CallToolResult::error(format!("Tool '{}' not found", name)),
        Err(e) => return CallToolResult::error(e),
    };

    // Update fields
    tool.code = code.to_string();
    tool.version += 1;
    tool.last_updated = Some(Utc::now());

    if let Some(desc) = arguments.get("description").and_then(|v| v.as_str()) {
        tool.description = desc.to_string();
    }

    if let Some(schema) = arguments.get("input_schema") {
        tool.input_schema = schema.clone();
    }

    if let Some(secrets) = arguments.get("required_secrets").and_then(|v| v.as_array()) {
        tool.required_secrets = secrets
            .iter()
            .filter_map(|v| v.as_str().map(String::from))
            .collect();
    }

    if let Some(requires_workspace) = arguments
        .get("requires_workspace")
        .and_then(|v| v.as_bool())
    {
        tool.requires_workspace = if requires_workspace { Some(true) } else { None };
    }

    if let Some(commands) = arguments
        .get("required_commands")
        .and_then(|v| v.as_array())
    {
        tool.required_commands = commands
            .iter()
            .filter_map(|v| v.as_str().map(String::from))
            .collect();
    }

    // Delete any existing approval (code changed = re-approval required)
    if state
        .atproto
        .delete_record(TOOL_APPROVAL_COLLECTION, &rkey)
        .await
        .is_ok()
    {
        // Remove approval from cache
        if let Some(cache) = &state.cache {
            cache.delete_tool_approval(&rkey);
        }
    }

    match state
        .atproto
        .put_record(TOOL_COLLECTION, &rkey, &tool)
        .await
    {
        Ok(response) => {
            // Update cache with the modified tool
            if let Some(cache) = &state.cache {
                cache.upsert_tool(rkey.clone(), tool.clone(), response.cid.clone());
            }

            // Notify operator
            notify_operator(
                state,
                name,
                &rkey,
                &tool.required_secrets,
                tool.requires_workspace.unwrap_or(false),
                &tool.required_commands,
            )
            .await;

            CallToolResult::success(
                json!({
                    "rkey": rkey,
                    "uri": response.uri,
                    "cid": response.cid,
                    "name": name,
                    "version": tool.version,
                    "status": "pending_approval",
                    "message": "Tool updated. Previous approval revoked. The operator has been notified."
                })
                .to_string(),
            )
        }
        Err(e) => CallToolResult::error(format!("Failed to update tool: {}", e)),
    }
}

pub async fn list_custom_tools(
    state: &ToolState,
    _arguments: &HashMap<String, Value>,
) -> CallToolResult {
    let tools = match state
        .atproto
        .list_all_records::<CustomTool>(TOOL_COLLECTION)
        .await
    {
        Ok(t) => t,
        Err(e) => return CallToolResult::error(format!("Failed to list tools: {}", e)),
    };

    let mut formatted = Vec::new();

    for item in tools {
        let rkey = item.uri.split('/').next_back().unwrap_or("");
        let approval = get_approval(state, rkey).await;
        let approved = is_approved(&approval, item.value.version);

        let status = if approved {
            "approved"
        } else if approval.is_some() {
            "outdated_approval" // Approval exists but version doesn't match
        } else {
            "pending"
        };

        formatted.push(json!({
            "rkey": rkey,
            "name": item.value.name,
            "description": item.value.description,
            "version": item.value.version,
            "status": status,
            "required_secrets": item.value.required_secrets,
            "requires_workspace": item.value.requires_workspace,
            "required_commands": item.value.required_commands,
            "allow_network": approval.as_ref().and_then(|a| a.allow_network),
            "allowed_secrets": approval.as_ref().map(|a| &a.allowed_secrets),
            "workspace_path": approval.as_ref().and_then(|a| a.workspace_path.as_ref()),
            "allow_workspace_read": approval.as_ref().and_then(|a| a.allow_workspace_read),
            "allow_workspace_write": approval.as_ref().and_then(|a| a.allow_workspace_write),
            "allowed_commands": approval.as_ref().map(|a| &a.allowed_commands),
        }));
    }

    CallToolResult::success(
        json!({
            "count": formatted.len(),
            "tools": formatted
        })
        .to_string(),
    )
}

pub async fn get_custom_tool(
    state: &ToolState,
    arguments: &HashMap<String, Value>,
) -> CallToolResult {
    let name = match arguments.get("name").and_then(|v| v.as_str()) {
        Some(n) => n,
        None => return CallToolResult::error("Missing required parameter: name"),
    };

    let (rkey, tool) = match find_tool_by_name(state, name).await {
        Ok(Some(t)) => t,
        Ok(None) => return CallToolResult::error(format!("Tool '{}' not found", name)),
        Err(e) => return CallToolResult::error(e),
    };

    let approval = get_approval(state, &rkey).await;
    let approved = is_approved(&approval, tool.version);

    CallToolResult::success(
        json!({
            "rkey": rkey,
            "name": tool.name,
            "description": tool.description,
            "code": tool.code,
            "input_schema": tool.input_schema,
            "required_secrets": tool.required_secrets,
            "requires_workspace": tool.requires_workspace,
            "required_commands": tool.required_commands,
            "version": tool.version,
            "approved": approved,
            "approval": approval.map(|a| json!({
                "status": format!("{:?}", a.status).to_lowercase(),
                "tool_version": a.tool_version,
                "allow_network": a.allow_network,
                "allowed_secrets": a.allowed_secrets,
                "workspace_path": a.workspace_path,
                "allow_workspace_read": a.allow_workspace_read,
                "allow_workspace_write": a.allow_workspace_write,
                "allowed_commands": a.allowed_commands,
                "reason": a.reason,
            })),
            "created_at": tool.created_at.to_rfc3339(),
            "last_updated": tool.last_updated.map(|t| t.to_rfc3339()),
        })
        .to_string(),
    )
}

pub async fn run_custom_tool(
    state: &ToolState,
    secrets: Option<&Arc<RwLock<SecretManager>>>,
    deno: Option<&DenoExecutor>,
    arguments: &HashMap<String, Value>,
) -> CallToolResult {
    let name = match arguments.get("name").and_then(|v| v.as_str()) {
        Some(n) => n,
        None => return CallToolResult::error("Missing required parameter: name"),
    };

    let input = match arguments.get("input") {
        Some(i) => i.clone(),
        None => return CallToolResult::error("Missing required parameter: input"),
    };

    let Some(deno) = deno else {
        return CallToolResult::error("Deno executor not configured");
    };

    // Find tool
    let (rkey, tool) = match find_tool_by_name(state, name).await {
        Ok(Some(t)) => t,
        Ok(None) => return CallToolResult::error(format!("Tool '{}' not found", name)),
        Err(e) => return CallToolResult::error(e),
    };

    // Check approval status
    let approval = get_approval(state, &rkey).await;
    let approved = is_approved(&approval, tool.version);

    // Build permissions based on approval
    let permissions = if approved {
        let approval = approval.unwrap();
        let secret_values = if let Some(secrets) = secrets {
            let mgr = secrets.read().await;
            mgr.get_subset(&approval.allowed_secrets)
        } else {
            HashMap::new()
        };

        // Build workspace permission if granted
        let workspace = match (
            approval.workspace_path,
            approval.allow_workspace_read,
            approval.allow_workspace_write,
        ) {
            (Some(path), read, write) if read.unwrap_or(false) || write.unwrap_or(false) => {
                Some(WorkspacePermission {
                    path: std::path::PathBuf::from(path),
                    read: read.unwrap_or(false),
                    write: write.unwrap_or(false),
                })
            }
            _ => None,
        };

        DenoPermissions {
            network: approval.allow_network.unwrap_or(false),
            secrets: secret_values,
            workspace,
            allowed_commands: approval.allowed_commands,
        }
    } else {
        // Sandboxed execution - no network, no secrets, no workspace, no commands
        DenoPermissions::default()
    };

    let sandbox_mode = !approved;

    info!(
        tool = %name,
        sandboxed = sandbox_mode,
        network = permissions.network,
        secrets_count = permissions.secrets.len(),
        workspace = permissions.workspace.is_some(),
        commands_count = permissions.allowed_commands.len(),
        "Executing custom tool"
    );

    match deno.execute(&tool.code, &input, permissions).await {
        Ok(output) => CallToolResult::success(
            json!({
                "result": output.result,
                "duration_ms": output.duration_ms,
                "sandboxed": sandbox_mode,
                "stderr": if output.stderr.is_empty() { None } else { Some(output.stderr) },
            })
            .to_string(),
        ),
        Err(e) => CallToolResult::error(format!(
            "Tool execution failed{}: {}",
            if sandbox_mode {
                " (sandboxed mode)"
            } else {
                ""
            },
            e
        )),
    }
}

pub async fn delete_custom_tool(
    state: &ToolState,
    arguments: &HashMap<String, Value>,
) -> CallToolResult {
    let name = match arguments.get("name").and_then(|v| v.as_str()) {
        Some(n) => n,
        None => return CallToolResult::error("Missing required parameter: name"),
    };

    let (rkey, _) = match find_tool_by_name(state, name).await {
        Ok(Some(t)) => t,
        Ok(None) => return CallToolResult::error(format!("Tool '{}' not found", name)),
        Err(e) => return CallToolResult::error(e),
    };

    // Delete approval first
    if state
        .atproto
        .delete_record(TOOL_APPROVAL_COLLECTION, &rkey)
        .await
        .is_ok()
    {
        if let Some(cache) = &state.cache {
            cache.delete_tool_approval(&rkey);
        }
    }

    // Delete tool
    match state.atproto.delete_record(TOOL_COLLECTION, &rkey).await {
        Ok(_) => {
            // Remove from cache
            if let Some(cache) = &state.cache {
                cache.delete_tool(&rkey);
            }
            CallToolResult::success(
                json!({
                    "name": name,
                    "deleted": true
                })
                .to_string(),
            )
        }
        Err(e) => CallToolResult::error(format!("Failed to delete tool: {}", e)),
    }
}

pub async fn request_secret(
    state: &ToolState,
    arguments: &HashMap<String, Value>,
) -> CallToolResult {
    let name = match arguments.get("name").and_then(|v| v.as_str()) {
        Some(n) => n,
        None => return CallToolResult::error("Missing required parameter: name"),
    };

    // Validate name
    if !name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
        return CallToolResult::error("Secret name must be alphanumeric with underscores only");
    }

    if name.len() > 64 {
        return CallToolResult::error("Secret name too long (max 64 chars)");
    }

    let description = match arguments.get("description").and_then(|v| v.as_str()) {
        Some(d) => d,
        None => return CallToolResult::error("Missing required parameter: description"),
    };

    // Get or create secret metadata
    let mut meta = match state
        .atproto
        .get_record::<SecretMeta>(SECRET_META_COLLECTION, SECRET_META_KEY)
        .await
    {
        Ok(record) => record.value,
        Err(winter_atproto::AtprotoError::NotFound { .. }) => SecretMeta {
            secrets: Vec::new(),
            created_at: Utc::now(),
            last_updated: None,
        },
        Err(e) => return CallToolResult::error(format!("Failed to get secret metadata: {}", e)),
    };

    // Check if already exists
    if meta.secrets.iter().any(|s| s.name == name) {
        return CallToolResult::error(format!("Secret '{}' already exists", name));
    }

    // Add new secret entry
    meta.secrets.push(SecretEntry {
        name: name.to_string(),
        description: Some(description.to_string()),
    });
    meta.last_updated = Some(Utc::now());

    // Save metadata
    match state
        .atproto
        .put_record(SECRET_META_COLLECTION, SECRET_META_KEY, &meta)
        .await
    {
        Ok(_) => {
            // Notify operator
            if let Ok(identity) = state
                .atproto
                .get_record::<Identity>(IDENTITY_COLLECTION, IDENTITY_KEY)
                .await
            {
                if let Some(ref bluesky) = state.bluesky {
                    let message = format!(
                        "I need a new secret \"{}\".\n\nDescription: {}\n\nPlease add it at {}/secrets",
                        name,
                        description,
                        web_url()
                    );
                    let _ = bluesky
                        .send_dm(&identity.value.operator_did, &message)
                        .await;
                }
            }

            CallToolResult::success(
                json!({
                    "name": name,
                    "description": description,
                    "message": "Secret requested. The operator has been notified to provide the value."
                })
                .to_string(),
            )
        }
        Err(e) => CallToolResult::error(format!("Failed to save secret metadata: {}", e)),
    }
}

pub async fn list_secrets(
    state: &ToolState,
    secrets: Option<&Arc<RwLock<SecretManager>>>,
    _arguments: &HashMap<String, Value>,
) -> CallToolResult {
    // Get metadata from ATProto
    let meta = match state
        .atproto
        .get_record::<SecretMeta>(SECRET_META_COLLECTION, SECRET_META_KEY)
        .await
    {
        Ok(record) => record.value,
        Err(winter_atproto::AtprotoError::NotFound { .. }) => SecretMeta {
            secrets: Vec::new(),
            created_at: Utc::now(),
            last_updated: None,
        },
        Err(e) => return CallToolResult::error(format!("Failed to get secret metadata: {}", e)),
    };

    // Check which secrets have values
    let has_value_set: std::collections::HashSet<String> = if let Some(secrets) = secrets {
        let mgr = secrets.read().await;
        mgr.list_names().into_iter().collect()
    } else {
        std::collections::HashSet::new()
    };

    let secrets: Vec<Value> = meta
        .secrets
        .iter()
        .map(|s| {
            json!({
                "name": s.name,
                "description": s.description,
                "has_value": has_value_set.contains(&s.name),
            })
        })
        .collect();

    CallToolResult::success(
        json!({
            "count": secrets.len(),
            "secrets": secrets
        })
        .to_string(),
    )
}

/// Dispatch custom tool calls.
pub async fn dispatch(
    state: &ToolState,
    secrets: Option<&Arc<RwLock<SecretManager>>>,
    deno: Option<&DenoExecutor>,
    tool_name: &str,
    arguments: HashMap<String, Value>,
) -> Option<CallToolResult> {
    match tool_name {
        "create_custom_tool" => Some(create_custom_tool(state, &arguments).await),
        "update_custom_tool" => Some(update_custom_tool(state, &arguments).await),
        "list_custom_tools" => Some(list_custom_tools(state, &arguments).await),
        "get_custom_tool" => Some(get_custom_tool(state, &arguments).await),
        "run_custom_tool" => Some(run_custom_tool(state, secrets, deno, &arguments).await),
        "delete_custom_tool" => Some(delete_custom_tool(state, &arguments).await),
        "request_secret" => Some(request_secret(state, &arguments).await),
        "list_secrets" => Some(list_secrets(state, secrets, &arguments).await),
        _ => None,
    }
}
