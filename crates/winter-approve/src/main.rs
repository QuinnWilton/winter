//! CLI tool for operators to approve Winter custom tools.
//!
//! This tool runs on the operator's machine, authenticates to the operator's PDS,
//! and creates approval records there. Winter reads these approvals via public XRPC.

use std::collections::HashMap;

use chrono::Utc;
use clap::{Parser, Subcommand};
use serde::{Deserialize, Serialize};
use winter_atproto::{CustomTool, ToolApproval, ToolApprovalStatus};

const TOOL_COLLECTION: &str = "diy.razorgirl.winter.tool";
const TOOL_APPROVAL_COLLECTION: &str = "diy.razorgirl.winter.toolApproval";

/// CLI for approving Winter custom tools.
///
/// Runs on the operator's machine. Creates approval records in the
/// operator's PDS, which Winter reads via public XRPC.
#[derive(Parser)]
#[command(name = "winter-approve", about = "Approve Winter custom tools")]
struct Cli {
    /// Operator's PDS URL (e.g., https://bsky.social)
    #[arg(long, env = "ATPROTO_PDS_URL")]
    pds: String,

    /// Operator's handle (e.g., operator.bsky.social)
    #[arg(long, env = "ATPROTO_HANDLE")]
    handle: String,

    /// Winter instance's DID (to read tool requests from)
    #[arg(long, env = "WINTER_DID")]
    winter_did: String,

    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// List tools needing approval (use --all to include approved/safe)
    List {
        /// Show all tools, not just pending ones
        #[arg(long)]
        all: bool,
    },
    /// Show tool details and requested permissions
    Show {
        /// Tool rkey
        rkey: String,
    },
    /// Approve a tool (interactive by default, or use flags for scripting).
    /// If no rkey is given, cycles through all pending tools interactively.
    Approve {
        /// Tool rkey (omit to cycle through all pending)
        rkey: Option<String>,
        /// Allow network access
        #[arg(long)]
        network: bool,
        /// Allow workspace read
        #[arg(long)]
        workspace_read: bool,
        /// Allow workspace write
        #[arg(long)]
        workspace_write: bool,
        /// Workspace path
        #[arg(long)]
        workspace_path: Option<String>,
        /// Secrets to allow (comma-separated)
        #[arg(long, value_delimiter = ',')]
        secrets: Vec<String>,
        /// Commands to allow (comma-separated)
        #[arg(long, value_delimiter = ',')]
        commands: Vec<String>,
        /// MCP/custom tools to allow calling (comma-separated)
        #[arg(long, value_delimiter = ',')]
        tools: Vec<String>,
        /// Reason for approval
        #[arg(long)]
        reason: Option<String>,
        /// Skip interactive prompts (use flags only)
        #[arg(long, short = 'y')]
        yes: bool,
    },
    /// Deny a tool request
    Deny {
        /// Tool rkey
        rkey: String,
        /// Reason for denial
        #[arg(long)]
        reason: Option<String>,
    },
    /// Revoke an existing approval
    Revoke {
        /// Tool rkey
        rkey: String,
    },
    /// Migrate existing approvals from Winter's PDS to operator's PDS
    Migrate,
}

/// ATProto session response.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Session {
    did: String,
    access_jwt: String,
}

/// ATProto listRecords response.
#[derive(Debug, Deserialize)]
struct ListRecordsResponse {
    records: Vec<RecordItem>,
    cursor: Option<String>,
}

#[derive(Debug, Deserialize)]
struct RecordItem {
    uri: String,
    value: serde_json::Value,
}

/// ATProto putRecord request.
#[derive(Debug, Serialize)]
struct PutRecordRequest {
    repo: String,
    collection: String,
    rkey: String,
    record: serde_json::Value,
}

/// Get the app password from env var or interactive prompt.
fn get_password() -> String {
    if let Ok(password) = std::env::var("ATPROTO_APP_PASSWORD") {
        return password;
    }

    eprint!("App password: ");
    rpassword::read_password().unwrap_or_else(|e| {
        eprintln!("Failed to read password: {}", e);
        std::process::exit(1);
    })
}

/// Authenticate to the operator's PDS, exiting on failure.
async fn authenticate(pds: &str, handle: &str) -> OperatorClient {
    let password = get_password();
    match OperatorClient::login(pds, handle, &password).await {
        Ok(c) => c,
        Err(e) => {
            eprintln!("Authentication failed: {}", e);
            std::process::exit(1);
        }
    }
}

/// Authenticated ATProto client for the operator's PDS.
struct OperatorClient {
    pds_url: String,
    did: String,
    access_jwt: String,
    http: reqwest::Client,
}

impl OperatorClient {
    async fn login(pds_url: &str, handle: &str, password: &str) -> Result<Self, String> {
        let http = reqwest::Client::new();
        let url = format!("{}/xrpc/com.atproto.server.createSession", pds_url);

        let response = http
            .post(&url)
            .json(&serde_json::json!({
                "identifier": handle,
                "password": password,
            }))
            .send()
            .await
            .map_err(|e| format!("Failed to connect to PDS: {}", e))?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            return Err(format!("Login failed ({}): {}", status, body));
        }

        let session: Session = response
            .json()
            .await
            .map_err(|e| format!("Failed to parse session: {}", e))?;

        Ok(Self {
            pds_url: pds_url.to_string(),
            did: session.did,
            access_jwt: session.access_jwt,
            http,
        })
    }

    async fn put_record(
        &self,
        collection: &str,
        rkey: &str,
        record: &serde_json::Value,
    ) -> Result<(), String> {
        let url = format!("{}/xrpc/com.atproto.repo.putRecord", self.pds_url);

        // ATProto requires $type in every record
        let mut record_with_type = record.clone();
        if let serde_json::Value::Object(ref mut map) = record_with_type {
            map.insert(
                "$type".to_string(),
                serde_json::Value::String(collection.to_string()),
            );
        }

        let request = PutRecordRequest {
            repo: self.did.clone(),
            collection: collection.to_string(),
            rkey: rkey.to_string(),
            record: record_with_type,
        };

        let response = self
            .http
            .post(&url)
            .bearer_auth(&self.access_jwt)
            .json(&request)
            .send()
            .await
            .map_err(|e| format!("Failed to put record: {}", e))?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            return Err(format!("Put record failed ({}): {}", status, body));
        }

        Ok(())
    }
}

/// Read tools from Winter's PDS (public, no auth needed).
async fn list_tools_from_winter(winter_did: &str) -> Result<Vec<(String, CustomTool)>, String> {
    let pds_url = resolve_pds_for_did(winter_did)
        .await
        .ok_or_else(|| format!("Could not resolve PDS for {}", winter_did))?;

    let http = reqwest::Client::new();
    let mut all_tools = Vec::new();
    let mut cursor: Option<String> = None;

    loop {
        let mut url = format!(
            "{}/xrpc/com.atproto.repo.listRecords?repo={}&collection={}&limit=100",
            pds_url, winter_did, TOOL_COLLECTION
        );
        if let Some(ref c) = cursor {
            url.push_str(&format!("&cursor={}", c));
        }

        let response = http
            .get(&url)
            .send()
            .await
            .map_err(|e| format!("Failed to list tools: {}", e))?;

        if !response.status().is_success() {
            return Err(format!("List records failed: {}", response.status()));
        }

        let list: ListRecordsResponse = response
            .json()
            .await
            .map_err(|e| format!("Failed to parse: {}", e))?;

        for item in &list.records {
            let rkey = item
                .uri
                .split('/')
                .next_back()
                .unwrap_or("")
                .to_string();
            if let Ok(tool) = serde_json::from_value::<CustomTool>(item.value.clone()) {
                all_tools.push((rkey, tool));
            }
        }

        cursor = list.cursor;
        if cursor.is_none() {
            break;
        }
    }

    Ok(all_tools)
}

/// Resolve a handle to a DID via public XRPC.
async fn resolve_handle(pds_url: &str, handle: &str) -> Option<String> {
    let url = format!(
        "{}/xrpc/com.atproto.identity.resolveHandle?handle={}",
        pds_url, handle
    );
    let response = reqwest::get(&url).await.ok()?;
    if !response.status().is_success() {
        return None;
    }
    let body: serde_json::Value = response.json().await.ok()?;
    body.get("did")?.as_str().map(String::from)
}

/// List approvals from a given DID's PDS.
async fn list_approvals_from_did(
    did: &str,
) -> Result<HashMap<String, ToolApproval>, String> {
    let pds_url = resolve_pds_for_did(did)
        .await
        .ok_or_else(|| format!("Could not resolve PDS for {}", did))?;

    let http = reqwest::Client::new();
    let mut approvals = HashMap::new();
    let mut cursor: Option<String> = None;

    loop {
        let mut url = format!(
            "{}/xrpc/com.atproto.repo.listRecords?repo={}&collection={}&limit=100",
            pds_url, did, TOOL_APPROVAL_COLLECTION
        );
        if let Some(ref c) = cursor {
            url.push_str(&format!("&cursor={}", c));
        }

        let response = http
            .get(&url)
            .send()
            .await
            .map_err(|e| format!("Failed to list approvals: {}", e))?;

        if !response.status().is_success() {
            break; // No approvals collection is fine
        }

        let list: ListRecordsResponse = response
            .json()
            .await
            .map_err(|e| format!("Failed to parse: {}", e))?;

        for item in &list.records {
            let rkey = item
                .uri
                .split('/')
                .next_back()
                .unwrap_or("")
                .to_string();
            if let Ok(approval) = serde_json::from_value::<ToolApproval>(item.value.clone()) {
                approvals.insert(rkey, approval);
            }
        }

        cursor = list.cursor;
        if cursor.is_none() {
            break;
        }
    }

    Ok(approvals)
}

/// Get all approvals, merging operator's PDS (primary) with Winter's PDS (legacy fallback).
async fn get_all_approvals(
    pds_url: &str,
    handle: &str,
    winter_did: &str,
) -> HashMap<String, ToolApproval> {
    // Start with Winter's PDS approvals (legacy/auto-approvals)
    let mut approvals = list_approvals_from_did(winter_did)
        .await
        .unwrap_or_default();

    // Resolve operator's DID and merge their approvals (take precedence)
    if let Some(operator_did) = resolve_handle(pds_url, handle).await {
        if operator_did != winter_did {
            if let Ok(operator_approvals) = list_approvals_from_did(&operator_did).await {
                approvals.extend(operator_approvals);
            }
        }
    }

    approvals
}

/// Resolve PDS URL from a DID.
async fn resolve_pds_for_did(did: &str) -> Option<String> {
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

/// Check if a tool is safe (auto-approvable).
fn is_safe_tool(tool: &CustomTool) -> bool {
    tool.required_secrets.is_empty()
        && tool.required_commands.is_empty()
        && !tool.requires_workspace.unwrap_or(false)
        && tool.required_tools.iter().all(|t| {
            // Same safe MCP tools list as in permissions.rs
            matches!(
                t.as_str(),
                "query_facts"
                    | "list_rules"
                    | "list_directives"
                    | "list_jobs"
                    | "list_notes"
                    | "get_note"
                    | "list_facts"
                    | "list_fact_declarations"
                    | "get_thread_context"
                    | "search_posts"
                    | "get_identity"
                    | "query_and_enrich"
                    | "list_predicates"
                    | "list_custom_tools"
                    | "get_custom_tool"
                    | "list_secrets"
                    | "list_thoughts"
                    | "get_thought"
                    | "list_blog_posts"
                    | "get_blog_post"
                    | "check_interruption"
                    | "pds_list_records"
                    | "pds_get_record"
                    | "pds_get_records"
                    | "search_users"
            )
        })
}

/// Resolve a tool reference to a friendly display name.
/// If it's an rkey that matches a custom tool, show "name (rkey)".
/// Otherwise show it as-is (built-in MCP tool name).
/// Prompt for y/n, returns true for yes. Default is the given bool.
fn prompt_yn(prompt: &str, default: bool) -> bool {
    let suffix = if default { "[Y/n]" } else { "[y/N]" };
    eprint!("{} {} ", prompt, suffix);
    let mut input = String::new();
    std::io::stdin().read_line(&mut input).unwrap_or(0);
    let trimmed = input.trim();
    if trimmed.is_empty() {
        return default;
    }
    trimmed.eq_ignore_ascii_case("y") || trimmed.eq_ignore_ascii_case("yes")
}

/// Prompt user to select items from a list. Returns selected items.
fn prompt_select(prompt: &str, items: &[String], all_tools: &[(String, CustomTool)]) -> Vec<String> {
    if items.is_empty() {
        return Vec::new();
    }
    println!("{}:", prompt);
    for (i, item) in items.iter().enumerate() {
        let display = resolve_tool_name(item, all_tools);
        println!("  [{}] {}", i + 1, display);
    }
    eprint!("Select (comma-separated numbers, 'all', or 'none'): ");
    let mut input = String::new();
    std::io::stdin().read_line(&mut input).unwrap_or(0);
    let trimmed = input.trim();
    if trimmed.eq_ignore_ascii_case("all") {
        return items.to_vec();
    }
    if trimmed.eq_ignore_ascii_case("none") || trimmed.is_empty() {
        return Vec::new();
    }
    trimmed
        .split(',')
        .filter_map(|s| {
            let n: usize = s.trim().parse().ok()?;
            items.get(n.wrapping_sub(1)).cloned()
        })
        .collect()
}

/// Resolve a tool reference to a friendly display name.
/// If it's an rkey that matches a custom tool, show "name (rkey)".
/// Otherwise show it as-is (built-in MCP tool name).
fn resolve_tool_name(tool_ref: &str, tools: &[(String, CustomTool)]) -> String {
    // Try direct rkey match first
    if let Some((_, tool)) = tools.iter().find(|(rkey, _)| rkey == tool_ref) {
        return format!("{} ({})", tool.name, tool_ref);
    }

    // Try extracting rkey from AT URI (at://did/collection/rkey)
    if tool_ref.starts_with("at://") {
        if let Some(rkey) = tool_ref.split('/').next_back() {
            if let Some((_, tool)) = tools.iter().find(|(r, _)| r == rkey) {
                return format!("{} ({})", tool.name, tool_ref);
            }
        }
    }

    tool_ref.to_string()
}

fn display_tool(
    tool: &CustomTool,
    rkey: &str,
    approval: Option<&ToolApproval>,
    all_tools: &[(String, CustomTool)],
) {
    let status = match approval {
        Some(a) if a.status == ToolApprovalStatus::Approved && a.tool_version == tool.version => {
            "approved"
        }
        Some(a) if a.status == ToolApprovalStatus::Denied => "denied",
        Some(a) if a.status == ToolApprovalStatus::Revoked => "revoked",
        Some(_) => "outdated",
        None => "pending",
    };

    println!("  {} (v{}) [{}] - {}", tool.name, tool.version, status, rkey);
    if !tool.required_secrets.is_empty() {
        println!("    Secrets: {}", tool.required_secrets.join(", "));
    }
    if tool.requires_workspace.unwrap_or(false) {
        println!("    Requires workspace: yes");
    }
    if !tool.required_commands.is_empty() {
        println!("    Commands: {}", tool.required_commands.join(", "));
    }
    if !tool.required_tools.is_empty() {
        let names: Vec<String> = tool
            .required_tools
            .iter()
            .map(|t| resolve_tool_name(t, all_tools))
            .collect();
        println!("    Tool chaining: {}", names.join(", "));
    }
}

/// Interactive approval for a single tool. Returns true if approved, false if skipped.
async fn approve_tool_interactive(
    client: &OperatorClient,
    winter_did: &str,
    rkey: &str,
    tool: &CustomTool,
    all_tools: &[(String, CustomTool)],
) -> bool {
    println!();
    println!("Tool: {} (v{})", tool.name, tool.version);
    println!("Description: {}", tool.description);
    println!("Rkey: {}", rkey);
    println!();

    // Network
    let net = if tool.code.contains("fetch(")
        || tool.code.contains("Deno.connect")
        || tool.description.to_lowercase().contains("network")
        || tool.description.to_lowercase().contains("fetch")
        || tool.description.to_lowercase().contains("http")
    {
        prompt_yn("Allow network access?", true)
    } else {
        prompt_yn("Allow network access?", false)
    };

    // Secrets
    let secs = prompt_select("Select secrets to grant", &tool.required_secrets, all_tools);

    // Commands
    let cmds = prompt_select("Select commands to allow", &tool.required_commands, all_tools);

    // Tool chaining
    let tls = prompt_select(
        "Select tools this tool may call",
        &tool.required_tools,
        all_tools,
    );

    // Workspace
    let requires_ws = tool.requires_workspace.unwrap_or(false);
    let (ws_read, ws_write, ws_path) = if requires_ws {
        let r = prompt_yn("Allow workspace read?", true);
        let w = prompt_yn("Allow workspace write?", false);
        eprint!("Workspace path (empty for default): ");
        let mut path_input = String::new();
        std::io::stdin().read_line(&mut path_input).unwrap_or(0);
        let p = path_input.trim();
        let p = if p.is_empty() { None } else { Some(p.to_string()) };
        (r, w, p)
    } else {
        (false, false, None)
    };

    // Summary
    println!();
    println!("Summary:");
    println!("  Network: {}", net);
    if !secs.is_empty() {
        println!("  Secrets: {}", secs.join(", "));
    }
    if !cmds.is_empty() {
        println!("  Commands: {}", cmds.join(", "));
    }
    if !tls.is_empty() {
        let names: Vec<String> = tls
            .iter()
            .map(|t| resolve_tool_name(t, all_tools))
            .collect();
        println!("  Allowed tools: {}", names.join(", "));
    }
    if requires_ws {
        println!("  Workspace read: {}, write: {}", ws_read, ws_write);
        if let Some(ref p) = ws_path {
            println!("  Workspace path: {}", p);
        }
    }
    println!();

    if !prompt_yn("Approve with these permissions?", false) {
        println!("Skipped.");
        return false;
    }

    write_approval(client, winter_did, rkey, tool, all_tools, net, secs, cmds, tls, ws_read, ws_write, ws_path, None).await
}

/// Approve a tool using explicit flags (non-interactive).
#[allow(clippy::too_many_arguments)]
async fn approve_tool_with_flags(
    client: &OperatorClient,
    winter_did: &str,
    rkey: &str,
    tool: &CustomTool,
    all_tools: &[(String, CustomTool)],
    network: bool,
    secrets: Vec<String>,
    commands: Vec<String>,
    tools: Vec<String>,
    workspace_read: bool,
    workspace_write: bool,
    workspace_path: Option<String>,
    reason: Option<String>,
) {
    write_approval(client, winter_did, rkey, tool, all_tools, network, secrets, commands, tools, workspace_read, workspace_write, workspace_path, reason).await;
}

/// Write an approval record to the operator's PDS.
#[allow(clippy::too_many_arguments)]
async fn write_approval(
    client: &OperatorClient,
    winter_did: &str,
    rkey: &str,
    tool: &CustomTool,
    all_tools: &[(String, CustomTool)],
    network: bool,
    secrets: Vec<String>,
    commands: Vec<String>,
    tools: Vec<String>,
    workspace_read: bool,
    workspace_write: bool,
    workspace_path: Option<String>,
    reason: Option<String>,
) -> bool {
    let approval = ToolApproval {
        tool_rkey: rkey.to_string(),
        tool_version: tool.version,
        status: ToolApprovalStatus::Approved,
        allow_network: Some(network),
        allowed_secrets: secrets,
        workspace_path,
        allow_workspace_read: Some(workspace_read),
        allow_workspace_write: Some(workspace_write),
        allowed_commands: commands,
        allowed_tools: tools,
        winter_did: Some(winter_did.to_string()),
        operator_did: Some(client.did.clone()),
        approved_by: Some(client.did.clone()),
        reason,
        created_at: Utc::now(),
    };

    let record_value = serde_json::to_value(&approval).unwrap();

    match client
        .put_record(TOOL_APPROVAL_COLLECTION, rkey, &record_value)
        .await
    {
        Ok(()) => {
            println!("Approved '{}' (v{})", tool.name, tool.version);
            if !approval.allowed_tools.is_empty() {
                let names: Vec<String> = approval
                    .allowed_tools
                    .iter()
                    .map(|t| resolve_tool_name(t, all_tools))
                    .collect();
                println!("  Allowed tools: {}", names.join(", "));
            }
            println!("Approval written to your PDS.");
            true
        }
        Err(e) => {
            eprintln!("Failed to write approval: {}", e);
            false
        }
    }
}

#[tokio::main]
async fn main() {
    let cli = Cli::parse();

    match cli.command {
        Commands::List { all } => {
            println!("Fetching tools from Winter's PDS ({})...", cli.winter_did);
            let tools = match list_tools_from_winter(&cli.winter_did).await {
                Ok(t) => t,
                Err(e) => {
                    eprintln!("Error: {}", e);
                    std::process::exit(1);
                }
            };

            let approvals = get_all_approvals(&cli.pds, &cli.handle, &cli.winter_did).await;

            if tools.is_empty() {
                println!("No tools found.");
                return;
            }

            // Categorize tools
            let (safe, unsafe_tools): (Vec<_>, Vec<_>) =
                tools.iter().partition(|(_, t)| is_safe_tool(t));

            // Split unsafe tools into pending vs already handled
            let mut pending = Vec::new();
            let mut handled = Vec::new();
            for (rkey, tool) in &unsafe_tools {
                let approval = approvals.get(rkey.as_str());
                let is_current = matches!(
                    approval,
                    Some(a) if a.status == ToolApprovalStatus::Approved
                        && a.tool_version == tool.version
                );
                if is_current {
                    handled.push((rkey, tool));
                } else {
                    pending.push((rkey, tool));
                }
            }

            if !pending.is_empty() {
                println!("\nPending approval:");
                for (rkey, tool) in &pending {
                    display_tool(tool, rkey, approvals.get(rkey.as_str()), &tools);
                }
            } else {
                println!("\nNo tools pending approval.");
            }

            if all {
                if !handled.is_empty() {
                    println!("\nApproved:");
                    for (rkey, tool) in &handled {
                        display_tool(tool, rkey, approvals.get(rkey.as_str()), &tools);
                    }
                }

                if !safe.is_empty() {
                    println!("\nSafe (auto-approved):");
                    for (rkey, tool) in &safe {
                        display_tool(tool, rkey, approvals.get(rkey.as_str()), &tools);
                    }
                }
            }

            println!(
                "\nTotal: {} tools ({} pending, {} approved, {} safe)",
                tools.len(),
                pending.len(),
                handled.len(),
                safe.len()
            );
            if !all && (!handled.is_empty() || !safe.is_empty()) {
                println!("Use --all to show approved and safe tools.");
            }
        }

        Commands::Show { rkey } => {
            let tools = match list_tools_from_winter(&cli.winter_did).await {
                Ok(t) => t,
                Err(e) => {
                    eprintln!("Error: {}", e);
                    std::process::exit(1);
                }
            };

            let tool = tools.iter().find(|(r, _)| r == &rkey);
            match tool {
                Some((_, tool)) => {
                    println!("Tool: {} (v{})", tool.name, tool.version);
                    println!("Description: {}", tool.description);
                    println!("Safe: {}", if is_safe_tool(tool) { "yes" } else { "no" });
                    println!();
                    println!("Requested permissions:");
                    if !tool.required_secrets.is_empty() {
                        println!("  Secrets: {}", tool.required_secrets.join(", "));
                    }
                    if tool.requires_workspace.unwrap_or(false) {
                        println!("  Workspace: read/write");
                    }
                    if !tool.required_commands.is_empty() {
                        println!("  Commands: {}", tool.required_commands.join(", "));
                    }
                    if !tool.required_tools.is_empty() {
                        let names: Vec<String> = tool
                            .required_tools
                            .iter()
                            .map(|t| resolve_tool_name(t, &tools))
                            .collect();
                        println!("  Tool chaining: {}", names.join(", "));
                    }
                    println!();
                    println!("Code:");
                    println!("---");
                    println!("{}", tool.code);
                    println!("---");
                }
                None => {
                    eprintln!("Tool '{}' not found", rkey);
                    std::process::exit(1);
                }
            }
        }

        Commands::Approve {
            rkey,
            network,
            workspace_read,
            workspace_write,
            workspace_path,
            secrets,
            commands,
            tools,
            reason,
            yes,
        } => {
            let all_tools = match list_tools_from_winter(&cli.winter_did).await {
                Ok(t) => t,
                Err(e) => {
                    eprintln!("Error: {}", e);
                    std::process::exit(1);
                }
            };

            if let Some(rkey) = rkey {
                // Single tool approval
                let tool = match all_tools.iter().find(|(r, _)| r == &rkey) {
                    Some((_, t)) => t,
                    None => {
                        eprintln!("Tool '{}' not found in Winter's PDS", rkey);
                        std::process::exit(1);
                    }
                };

                if is_safe_tool(tool) {
                    println!("Tool '{}' is safe and auto-approved. No action needed.", tool.name);
                    return;
                }

                // Determine if any permission flags were explicitly set
                let has_flags = network
                    || workspace_read
                    || workspace_write
                    || workspace_path.is_some()
                    || !secrets.is_empty()
                    || !commands.is_empty()
                    || !tools.is_empty()
                    || yes;

                let client = authenticate(&cli.pds, &cli.handle).await;
                if has_flags {
                    approve_tool_with_flags(
                        &client, &cli.winter_did, &rkey, tool, &all_tools,
                        network, secrets, commands, tools,
                        workspace_read, workspace_write, workspace_path, reason,
                    ).await;
                } else {
                    approve_tool_interactive(&client, &cli.winter_did, &rkey, tool, &all_tools).await;
                }
            } else {
                // No rkey: cycle through all pending tools
                let approvals = get_all_approvals(&cli.pds, &cli.handle, &cli.winter_did).await;

                let pending: Vec<_> = all_tools
                    .iter()
                    .filter(|(_, t)| !is_safe_tool(t))
                    .filter(|(rkey, tool)| {
                        let approval = approvals.get(rkey.as_str());
                        !matches!(
                            approval,
                            Some(a) if a.status == ToolApprovalStatus::Approved
                                && a.tool_version == tool.version
                        )
                    })
                    .collect();

                if pending.is_empty() {
                    println!("No tools pending approval.");
                    return;
                }

                println!("{} tool(s) pending approval.\n", pending.len());
                let client = authenticate(&cli.pds, &cli.handle).await;

                for (i, (rkey, tool)) in pending.iter().enumerate() {
                    println!("--- [{}/{}] ---", i + 1, pending.len());
                    approve_tool_interactive(&client, &cli.winter_did, rkey, tool, &all_tools).await;
                    println!();
                }
            }
        }

        Commands::Deny { rkey, reason } => {
            let all_tools = match list_tools_from_winter(&cli.winter_did).await {
                Ok(t) => t,
                Err(e) => {
                    eprintln!("Error: {}", e);
                    std::process::exit(1);
                }
            };

            let tool = match all_tools.iter().find(|(r, _)| r == &rkey) {
                Some((_, t)) => t,
                None => {
                    eprintln!("Tool '{}' not found", rkey);
                    std::process::exit(1);
                }
            };

            let client = authenticate(&cli.pds, &cli.handle).await;

            let approval = ToolApproval {
                tool_rkey: rkey.clone(),
                tool_version: tool.version,
                status: ToolApprovalStatus::Denied,
                allow_network: None,
                allowed_secrets: Vec::new(),
                workspace_path: None,
                allow_workspace_read: None,
                allow_workspace_write: None,
                allowed_commands: Vec::new(),
                allowed_tools: Vec::new(),
                winter_did: Some(cli.winter_did.clone()),
                operator_did: Some(client.did.clone()),
                approved_by: Some(client.did.clone()),
                reason,
                created_at: Utc::now(),
            };

            let record_value = serde_json::to_value(&approval).unwrap();

            match client
                .put_record(TOOL_APPROVAL_COLLECTION, &rkey, &record_value)
                .await
            {
                Ok(()) => println!("Denied '{}' (v{})", tool.name, tool.version),
                Err(e) => {
                    eprintln!("Failed to write denial: {}", e);
                    std::process::exit(1);
                }
            }
        }

        Commands::Revoke { rkey } => {
            let all_tools = match list_tools_from_winter(&cli.winter_did).await {
                Ok(t) => t,
                Err(e) => {
                    eprintln!("Error: {}", e);
                    std::process::exit(1);
                }
            };

            let tool = match all_tools.iter().find(|(r, _)| r == &rkey) {
                Some((_, t)) => t,
                None => {
                    eprintln!("Tool '{}' not found", rkey);
                    std::process::exit(1);
                }
            };

            let client = authenticate(&cli.pds, &cli.handle).await;

            let approval = ToolApproval {
                tool_rkey: rkey.clone(),
                tool_version: tool.version,
                status: ToolApprovalStatus::Revoked,
                allow_network: None,
                allowed_secrets: Vec::new(),
                workspace_path: None,
                allow_workspace_read: None,
                allow_workspace_write: None,
                allowed_commands: Vec::new(),
                allowed_tools: Vec::new(),
                winter_did: Some(cli.winter_did.clone()),
                operator_did: Some(client.did.clone()),
                approved_by: Some(client.did.clone()),
                reason: Some("Revoked by operator".to_string()),
                created_at: Utc::now(),
            };

            let record_value = serde_json::to_value(&approval).unwrap();

            match client
                .put_record(TOOL_APPROVAL_COLLECTION, &rkey, &record_value)
                .await
            {
                Ok(()) => println!("Revoked approval for '{}' (v{})", tool.name, tool.version),
                Err(e) => {
                    eprintln!("Failed to write revocation: {}", e);
                    std::process::exit(1);
                }
            }
        }

        Commands::Migrate => {
            println!("Fetching tools from Winter's PDS ({})...", cli.winter_did);

            let tools = match list_tools_from_winter(&cli.winter_did).await {
                Ok(t) => t,
                Err(e) => {
                    eprintln!("Error: {}", e);
                    std::process::exit(1);
                }
            };

            let old_approvals = list_approvals_from_did(&cli.winter_did)
                .await
                .unwrap_or_default();

            let (safe, unsafe_tools): (Vec<_>, Vec<_>) =
                tools.iter().partition(|(_, t)| is_safe_tool(t));

            println!(
                "Found {} tools total:",
                tools.len()
            );
            println!("  - {} safe tools (will auto-approve)", safe.len());
            println!("  - {} unsafe tools need migration", unsafe_tools.len());

            // Only migrate unsafe tools with existing approvals
            let to_migrate: Vec<_> = unsafe_tools
                .iter()
                .filter(|(rkey, _)| {
                    old_approvals
                        .get(rkey.as_str())
                        .map(|a| a.status == ToolApprovalStatus::Approved)
                        .unwrap_or(false)
                })
                .collect();

            if to_migrate.is_empty() {
                println!("\nNo unsafe tools with existing approvals to migrate.");
                return;
            }

            println!("\nUnsafe tools to migrate:");
            for (rkey, tool) in &to_migrate {
                display_tool(tool, rkey, old_approvals.get(rkey.as_str()), &tools);
            }

            println!("\nMigrate all with existing permissions? [y/N]");
            let mut input = String::new();
            std::io::stdin().read_line(&mut input).unwrap_or(0);
            if !input.trim().eq_ignore_ascii_case("y") {
                println!("Aborted.");
                return;
            }

            // Authenticate
            let client = authenticate(&cli.pds, &cli.handle).await;

            let mut migrated = 0;
            for (rkey, tool) in &to_migrate {
                let old = &old_approvals[rkey.as_str()];

                let new_approval = ToolApproval {
                    tool_rkey: rkey.to_string(),
                    tool_version: old.tool_version,
                    status: old.status.clone(),
                    allow_network: old.allow_network,
                    allowed_secrets: old.allowed_secrets.clone(),
                    workspace_path: old.workspace_path.clone(),
                    allow_workspace_read: old.allow_workspace_read,
                    allow_workspace_write: old.allow_workspace_write,
                    allowed_commands: old.allowed_commands.clone(),
                    allowed_tools: old.allowed_tools.clone(),
                    winter_did: Some(cli.winter_did.clone()),
                    operator_did: Some(client.did.clone()),
                    approved_by: Some(client.did.clone()),
                    reason: Some("Migrated from Winter's PDS".to_string()),
                    created_at: Utc::now(),
                };

                let record_value = serde_json::to_value(&new_approval).unwrap();

                match client
                    .put_record(TOOL_APPROVAL_COLLECTION, rkey, &record_value)
                    .await
                {
                    Ok(()) => {
                        println!("  Migrated: {} (v{})", tool.name, tool.version);
                        migrated += 1;
                    }
                    Err(e) => {
                        eprintln!("  Failed to migrate {}: {}", tool.name, e);
                    }
                }
            }

            println!(
                "\nMigration complete. {} approvals written to your PDS.",
                migrated
            );
        }
    }
}
