//! Permission model for custom tool chaining.
//!
//! Implements a product lattice over independent permission dimensions
//! (network, filesystem, secrets, commands, MCP tools). A tool can call
//! another tool only if it dominates the callee in every dimension.
//!
//! Custom tools are referenced by AT URI (e.g., `at://did:plc:xxx/diy.razorgirl.winter.tool/rkey`),
//! enabling cross-agent tool sharing between different PDS instances.
//! Built-in MCP tools use plain names (e.g., `query_facts`).

use std::cmp::Ordering;
use std::collections::{BTreeSet, HashMap, HashSet};

use winter_atproto::{CustomTool, ToolApproval};

/// MCP tools that are safe to call without operator approval.
/// These are all read-only operations that don't modify state.
pub const SAFE_MCP_TOOLS: &[&str] = &[
    "query_facts",
    "list_rules",
    "list_directives",
    "list_jobs",
    "list_notes",
    "get_note",
    "list_facts",
    "list_fact_declarations",
    "get_thread_context",
    "search_posts",
    "get_identity",
    "query_and_enrich",
    "list_predicates",
    "list_custom_tools",
    "get_custom_tool",
    "list_secrets",
    "list_thoughts",
    "get_thought",
    "list_blog_posts",
    "get_blog_post",
    "check_interruption",
    "pds_list_records",
    "pds_get_record",
    "pds_get_records",
    "search_users",
];

/// Detect if tool code needs network access based on common patterns.
/// Catches remote imports, fetch calls, and other network APIs that
/// would fail without `--allow-net`.
pub fn code_needs_network(code: &str) -> bool {
    // Remote ES module imports — both static and dynamic
    // Static:  import ... from "https://...", import "https://..."
    // Dynamic: await import("https://..."), import("npm:...")
    let remote_prefixes = [
        "\"https://", "'https://",
        "\"http://",  "'http://",
        "\"npm:",     "'npm:",
        "\"jsr:",     "'jsr:",
    ];

    for prefix in &remote_prefixes {
        // Static: from "https://..." or import "https://..."
        if code.contains(&format!("from {}", prefix))
            || code.contains(&format!("import {}", prefix))
        {
            return true;
        }
        // Dynamic: import("https://...") — with optional whitespace
        if code.contains(&format!("import({}", prefix))
            || code.contains(&format!("import ({}", prefix))
        {
            return true;
        }
    }

    // Explicit network APIs
    code.contains("fetch(")
        || code.contains("Deno.connect")
        || code.contains("new WebSocket")
        || code.contains("new EventSource")
}

/// A permission vector — one point in the product lattice.
/// Comparison is component-wise across all dimensions.
///
/// Workspace access is NOT a permission dimension — all tools get workspace access
/// since the agent already has full filesystem access via Claude Code.
///
/// The `mcp_tools` set contains:
/// - Plain names for built-in MCP tools (e.g., "query_facts")
/// - AT URIs for custom tools (e.g., "at://did:plc:xxx/diy.razorgirl.winter.tool/rkey")
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PermissionVec {
    pub network: bool,
    pub secrets: BTreeSet<String>,
    pub commands: BTreeSet<String>,
    pub mcp_tools: BTreeSet<String>,
}

impl PermissionVec {
    /// Bottom element: pure computation, no privileges.
    pub fn bottom() -> Self {
        Self {
            network: false,
            secrets: BTreeSet::new(),
            commands: BTreeSet::new(),
            mcp_tools: BTreeSet::new(),
        }
    }

    /// True if this vector is within the auto-approval threshold:
    /// no network, no secrets, no commands, only safe built-in MCP tools.
    ///
    /// Any reference to a custom tool AT URI makes this unsafe, because we can't
    /// verify the remote tool's safety without fetching it.
    pub fn is_safe(&self) -> bool {
        !self.network
            && self.secrets.is_empty()
            && self.commands.is_empty()
            && self.mcp_tools.iter().all(|t| {
                // AT URIs reference custom tools — always require approval
                if is_at_uri(t) {
                    return false;
                }
                is_safe_mcp_tool(t)
            })
    }

    /// True if self dominates other in every dimension.
    /// This is the core operation: A can call B iff A.dominates(B).
    pub fn dominates(&self, other: &PermissionVec) -> bool {
        (self.network || !other.network)
            && other.secrets.is_subset(&self.secrets)
            && other.commands.is_subset(&self.commands)
            && other.mcp_tools.is_subset(&self.mcp_tools)
    }

    /// Join (least upper bound) — union of capabilities.
    /// Used to compute effective permissions through a call chain.
    pub fn join(&self, other: &PermissionVec) -> PermissionVec {
        PermissionVec {
            network: self.network || other.network,
            secrets: self.secrets.union(&other.secrets).cloned().collect(),
            commands: self.commands.union(&other.commands).cloned().collect(),
            mcp_tools: self.mcp_tools.union(&other.mcp_tools).cloned().collect(),
        }
    }

    /// Construct from a CustomTool record (requested permissions).
    /// Network is detected from code patterns, but `requires_network` overrides.
    pub fn from_tool(tool: &CustomTool) -> Self {
        Self {
            network: tool.requires_network.unwrap_or_else(|| code_needs_network(&tool.code)),
            secrets: tool.required_secrets.iter().cloned().collect(),
            commands: tool.required_commands.iter().cloned().collect(),
            mcp_tools: tool.required_tools.iter().cloned().collect(),
        }
    }

    /// Construct from a ToolApproval record (granted permissions).
    pub fn from_approval(approval: &ToolApproval) -> Self {
        Self {
            network: approval.allow_network.unwrap_or(false),
            secrets: approval.allowed_secrets.iter().cloned().collect(),
            commands: approval.allowed_commands.iter().cloned().collect(),
            mcp_tools: approval.allowed_tools.iter().cloned().collect(),
        }
    }

    /// Compute the missing dimensions where self does NOT dominate other.
    /// Returns human-readable descriptions of what's missing.
    pub fn missing_dimensions(&self, other: &PermissionVec) -> Vec<String> {
        let mut missing = Vec::new();

        if !self.network && other.network {
            missing.push("network".to_string());
        }

        let missing_secrets: BTreeSet<_> = other.secrets.difference(&self.secrets).collect();
        if !missing_secrets.is_empty() {
            missing.push(format!(
                "secrets: {{{}}}",
                missing_secrets
                    .into_iter()
                    .cloned()
                    .collect::<Vec<_>>()
                    .join(", ")
            ));
        }

        let missing_commands: BTreeSet<_> = other.commands.difference(&self.commands).collect();
        if !missing_commands.is_empty() {
            missing.push(format!(
                "commands: {{{}}}",
                missing_commands
                    .into_iter()
                    .cloned()
                    .collect::<Vec<_>>()
                    .join(", ")
            ));
        }

        let missing_tools: BTreeSet<_> = other.mcp_tools.difference(&self.mcp_tools).collect();
        if !missing_tools.is_empty() {
            missing.push(format!(
                "mcp_tools: {{{}}}",
                missing_tools
                    .into_iter()
                    .cloned()
                    .collect::<Vec<_>>()
                    .join(", ")
            ));
        }

        missing
    }
}

/// PartialOrd returns None for incomparable vectors.
impl PartialOrd for PermissionVec {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        match (self.dominates(other), other.dominates(self)) {
            (true, true) => Some(Ordering::Equal),
            (true, false) => Some(Ordering::Greater),
            (false, true) => Some(Ordering::Less),
            (false, false) => None, // Incomparable
        }
    }
}

/// Check if an MCP tool name is in the safe list.
pub fn is_safe_mcp_tool(name: &str) -> bool {
    SAFE_MCP_TOOLS.contains(&name)
}

/// Check if a string is an AT URI (references a custom tool on some PDS).
pub fn is_at_uri(s: &str) -> bool {
    s.starts_with("at://")
}

/// Parse an AT URI into (did, collection, rkey).
/// Returns None if the URI is not a valid AT URI.
pub fn parse_at_uri(uri: &str) -> Option<(&str, &str, &str)> {
    let rest = uri.strip_prefix("at://")?;
    let mut parts = rest.splitn(3, '/');
    let did = parts.next()?;
    let collection = parts.next()?;
    let rkey = parts.next()?;
    if did.is_empty() || collection.is_empty() || rkey.is_empty() {
        return None;
    }
    Some((did, collection, rkey))
}

/// Privilege violation errors.
#[derive(Debug)]
pub enum PrivilegeViolation {
    /// The tool is not in the permission map.
    UnknownTool(String),
    /// Caller lacks sufficient privilege to call callee.
    InsufficientPrivilege {
        caller: String,
        callee: String,
        missing: Vec<String>,
    },
    /// The call graph contains a cycle.
    CycleDetected(Vec<String>),
}

impl std::fmt::Display for PrivilegeViolation {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::UnknownTool(name) => write!(f, "unknown tool: {}", name),
            Self::InsufficientPrivilege {
                caller,
                callee,
                missing,
            } => {
                write!(
                    f,
                    "insufficient privilege: {} cannot call {} (missing: {})",
                    caller,
                    callee,
                    missing.join(", ")
                )
            }
            Self::CycleDetected(path) => {
                write!(f, "cycle detected: {}", path.join(" -> "))
            }
        }
    }
}

impl std::error::Error for PrivilegeViolation {}

/// Validates the tool call graph for privilege safety and acyclicity.
pub struct CallGraphValidator {
    permissions: HashMap<String, PermissionVec>,
    call_edges: HashMap<String, BTreeSet<String>>,
}

impl CallGraphValidator {
    /// Create a new validator with known tool permissions and call edges.
    pub fn new(
        permissions: HashMap<String, PermissionVec>,
        call_edges: HashMap<String, BTreeSet<String>>,
    ) -> Self {
        Self {
            permissions,
            call_edges,
        }
    }

    /// Validate a single call edge: can caller invoke callee?
    pub fn validate_call(
        &self,
        caller: &str,
        callee: &str,
    ) -> Result<(), PrivilegeViolation> {
        let caller_perms = self
            .permissions
            .get(caller)
            .ok_or_else(|| PrivilegeViolation::UnknownTool(caller.to_string()))?;

        // Built-in MCP tools: check if callee is in caller's mcp_tools set
        if !self.permissions.contains_key(callee) {
            // It's a built-in MCP tool — check if caller has it in mcp_tools
            if caller_perms.mcp_tools.contains(callee) {
                return Ok(());
            }
            // Safe MCP tools are always callable if caller is safe or has them
            if is_safe_mcp_tool(callee) {
                return Ok(());
            }
            return Err(PrivilegeViolation::InsufficientPrivilege {
                caller: caller.to_string(),
                callee: callee.to_string(),
                missing: vec![format!("mcp_tools: {{{}}}", callee)],
            });
        }

        let callee_perms = self
            .permissions
            .get(callee)
            .ok_or_else(|| PrivilegeViolation::UnknownTool(callee.to_string()))?;

        if !caller_perms.dominates(callee_perms) {
            let missing = caller_perms.missing_dimensions(callee_perms);
            return Err(PrivilegeViolation::InsufficientPrivilege {
                caller: caller.to_string(),
                callee: callee.to_string(),
                missing,
            });
        }

        Ok(())
    }

    /// Validate the entire graph is a DAG (DFS cycle detection).
    pub fn validate_acyclic(&self) -> Result<(), PrivilegeViolation> {
        let mut visited = HashSet::new();
        let mut on_stack = HashSet::new();
        let mut path = Vec::new();

        for node in self.call_edges.keys() {
            if !visited.contains(node.as_str()) {
                self.dfs_cycle_check(node, &mut visited, &mut on_stack, &mut path)?;
            }
        }

        Ok(())
    }

    fn dfs_cycle_check(
        &self,
        node: &str,
        visited: &mut HashSet<String>,
        on_stack: &mut HashSet<String>,
        path: &mut Vec<String>,
    ) -> Result<(), PrivilegeViolation> {
        visited.insert(node.to_string());
        on_stack.insert(node.to_string());
        path.push(node.to_string());

        if let Some(neighbors) = self.call_edges.get(node) {
            for neighbor in neighbors {
                // Only check edges to custom tools (not built-in MCP tools)
                if !self.call_edges.contains_key(neighbor.as_str())
                    && !self.permissions.contains_key(neighbor.as_str())
                {
                    continue;
                }

                if on_stack.contains(neighbor.as_str()) {
                    // Found a cycle — extract the cycle path
                    let cycle_start = path.iter().position(|n| n == neighbor).unwrap_or(0);
                    let mut cycle: Vec<String> = path[cycle_start..].to_vec();
                    cycle.push(neighbor.clone());
                    return Err(PrivilegeViolation::CycleDetected(cycle));
                }

                if !visited.contains(neighbor.as_str()) {
                    self.dfs_cycle_check(neighbor, visited, on_stack, path)?;
                }
            }
        }

        on_stack.remove(node);
        path.pop();
        Ok(())
    }

    /// Compute effective permissions: join of all transitively reachable tools.
    /// Shown to operator at approval time so they see the true capability surface.
    pub fn effective_permissions(&self, tool: &str) -> Result<PermissionVec, PrivilegeViolation> {
        let base = self
            .permissions
            .get(tool)
            .ok_or_else(|| PrivilegeViolation::UnknownTool(tool.to_string()))?
            .clone();

        let mut visited = HashSet::new();
        self.collect_effective(tool, &mut visited, base)
    }

    fn collect_effective(
        &self,
        tool: &str,
        visited: &mut HashSet<String>,
        mut acc: PermissionVec,
    ) -> Result<PermissionVec, PrivilegeViolation> {
        if !visited.insert(tool.to_string()) {
            return Ok(acc);
        }

        if let Some(neighbors) = self.call_edges.get(tool) {
            for neighbor in neighbors {
                if let Some(neighbor_perms) = self.permissions.get(neighbor.as_str()) {
                    acc = acc.join(neighbor_perms);
                    acc = self.collect_effective(neighbor, visited, acc)?;
                }
                // Built-in MCP tools don't contribute to effective permissions
                // beyond their presence in mcp_tools (already tracked)
            }
        }

        Ok(acc)
    }

    /// Validate all edges in the graph.
    pub fn validate_all(&self) -> Result<(), PrivilegeViolation> {
        self.validate_acyclic()?;

        for (caller, callees) in &self.call_edges {
            for callee in callees {
                self.validate_call(caller, callee)?;
            }
        }

        Ok(())
    }
}

/// Maximum call depth for tool chaining at runtime.
pub const MAX_CALL_DEPTH: u32 = 10;

/// An active tool execution session for tool chaining.
/// Created when a custom tool with `allowed_tools` starts executing,
/// allowing it to call other tools via the /mcp/internal endpoint.
#[derive(Debug, Clone)]
pub struct ToolExecutionSession {
    /// Which tools this session is allowed to call.
    pub allowed_tools: HashSet<String>,
    /// Permission vector of the calling tool (for dominance checks on custom tool calls).
    pub caller_permissions: PermissionVec,
    /// Current call depth (incremented per chained call).
    pub depth: u32,
}

/// Shared session store for tool chaining tokens.
///
/// This is held by both `HttpState` (for the /mcp/internal endpoint) and
/// `ToolState` (for `run_custom_tool` to register sessions). Keeping it
/// separate avoids Arc cycles.
pub struct ToolSessionStore {
    sessions: tokio::sync::RwLock<HashMap<String, ToolExecutionSession>>,
}

impl ToolSessionStore {
    /// Create a new empty session store.
    pub fn new() -> Self {
        Self {
            sessions: tokio::sync::RwLock::new(HashMap::new()),
        }
    }

    /// Register a tool execution session and return its token.
    pub async fn register(
        &self,
        allowed_tools: HashSet<String>,
        caller_permissions: PermissionVec,
        depth: u32,
    ) -> String {
        let token = uuid::Uuid::new_v4().to_string();
        let session = ToolExecutionSession {
            allowed_tools,
            caller_permissions,
            depth,
        };
        self.sessions.write().await.insert(token.clone(), session);
        token
    }

    /// Remove a tool execution session.
    pub async fn remove(&self, token: &str) {
        self.sessions.write().await.remove(token);
    }

    /// Get a tool execution session by token.
    pub async fn get(&self, token: &str) -> Option<ToolExecutionSession> {
        self.sessions.read().await.get(token).cloned()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pvec(
        network: bool,
        secrets: &[&str],
        commands: &[&str],
        mcp_tools: &[&str],
    ) -> PermissionVec {
        PermissionVec {
            network,
            secrets: secrets.iter().map(|s| s.to_string()).collect(),
            commands: commands.iter().map(|s| s.to_string()).collect(),
            mcp_tools: mcp_tools.iter().map(|s| s.to_string()).collect(),
        }
    }

    #[test]
    fn bottom_is_safe() {
        assert!(PermissionVec::bottom().is_safe());
    }

    #[test]
    fn safe_with_safe_mcp_tools() {
        let p = pvec(false, &[], &[], &["query_facts", "list_rules"]);
        assert!(p.is_safe());
    }

    #[test]
    fn unsafe_with_network() {
        let p = pvec(true, &[], &[], &[]);
        assert!(!p.is_safe());
    }

    #[test]
    fn unsafe_with_secrets() {
        let p = pvec(false, &["API_KEY"], &[], &[]);
        assert!(!p.is_safe());
    }

    #[test]
    fn unsafe_with_commands() {
        let p = pvec(false, &[], &["git"], &[]);
        assert!(!p.is_safe());
    }

    #[test]
    fn unsafe_with_unsafe_mcp_tool() {
        let p = pvec(false, &[], &[], &["post_to_bluesky"]);
        assert!(!p.is_safe());
    }

    #[test]
    fn dominance_basic() {
        let a = pvec(true, &["KEY"], &["git"], &["query_facts"]);
        let b = pvec(false, &[], &[], &["query_facts"]);
        assert!(a.dominates(&b));
        assert!(!b.dominates(&a));
    }

    #[test]
    fn dominance_equal() {
        let a = pvec(true, &["KEY"], &[], &[]);
        let b = pvec(true, &["KEY"], &[], &[]);
        assert!(a.dominates(&b));
        assert!(b.dominates(&a));
    }

    #[test]
    fn incomparable_vectors() {
        let a = pvec(true, &[], &[], &[]);
        let b = pvec(false, &["API_KEY"], &[], &[]);
        assert!(!a.dominates(&b));
        assert!(!b.dominates(&a));
        assert_eq!(a.partial_cmp(&b), None);
    }

    #[test]
    fn partial_ord_greater() {
        let a = pvec(true, &["KEY"], &["git"], &[]);
        let b = pvec(false, &[], &[], &[]);
        assert_eq!(a.partial_cmp(&b), Some(Ordering::Greater));
        assert_eq!(b.partial_cmp(&a), Some(Ordering::Less));
    }

    #[test]
    fn join_computes_union() {
        let a = pvec(true, &["A"], &[], &["query_facts"]);
        let b = pvec(false, &["B"], &["git"], &["list_rules"]);
        let joined = a.join(&b);
        assert!(joined.network);
        assert!(joined.secrets.contains("A"));
        assert!(joined.secrets.contains("B"));
        assert!(joined.commands.contains("git"));
        assert!(joined.mcp_tools.contains("query_facts"));
        assert!(joined.mcp_tools.contains("list_rules"));
    }

    #[test]
    fn missing_dimensions_reports_correctly() {
        let caller = pvec(true, &[], &[], &[]);
        let callee = pvec(false, &["API_KEY"], &["git"], &[]);
        let missing = caller.missing_dimensions(&callee);
        assert!(missing.iter().any(|m| m.contains("API_KEY")));
        assert!(missing.iter().any(|m| m.contains("git")));
        assert!(!missing.iter().any(|m| m == "network")); // caller has network
    }

    #[test]
    fn cycle_detection_finds_cycle() {
        let mut permissions = HashMap::new();
        permissions.insert("A".to_string(), pvec(true, &[], &[], &[]));
        permissions.insert("B".to_string(), pvec(false, &[], &[], &[]));

        let mut edges = HashMap::new();
        edges.insert("A".to_string(), BTreeSet::from(["B".to_string()]));
        edges.insert("B".to_string(), BTreeSet::from(["A".to_string()]));

        let validator = CallGraphValidator::new(permissions, edges);
        let result = validator.validate_acyclic();
        assert!(result.is_err());
        if let Err(PrivilegeViolation::CycleDetected(path)) = result {
            assert!(path.len() >= 2);
        }
    }

    #[test]
    fn acyclic_dag_passes() {
        let mut permissions = HashMap::new();
        permissions.insert("A".to_string(), pvec(true, &[], &[], &[]));
        permissions.insert("B".to_string(), pvec(false, &[], &[], &[]));
        permissions.insert("C".to_string(), pvec(false, &[], &[], &[]));

        let mut edges = HashMap::new();
        edges.insert("A".to_string(), BTreeSet::from(["B".to_string()]));
        edges.insert("B".to_string(), BTreeSet::from(["C".to_string()]));

        let validator = CallGraphValidator::new(permissions, edges);
        assert!(validator.validate_acyclic().is_ok());
    }

    #[test]
    fn effective_permissions_transitive() {
        let mut permissions = HashMap::new();
        permissions.insert(
            "A".to_string(),
            pvec(true, &[], &[], &["query_facts"]),
        );
        permissions.insert(
            "B".to_string(),
            pvec(false, &["KEY"], &[], &[]),
        );

        let mut edges = HashMap::new();
        edges.insert("A".to_string(), BTreeSet::from(["B".to_string()]));

        let validator = CallGraphValidator::new(permissions, edges);
        let effective = validator.effective_permissions("A").unwrap();

        // Should be join of A and B
        assert!(effective.network);
        assert!(effective.secrets.contains("KEY"));
        assert!(effective.mcp_tools.contains("query_facts"));
    }

    #[test]
    fn validate_call_succeeds_when_dominant() {
        let mut permissions = HashMap::new();
        permissions.insert(
            "A".to_string(),
            pvec(true, &["KEY"], &["git"], &[]),
        );
        permissions.insert("B".to_string(), pvec(false, &[], &[], &[]));

        let edges = HashMap::new();
        let validator = CallGraphValidator::new(permissions, edges);
        assert!(validator.validate_call("A", "B").is_ok());
    }

    #[test]
    fn validate_call_fails_when_not_dominant() {
        let mut permissions = HashMap::new();
        permissions.insert("A".to_string(), pvec(true, &[], &[], &[]));
        permissions.insert(
            "B".to_string(),
            pvec(false, &["KEY"], &[], &[]),
        );

        let edges = HashMap::new();
        let validator = CallGraphValidator::new(permissions, edges);
        let result = validator.validate_call("A", "B");
        assert!(result.is_err());
    }

    #[test]
    fn validate_call_safe_mcp_always_allowed() {
        let mut permissions = HashMap::new();
        permissions.insert("A".to_string(), pvec(false, &[], &[], &[]));

        let edges = HashMap::new();
        let validator = CallGraphValidator::new(permissions, edges);
        assert!(validator.validate_call("A", "query_facts").is_ok());
    }

    #[test]
    fn validate_call_unsafe_mcp_requires_permission() {
        let mut permissions = HashMap::new();
        permissions.insert("A".to_string(), pvec(false, &[], &[], &[]));

        let edges = HashMap::new();
        let validator = CallGraphValidator::new(permissions, edges);
        assert!(validator.validate_call("A", "post_to_bluesky").is_err());
    }

    #[test]
    fn validate_call_unsafe_mcp_with_permission() {
        let mut permissions = HashMap::new();
        permissions.insert(
            "A".to_string(),
            pvec(false, &[], &[], &["post_to_bluesky"]),
        );

        let edges = HashMap::new();
        let validator = CallGraphValidator::new(permissions, edges);
        assert!(validator.validate_call("A", "post_to_bluesky").is_ok());
    }

    // --- AT URI tests ---

    #[test]
    fn is_at_uri_detects_at_uris() {
        assert!(is_at_uri("at://did:plc:abc/diy.razorgirl.winter.tool/3lbxxx"));
        assert!(!is_at_uri("query_facts"));
        assert!(!is_at_uri("post_to_bluesky"));
        assert!(!is_at_uri(""));
    }

    #[test]
    fn parse_at_uri_works() {
        let (did, collection, rkey) =
            parse_at_uri("at://did:plc:abc/diy.razorgirl.winter.tool/3lbxxx").unwrap();
        assert_eq!(did, "did:plc:abc");
        assert_eq!(collection, "diy.razorgirl.winter.tool");
        assert_eq!(rkey, "3lbxxx");
    }

    #[test]
    fn parse_at_uri_invalid() {
        assert!(parse_at_uri("query_facts").is_none());
        assert!(parse_at_uri("at://").is_none());
        assert!(parse_at_uri("at://did:plc:abc").is_none());
        assert!(parse_at_uri("at://did:plc:abc/collection").is_none());
    }

    #[test]
    fn unsafe_with_at_uri_tool_reference() {
        let p = pvec(
            false,
            &[],
            &[],
            &["at://did:plc:abc/diy.razorgirl.winter.tool/3lbxxx"],
        );
        assert!(!p.is_safe());
    }

    #[test]
    fn safe_mcp_plus_at_uri_is_unsafe() {
        let p = pvec(
            false,
            &[],
            &[],
            &[
                "query_facts",
                "at://did:plc:abc/diy.razorgirl.winter.tool/3lbxxx",
            ],
        );
        assert!(!p.is_safe());
    }

    #[test]
    fn validate_call_at_uri_with_permission() {
        let tool_uri = "at://did:plc:abc/diy.razorgirl.winter.tool/3lbxxx";
        let mut permissions = HashMap::new();
        permissions.insert(
            "A".to_string(),
            pvec(false, &[], &[], &[tool_uri]),
        );

        let edges = HashMap::new();
        let validator = CallGraphValidator::new(permissions, edges);
        assert!(validator.validate_call("A", tool_uri).is_ok());
    }

    #[test]
    fn validate_call_at_uri_without_permission() {
        let tool_uri = "at://did:plc:abc/diy.razorgirl.winter.tool/3lbxxx";
        let mut permissions = HashMap::new();
        permissions.insert("A".to_string(), pvec(false, &[], &[], &[]));

        let edges = HashMap::new();
        let validator = CallGraphValidator::new(permissions, edges);
        assert!(validator.validate_call("A", tool_uri).is_err());
    }

    // --- ToolSessionStore tests ---

    #[tokio::test]
    async fn session_store_register_and_get() {
        let store = ToolSessionStore::new();
        let allowed: HashSet<String> = ["query_facts".to_string()].into();
        let perms = PermissionVec::bottom();

        let token = store.register(allowed.clone(), perms, 0).await;
        assert!(!token.is_empty());

        let session = store.get(&token).await.unwrap();
        assert_eq!(session.allowed_tools, allowed);
        assert_eq!(session.depth, 0);
    }

    #[tokio::test]
    async fn session_store_remove() {
        let store = ToolSessionStore::new();
        let token = store
            .register(HashSet::new(), PermissionVec::bottom(), 0)
            .await;

        assert!(store.get(&token).await.is_some());
        store.remove(&token).await;
        assert!(store.get(&token).await.is_none());
    }

    #[tokio::test]
    async fn session_store_invalid_token_returns_none() {
        let store = ToolSessionStore::new();
        assert!(store.get("nonexistent-token").await.is_none());
    }

    // --- Network detection tests ---

    #[test]
    fn detects_remote_es_import() {
        assert!(code_needs_network(
            r#"import { serve } from "https://deno.land/std/http/server.ts";"#
        ));
    }

    #[test]
    fn detects_npm_specifier() {
        assert!(code_needs_network(r#"import chalk from "npm:chalk";"#));
    }

    #[test]
    fn detects_jsr_specifier() {
        assert!(code_needs_network(
            r#"import { parse } from "jsr:@std/csv";"#
        ));
    }

    #[test]
    fn detects_fetch_call() {
        assert!(code_needs_network("const res = await fetch(url);"));
    }

    #[test]
    fn no_network_for_local_code() {
        assert!(!code_needs_network(
            r#"export default async function(input) { return input.x + 1; }"#
        ));
    }

    #[test]
    fn no_network_for_relative_import() {
        assert!(!code_needs_network(
            r#"import { helper } from "./utils.ts";"#
        ));
    }
}
