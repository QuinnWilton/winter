//! Custom tools for MCP.
//!
//! This module provides tools for creating, managing, and running
//! custom JavaScript/TypeScript tools in a sandboxed Deno environment.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use chrono::Utc;
use serde_json::{Value, json};
use tokio::sync::RwLock;
use tracing::{info, warn};

use crate::deno::{DenoExecutor, DenoPermissions};
use crate::protocol::{CallToolResult, ToolDefinition};
use crate::secrets::SecretManager;
use winter_atproto::{
    ByteSlice, CustomTool, Facet, FacetFeature, IDENTITY_COLLECTION, IDENTITY_KEY, Identity,
    SECRET_META_COLLECTION, SECRET_META_KEY, SecretEntry, SecretMeta, TOOL_APPROVAL_COLLECTION,
    TOOL_COLLECTION, Tid, ToolApproval, ToolApprovalStatus,
};

use super::permissions::PermissionVec;
use super::{ToolMeta, ToolState};

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
                    "requires_network": {
                        "type": "boolean",
                        "description": "Whether this tool needs network access. Auto-detected from code (remote imports, fetch, etc.) but set this to true to override detection."
                    },
                    "required_commands": {
                        "type": "array",
                        "items": { "type": "string" },
                        "description": "Subprocess commands this tool needs to run (e.g., ['git'])"
                    },
                    "required_tools": {
                        "type": "array",
                        "items": { "type": "string" },
                        "description": "Tools this tool needs to call for chaining. Use AT URIs for custom tools (e.g., 'at://did:plc:xxx/diy.razorgirl.winter.tool/rkey') and plain names for built-in MCP tools (e.g., 'query_facts'). AT URIs enable cross-agent tool sharing."
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
                    "requires_network": {
                        "type": "boolean",
                        "description": "Whether this tool needs network access (optional, auto-detected from code)"
                    },
                    "required_commands": {
                        "type": "array",
                        "items": { "type": "string" },
                        "description": "Subprocess commands this tool needs to run (optional)"
                    },
                    "required_tools": {
                        "type": "array",
                        "items": { "type": "string" },
                        "description": "Tools this tool needs to call (optional). Use AT URIs for custom tools, plain names for built-in MCP tools."
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
                "properties": {
                    "name": {
                        "type": "string",
                        "description": "Filter by tool name (case-insensitive substring)"
                    },
                    "status": {
                        "type": "string",
                        "enum": ["pending", "approved", "outdated"],
                        "description": "Filter by approval status"
                    },
                    "limit": {
                        "type": "integer",
                        "description": "Maximum number of tools to return"
                    }
                }
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

/// Get all custom tools management tools with their permission metadata.
/// All custom tools management tools are allowed for the autonomous agent.
pub fn tools() -> Vec<ToolMeta> {
    definitions().into_iter().map(ToolMeta::allowed).collect()
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

/// Build a mapping from tool name to AT URI for allowed_tools entries.
/// This lets Deno tools call chained tools by name instead of AT URI.
///
/// If multiple AT URIs resolve to the same tool name (e.g., same-named tools
/// on different PDSs), the name is ambiguous and excluded from the map.
/// Tool code must use AT URIs directly to disambiguate.
async fn build_tool_name_map(state: &ToolState, allowed_tools: &[String]) -> HashMap<String, String> {
    use super::permissions::parse_at_uri;

    let mut name_map = HashMap::new();
    if allowed_tools.is_empty() {
        return name_map;
    }

    // Collect (AT URI, DID, rkey) tuples to resolve
    let at_uri_tools: Vec<(&str, &str, &str)> = allowed_tools
        .iter()
        .filter_map(|t| {
            parse_at_uri(t).map(|(did, _col, rkey)| (t.as_str(), did, rkey))
        })
        .collect();

    if at_uri_tools.is_empty() {
        return name_map;
    }

    // Resolve local tools (same PDS)
    if let Ok(tools) = state
        .atproto
        .list_all_records::<CustomTool>(TOOL_COLLECTION)
        .await
    {
        for item in &tools {
            let rkey = item.uri.split('/').next_back().unwrap_or("");
            for (at_uri, _did, uri_rkey) in &at_uri_tools {
                if rkey == *uri_rkey {
                    name_map.insert(item.value.name.clone(), at_uri.to_string());
                }
            }
        }
    }

    // Resolve remote tools — fetch tool record from each remote DID's PDS
    let local_did = state.atproto.did().await;
    for (at_uri, did, rkey) in &at_uri_tools {
        if local_did.as_deref() == Some(*did) {
            continue; // Already resolved above
        }
        // Fetch individual tool record from remote PDS
        if let Some(pds_url) = resolve_pds_for_did(did).await {
            let url = format!(
                "{}/xrpc/com.atproto.repo.getRecord?repo={}&collection={}&rkey={}",
                pds_url, did, TOOL_COLLECTION, rkey
            );
            if let Ok(response) = reqwest::get(&url).await {
                if response.status().is_success() {
                    if let Ok(body) = response.json::<serde_json::Value>().await {
                        if let Some(value) = body.get("value") {
                            if let Ok(tool) = serde_json::from_value::<CustomTool>(value.clone()) {
                                name_map.insert(tool.name.clone(), at_uri.to_string());
                            }
                        }
                    }
                }
            }
        }
    }

    // Remove ambiguous names (multiple AT URIs resolved to the same name)
    let mut seen_names: HashMap<String, Vec<String>> = HashMap::new();
    for (name, uri) in &name_map {
        seen_names
            .entry(name.clone())
            .or_default()
            .push(uri.clone());
    }
    let ambiguous: HashSet<String> = seen_names
        .into_iter()
        .filter(|(_, uris)| uris.len() > 1)
        .map(|(name, uris)| {
            warn!(
                name = %name,
                uris = ?uris,
                "Ambiguous tool name maps to multiple AT URIs — use AT URIs in tool code to disambiguate"
            );
            name
        })
        .collect();
    for name in &ambiguous {
        name_map.remove(name);
    }

    name_map
}

/// Get approval for a tool.
///
/// Checks both Winter's PDS (for auto-approvals and legacy) and the operator's PDS
/// (for operator-granted approvals). Operator PDS approvals take precedence.
async fn get_approval(state: &ToolState, tool_rkey: &str) -> Option<ToolApproval> {
    // First check operator's PDS if WINTER_OPERATOR_DID is set
    match std::env::var("WINTER_OPERATOR_DID") {
        Ok(operator_did) => {
            info!(
                operator_did = %operator_did,
                tool_rkey = %tool_rkey,
                "Checking operator's PDS for approval"
            );
            if let Some(approval) = get_operator_approval(&operator_did, tool_rkey).await {
                // Verify winter_did if set
                if let Some(ref winter_did) = approval.winter_did {
                    if let Some(our_did) = state.atproto.did().await {
                        if winter_did != &our_did {
                            warn!(
                                expected = %our_did,
                                found = %winter_did,
                                "Operator approval winter_did mismatch, ignoring"
                            );
                            // Fall through to local check
                        } else {
                            info!(tool_rkey = %tool_rkey, "Found valid operator approval");
                            return Some(approval);
                        }
                    } else {
                        return Some(approval);
                    }
                } else {
                    info!(tool_rkey = %tool_rkey, "Found operator approval (no winter_did binding)");
                    return Some(approval);
                }
            }
        }
        Err(_) => {
            info!(
                tool_rkey = %tool_rkey,
                "WINTER_OPERATOR_DID not set, skipping operator PDS check"
            );
        }
    }

    // Fallback: check Winter's own PDS (auto-approvals and legacy approvals)
    info!(tool_rkey = %tool_rkey, "Checking Winter's own PDS for approval (fallback)");
    state
        .atproto
        .get_record::<ToolApproval>(TOOL_APPROVAL_COLLECTION, tool_rkey)
        .await
        .ok()
        .map(|r| r.value)
}

/// Fetch tool approval from operator's PDS (public XRPC, no auth needed).
async fn get_operator_approval(operator_did: &str, tool_rkey: &str) -> Option<ToolApproval> {
    // Resolve operator's PDS endpoint
    let pds_url = match resolve_pds_for_did(operator_did).await {
        Some(url) => url,
        None => {
            warn!(
                operator_did = %operator_did,
                tool_rkey = %tool_rkey,
                "Could not resolve PDS URL for operator DID"
            );
            return None;
        }
    };

    let url = format!(
        "{}/xrpc/com.atproto.repo.getRecord?repo={}&collection={}&rkey={}",
        pds_url,
        operator_did,
        "diy.razorgirl.winter.toolApproval",
        tool_rkey
    );

    let response = match reqwest::get(&url).await {
        Ok(r) => r,
        Err(e) => {
            warn!(
                error = %e,
                operator_did = %operator_did,
                tool_rkey = %tool_rkey,
                "Failed to fetch operator approval from PDS"
            );
            return None;
        }
    };

    if !response.status().is_success() {
        let status = response.status();
        info!(
            status = %status,
            operator_did = %operator_did,
            tool_rkey = %tool_rkey,
            "No approval record found on operator's PDS"
        );
        return None;
    }

    // Parse the response — ATProto getRecord returns { uri, cid, value }
    let body: serde_json::Value = match response.json().await {
        Ok(b) => b,
        Err(e) => {
            warn!(
                error = %e,
                tool_rkey = %tool_rkey,
                "Failed to parse operator approval response"
            );
            return None;
        }
    };
    let value = match body.get("value") {
        Some(v) => v,
        None => {
            warn!(
                tool_rkey = %tool_rkey,
                "Operator approval response missing 'value' field"
            );
            return None;
        }
    };
    match serde_json::from_value::<ToolApproval>(value.clone()) {
        Ok(approval) => Some(approval),
        Err(e) => {
            warn!(
                error = %e,
                tool_rkey = %tool_rkey,
                "Failed to deserialize operator approval record"
            );
            None
        }
    }
}

/// Resolve the PDS URL for a DID via the DID document.
pub(crate) async fn resolve_pds_for_did(did: &str) -> Option<String> {
    let doc_url = if did.starts_with("did:plc:") {
        format!("https://plc.directory/{}", did)
    } else if did.starts_with("did:web:") {
        let domain = did.strip_prefix("did:web:")?;
        format!("https://{}/.well-known/did.json", domain)
    } else {
        return None;
    };

    let response = reqwest::get(&doc_url).await.ok()?;
    if !response.status().is_success() {
        return None;
    }

    let doc: serde_json::Value = response.json().await.ok()?;

    // Find the ATProto PDS service endpoint
    let services = doc.get("service")?.as_array()?;
    for service in services {
        let service_type = service.get("type")?.as_str()?;
        if service_type == "AtprotoPersonalDataServer" {
            return service
                .get("serviceEndpoint")
                .and_then(|v| v.as_str())
                .map(|s| s.trim_end_matches('/').to_string());
        }
    }

    None
}

/// Check if a tool is auto-approvable, including transitive checks for chained tools.
///
/// A tool is auto-approvable if:
/// 1. Its own PermissionVec is safe (no network, no secrets, no commands, only safe MCP tools)
/// 2. Any custom tool AT URIs it chains to are local (same PDS) and themselves auto-approvable
///
/// Uses a visited set to handle cycles (a cycle makes the tool not auto-approvable).
async fn is_auto_approvable(state: &ToolState, tool: &CustomTool) -> bool {
    let mut visited = HashSet::new();
    is_auto_approvable_inner(state, tool, &mut visited).await
}

async fn is_auto_approvable_inner(
    state: &ToolState,
    tool: &CustomTool,
    visited: &mut HashSet<String>,
) -> bool {
    use super::permissions::{is_at_uri, is_safe_mcp_tool, parse_at_uri};

    let perms = PermissionVec::from_tool(tool);

    // Check non-tool dimensions
    if perms.network || !perms.secrets.is_empty() || !perms.commands.is_empty() {
        return false;
    }

    // Check each tool reference
    let our_did = state.atproto.did().await;
    for tool_ref in &perms.mcp_tools {
        if is_at_uri(tool_ref) {
            // It's a custom tool reference — check if it's local and safe
            let Some((did, _collection, rkey)) = parse_at_uri(tool_ref) else {
                return false;
            };

            // Must be on our own PDS
            if our_did.as_deref() != Some(did) {
                return false;
            }

            // Cycle detection
            if !visited.insert(rkey.to_string()) {
                return false;
            }

            // Look up the referenced tool
            let referenced_tool = if let Some(cache) = &state.cache {
                cache.get_tool(rkey).map(|r| r.value)
            } else {
                None
            };

            let Some(referenced_tool) = referenced_tool else {
                return false; // Can't find it — not safe to auto-approve
            };

            // Recursively check the referenced tool
            if !Box::pin(is_auto_approvable_inner(state, &referenced_tool, visited)).await {
                return false;
            }
        } else if !is_safe_mcp_tool(tool_ref) {
            return false;
        }
    }

    true
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

    let commands_info = if required_commands.is_empty() {
        String::new()
    } else {
        format!("\nRequired commands: {}", required_commands.join(", "))
    };

    // Build the URL and create explicit facet for it
    let review_url = format!("{}/tools/{}", web_url(), tool_rkey);
    info!(url = %review_url, "Tool approval notification URL");
    let message_prefix = format!(
        "I created/updated a tool \"{}\" that needs your approval.\n\nRequired secrets: {}{}\n\nPlease review at ",
        tool_name, secrets_list, commands_info
    );
    let message = format!("{}{}", message_prefix, review_url);

    // Create explicit facet for the URL to avoid auto-detection issues with ports
    let url_start = message_prefix.len();
    let url_end = url_start + review_url.len();
    let facets = vec![Facet {
        index: ByteSlice {
            byte_start: url_start as u64,
            byte_end: url_end as u64,
        },
        features: vec![FacetFeature::Link {
            uri: review_url.clone(),
        }],
    }];

    info!(
        facet_uri = %review_url,
        byte_start = %url_start,
        byte_end = %url_end,
        "Sending tool approval DM with facet"
    );
    if let Err(e) = bluesky.send_dm(&operator_did, &message, Some(facets)).await {
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

    let requires_network = arguments
        .get("requires_network")
        .and_then(|v| v.as_bool());

    let required_commands: Vec<String> = arguments
        .get("required_commands")
        .and_then(|v| v.as_array())
        .map(|a| {
            a.iter()
                .filter_map(|v| v.as_str().map(String::from))
                .collect()
        })
        .unwrap_or_default();

    let required_tools: Vec<String> = arguments
        .get("required_tools")
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
        requires_network,
        required_commands: required_commands.clone(),
        required_tools: required_tools.clone(),
        version: 1,
        created_at: now,
        last_updated: Some(now),
    };

    let rkey = Tid::now().to_string();

    // Check if tool is safe (auto-approval eligible), including transitive chaining checks
    let is_safe = is_auto_approvable(state, &tool).await;

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

            if is_safe {
                // Auto-approve safe tools — no operator intervention needed
                info!(tool = %name, "Auto-approving safe tool");
                let auto_approval = ToolApproval {
                    tool_rkey: rkey.clone(),
                    tool_version: 1,
                    status: ToolApprovalStatus::Approved,
                    allow_network: Some(false),
                    allowed_secrets: Vec::new(),
                    workspace_path: None,
                    allow_workspace_read: None,
                    allow_workspace_write: None,
                    allowed_commands: Vec::new(),
                    allowed_tools: tool.required_tools.clone(),
                    winter_did: None,
                    operator_did: None,
                    approved_by: Some("auto".to_string()),
                    reason: Some("Auto-approved: safe tool".to_string()),
                    created_at: Utc::now(),
                };
                if let Err(e) = state
                    .atproto
                    .put_record(TOOL_APPROVAL_COLLECTION, &rkey, &auto_approval)
                    .await
                {
                    warn!(error = %e, "Failed to auto-approve safe tool");
                } else if let Some(cache) = &state.cache {
                    cache.upsert_tool_approval(rkey.clone(), auto_approval, String::new());
                }

                CallToolResult::success(
                    json!({
                        "rkey": rkey,
                        "uri": response.uri,
                        "cid": response.cid,
                        "name": name,
                        "version": 1,
                        "status": "approved",
                        "auto_approved": true,
                        "message": "Tool created and auto-approved. Ready to run."
                    })
                    .to_string(),
                )
            } else {
                // Unsafe tool — notify operator for approval
                notify_operator(
                    state,
                    name,
                    &rkey,
                    &required_secrets,
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
                        "auto_approved": false,
                        "message": "Tool created. The operator has been notified for approval. You can test it sandboxed with run_custom_tool."
                    })
                    .to_string(),
                )
            }
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

    if let Some(requires_network) = arguments
        .get("requires_network")
        .and_then(|v| v.as_bool())
    {
        tool.requires_network = Some(requires_network);
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

    if let Some(tools_list) = arguments
        .get("required_tools")
        .and_then(|v| v.as_array())
    {
        tool.required_tools = tools_list
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

    // Check if updated tool is safe (including transitive chaining checks)
    let is_safe = is_auto_approvable(state, &tool).await;

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

            if is_safe {
                // Auto-approve safe tools
                info!(tool = %name, "Auto-approving updated safe tool");
                let auto_approval = ToolApproval {
                    tool_rkey: rkey.clone(),
                    tool_version: tool.version,
                    status: ToolApprovalStatus::Approved,
                    allow_network: Some(false),
                    allowed_secrets: Vec::new(),
                    workspace_path: None,
                    allow_workspace_read: None,
                    allow_workspace_write: None,
                    allowed_commands: Vec::new(),
                    allowed_tools: tool.required_tools.clone(),
                    winter_did: None,
                    operator_did: None,
                    approved_by: Some("auto".to_string()),
                    reason: Some("Auto-approved: safe tool (no network, no secrets)".to_string()),
                    created_at: Utc::now(),
                };
                if let Err(e) = state
                    .atproto
                    .put_record(TOOL_APPROVAL_COLLECTION, &rkey, &auto_approval)
                    .await
                {
                    warn!(error = %e, "Failed to auto-approve safe tool");
                } else if let Some(cache) = &state.cache {
                    cache.upsert_tool_approval(rkey.clone(), auto_approval, String::new());
                }

                CallToolResult::success(
                    json!({
                        "rkey": rkey,
                        "uri": response.uri,
                        "cid": response.cid,
                        "name": name,
                        "version": tool.version,
                        "status": "approved",
                        "auto_approved": true,
                        "message": "Tool updated and auto-approved (safe tool). Ready to run."
                    })
                    .to_string(),
                )
            } else {
                // Notify operator for unsafe tools
                notify_operator(
                    state,
                    name,
                    &rkey,
                    &tool.required_secrets,
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
                        "auto_approved": false,
                        "message": "Tool updated. Previous approval revoked. The operator has been notified."
                    })
                    .to_string(),
                )
            }
        }
        Err(e) => CallToolResult::error(format!("Failed to update tool: {}", e)),
    }
}

pub async fn list_custom_tools(
    state: &ToolState,
    arguments: &HashMap<String, Value>,
) -> CallToolResult {
    let name_filter = arguments.get("name").and_then(|v| v.as_str());
    let status_filter = arguments.get("status").and_then(|v| v.as_str());
    let limit = arguments.get("limit").and_then(|v| v.as_u64());

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
        // Filter by name (case-insensitive substring)
        if let Some(name) = name_filter
            && !item
                .value
                .name
                .to_lowercase()
                .contains(&name.to_lowercase())
        {
            continue;
        }

        let rkey = item.uri.split('/').next_back().unwrap_or("");
        let approval = get_approval(state, rkey).await;
        let approved = is_approved(&approval, item.value.version);

        let status = if approved {
            "approved"
        } else if approval.is_some() {
            "outdated" // Approval exists but version doesn't match
        } else {
            "pending"
        };

        // Filter by status
        if let Some(filter) = status_filter
            && status != filter
        {
            continue;
        }

        // Apply limit
        if let Some(lim) = limit
            && formatted.len() >= lim as usize
        {
            break;
        }

        formatted.push(json!({
            "rkey": rkey,
            "name": item.value.name,
            "description": item.value.description,
            "version": item.value.version,
            "status": status,
            "required_secrets": item.value.required_secrets,
            "requires_workspace": item.value.requires_workspace,
            "required_commands": item.value.required_commands,
            "required_tools": item.value.required_tools,
            "allow_network": approval.as_ref().and_then(|a| a.allow_network),
            "allowed_secrets": approval.as_ref().map(|a| &a.allowed_secrets),
            "workspace_path": approval.as_ref().and_then(|a| a.workspace_path.as_ref()),
            "allow_workspace_read": approval.as_ref().and_then(|a| a.allow_workspace_read),
            "allow_workspace_write": approval.as_ref().and_then(|a| a.allow_workspace_write),
            "allowed_commands": approval.as_ref().map(|a| &a.allowed_commands),
            "allowed_tools": approval.as_ref().map(|a| &a.allowed_tools),
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
            "required_tools": tool.required_tools,
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
                "allowed_tools": a.allowed_tools,
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

    // Track the chaining token for cleanup after execution
    let mut chaining_token: Option<String> = None;

    // Build permissions based on approval
    let permissions = if approved {
        let approval = approval.unwrap();
        let secret_values = if let Some(secrets) = secrets {
            let mut mgr = secrets.write().await;
            if let Err(e) = mgr.reload().await {
                tracing::warn!(error = %e, "failed to reload secrets");
            }
            mgr.get_subset(&approval.allowed_secrets)
        } else {
            HashMap::new()
        };

        // Build tool chaining permissions
        let allowed_tools = approval.allowed_tools.clone();

        // Build name→AT URI map so Deno tools can call by name
        let tool_name_map = build_tool_name_map(state, &allowed_tools).await;

        let (tool_token, mcp_url) = if !allowed_tools.is_empty() {
            // Use the internal MCP URL set by the HTTP server (same container, localhost)
            let url = match &state.internal_mcp_url {
                Some(url) => url.clone(),
                None => {
                    tracing::warn!("Tool chaining requested but internal_mcp_url not set (HTTP server not running?)");
                    "http://127.0.0.1:3847".to_string()
                }
            };

            // Register a session in the shared store to get a token
            let token = if let Some(ref sessions) = state.tool_sessions {
                let caller_perms = PermissionVec::from_approval(&approval);
                let token = sessions
                    .register(
                        allowed_tools.iter().cloned().collect(),
                        caller_perms,
                        0, // depth 0 for initial execution
                    )
                    .await;
                Some(token)
            } else {
                tracing::warn!(
                    "Tool chaining requested but no session store available (HTTP server not running?)"
                );
                None
            };

            // Store token for cleanup after execution
            chaining_token = token.clone();

            (token, Some(url))
        } else {
            (None, None)
        };

        DenoPermissions {
            network: approval.allow_network.unwrap_or(false),
            secrets: secret_values,
            allowed_commands: approval.allowed_commands.clone(),
            allowed_tools,
            tool_name_map,
            tool_token,
            mcp_url,
        }
    } else {
        // Sandboxed execution - no network, no secrets, no commands
        DenoPermissions::default()
    };

    let sandbox_mode = !approved;

    info!(
        tool = %name,
        sandboxed = sandbox_mode,
        network = permissions.network,
        secrets_count = permissions.secrets.len(),
        commands_count = permissions.allowed_commands.len(),
        "Executing custom tool"
    );

    let result = match deno.execute(&tool.code, &input, permissions).await {
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
    };

    // Clean up the chaining session token (if one was registered)
    if let Some(ref token) = chaining_token {
        if let Some(ref sessions) = state.tool_sessions {
            sessions.remove(token).await;
        }
    }

    result
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
        && let Some(cache) = &state.cache
    {
        cache.delete_tool_approval(&rkey);
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
                && let Some(ref bluesky) = state.bluesky
            {
                // Build URL and create explicit facet for it
                let secrets_url = format!("{}/secrets", web_url());
                let message_prefix = format!(
                    "I need a new secret \"{}\".\n\nDescription: {}\n\nPlease add it at ",
                    name, description
                );
                let message = format!("{}{}", message_prefix, secrets_url);

                // Create explicit facet for the URL
                let url_start = message_prefix.len();
                let url_end = url_start + secrets_url.len();
                let facets = vec![Facet {
                    index: ByteSlice {
                        byte_start: url_start as u64,
                        byte_end: url_end as u64,
                    },
                    features: vec![FacetFeature::Link {
                        uri: secrets_url.clone(),
                    }],
                }];

                let _ = bluesky
                    .send_dm(&identity.value.operator_did, &message, Some(facets))
                    .await;
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
