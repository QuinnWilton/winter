//! Wiki tools for MCP — semantic wiki entries and typed links.

use std::collections::HashMap;
use std::sync::LazyLock;

use chrono::Utc;
use regex::Regex;
use serde_json::{Value, json};

static WIKI_REF_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\[\[([^\]|]+?)(?:\|([^\]]+))?\]\]").unwrap());

use crate::protocol::{CallToolResult, ToolDefinition};
use winter_atproto::{Tid, WikiEntry, WikiLink, WIKI_ENTRY_COLLECTION, WIKI_LINK_COLLECTION};

use super::{ToolMeta, ToolState, truncate_for_summary};

/// Maximum content size (100KB).
const MAX_CONTENT_SIZE: usize = 100 * 1024;

/// Maximum slug length.
const MAX_SLUG_LENGTH: usize = 128;

/// Valid status values.
const VALID_STATUSES: &[&str] = &["draft", "stable", "deprecated"];

/// Known link types.
const KNOWN_LINK_TYPES: &[&str] = &[
    "related-to",
    "depends-on",
    "extends",
    "contradicts",
    "is-example-of",
    "supersedes",
    "references",
    "defines",
    "is-part-of",
];

// ============================================================================
// Wiki-link syntax parsing
// ============================================================================

/// A parsed wiki reference from `[[...]]` syntax.
#[derive(Debug, Clone, PartialEq)]
pub enum WikiRef {
    /// `[[slug]]` or `[[slug|text]]` — same author.
    Local { slug: String },
    /// `[[handle/slug]]` — cross-user by handle.
    ByHandle { handle: String, slug: String },
    /// `[[did:plc:xxx/slug]]` — cross-user by DID.
    ByDid { did: String, slug: String },
}

/// Parse all `[[...]]` wiki references from markdown content.
pub fn parse_wiki_refs(content: &str) -> Vec<(WikiRef, Option<String>)> {
    let mut refs = Vec::new();

    for cap in WIKI_REF_RE.captures_iter(content) {
        let reference = cap[1].trim();
        let display_text = cap.get(2).map(|m| m.as_str().trim().to_string());

        let wiki_ref = if reference.starts_with("did:") {
            // [[did:plc:xxx/slug]]
            if let Some(slash_pos) = reference.find('/') {
                WikiRef::ByDid {
                    did: reference[..slash_pos].to_string(),
                    slug: reference[slash_pos + 1..].to_string(),
                }
            } else {
                continue; // Invalid: DID without slug
            }
        } else if reference.contains('/') {
            // [[handle/slug]]
            if let Some(slash_pos) = reference.find('/') {
                WikiRef::ByHandle {
                    handle: reference[..slash_pos].to_string(),
                    slug: reference[slash_pos + 1..].to_string(),
                }
            } else {
                continue;
            }
        } else {
            // [[slug]]
            WikiRef::Local {
                slug: reference.to_string(),
            }
        };

        refs.push((wiki_ref, display_text));
    }

    refs
}

/// Validate a slug (lowercase alphanumeric + hyphens).
fn is_valid_slug(slug: &str) -> bool {
    !slug.is_empty()
        && slug.len() <= MAX_SLUG_LENGTH
        && slug
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
        && !slug.starts_with('-')
        && !slug.ends_with('-')
}

// ============================================================================
// Tool definitions
// ============================================================================

pub fn definitions() -> Vec<ToolDefinition> {
    vec![
        ToolDefinition {
            name: "create_wiki_entry".to_string(),
            description: "Create a new wiki entry. Validates slug uniqueness and auto-creates WikiLink records from [[wiki-link]] syntax in content.".to_string(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "title": {
                        "type": "string",
                        "description": "Display title of the wiki entry"
                    },
                    "slug": {
                        "type": "string",
                        "description": "URL-safe identifier for [[slug]] linking. Lowercase alphanumeric + hyphens only."
                    },
                    "content": {
                        "type": "string",
                        "description": "Markdown content with [[wiki-link]] syntax (max 100KB)"
                    },
                    "status": {
                        "type": "string",
                        "description": "Lifecycle status: draft, stable, deprecated. Default: stable",
                        "enum": ["draft", "stable", "deprecated"]
                    },
                    "summary": {
                        "type": "string",
                        "description": "Plain-text abstract for previews (max 512 chars)"
                    },
                    "aliases": {
                        "type": "array",
                        "items": { "type": "string" },
                        "description": "Alternative names for [[alias]] resolution (max 20)"
                    },
                    "tags": {
                        "type": "array",
                        "items": { "type": "string" },
                        "description": "Tags for categorization (max 20)"
                    },
                    "supersedes": {
                        "type": "string",
                        "description": "AT URI of the previous version of this entry"
                    }
                },
                "required": ["title", "slug", "content"]
            }),
        },
        ToolDefinition {
            name: "update_wiki_entry".to_string(),
            description: "Update an existing wiki entry. Only provided fields are changed. Reconciles WikiLink records from [[wiki-link]] syntax changes.".to_string(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "rkey": {
                        "type": "string",
                        "description": "Record key of the wiki entry to update"
                    },
                    "title": {
                        "type": "string",
                        "description": "New title (optional)"
                    },
                    "content": {
                        "type": "string",
                        "description": "New markdown content (optional)"
                    },
                    "status": {
                        "type": "string",
                        "description": "New status (optional)",
                        "enum": ["draft", "stable", "deprecated"]
                    },
                    "summary": {
                        "type": "string",
                        "description": "New summary (optional)"
                    },
                    "aliases": {
                        "type": "array",
                        "items": { "type": "string" },
                        "description": "New aliases (optional, replaces existing)"
                    },
                    "tags": {
                        "type": "array",
                        "items": { "type": "string" },
                        "description": "New tags (optional, replaces existing)"
                    },
                    "supersedes": {
                        "type": "string",
                        "description": "AT URI of the previous version (optional)"
                    }
                },
                "required": ["rkey"]
            }),
        },
        ToolDefinition {
            name: "delete_wiki_entry".to_string(),
            description: "Delete a wiki entry by its record key.".to_string(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "rkey": {
                        "type": "string",
                        "description": "Record key of the wiki entry to delete"
                    }
                },
                "required": ["rkey"]
            }),
        },
        ToolDefinition {
            name: "get_wiki_entry".to_string(),
            description: "Get a wiki entry by its record key, including full content.".to_string(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "rkey": {
                        "type": "string",
                        "description": "Record key of the wiki entry"
                    }
                },
                "required": ["rkey"]
            }),
        },
        ToolDefinition {
            name: "get_wiki_entry_by_slug".to_string(),
            description: "Resolve a slug or alias to a wiki entry. Checks both slugs and aliases.".to_string(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "slug": {
                        "type": "string",
                        "description": "Slug or alias to resolve"
                    }
                },
                "required": ["slug"]
            }),
        },
        ToolDefinition {
            name: "list_wiki_entries".to_string(),
            description: "List wiki entries with optional filtering by tag, status, or text search.".to_string(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "tag": {
                        "type": "string",
                        "description": "Filter by tag"
                    },
                    "status": {
                        "type": "string",
                        "description": "Filter by status (draft, stable, deprecated)"
                    },
                    "search": {
                        "type": "string",
                        "description": "Filter by title, slug, or content (case-insensitive substring)"
                    },
                    "limit": {
                        "type": "integer",
                        "description": "Maximum entries to return (default 20)"
                    }
                }
            }),
        },
        ToolDefinition {
            name: "create_wiki_link".to_string(),
            description: "Create a typed semantic link between two records. Link types: related-to, depends-on, extends, contradicts, is-example-of, supersedes, references, defines, is-part-of.".to_string(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "source": {
                        "type": "string",
                        "description": "AT URI of the source record (the one doing the linking)"
                    },
                    "target": {
                        "type": "string",
                        "description": "AT URI of the target record (can be cross-PDS)"
                    },
                    "link_type": {
                        "type": "string",
                        "description": "Semantic relationship type",
                        "enum": ["related-to", "depends-on", "extends", "contradicts", "is-example-of", "supersedes", "references", "defines", "is-part-of"]
                    },
                    "source_anchor": {
                        "type": "string",
                        "description": "Section heading slug within source (optional)"
                    },
                    "target_anchor": {
                        "type": "string",
                        "description": "Section heading slug within target (optional)"
                    },
                    "context": {
                        "type": "string",
                        "description": "Why this link exists (optional)"
                    }
                },
                "required": ["source", "target", "link_type"]
            }),
        },
        ToolDefinition {
            name: "delete_wiki_link".to_string(),
            description: "Delete a wiki link by its record key.".to_string(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "rkey": {
                        "type": "string",
                        "description": "Record key of the wiki link to delete"
                    }
                },
                "required": ["rkey"]
            }),
        },
        ToolDefinition {
            name: "list_wiki_links".to_string(),
            description: "List wiki links with optional filtering by source, target, or link type.".to_string(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "source": {
                        "type": "string",
                        "description": "Filter by source AT URI"
                    },
                    "target": {
                        "type": "string",
                        "description": "Filter by target AT URI"
                    },
                    "link_type": {
                        "type": "string",
                        "description": "Filter by link type"
                    },
                    "limit": {
                        "type": "integer",
                        "description": "Maximum links to return (default 50)"
                    }
                }
            }),
        },
    ]
}

/// Get all wiki tools with their permission metadata.
/// All wiki tools are allowed for the autonomous agent.
pub fn tools() -> Vec<ToolMeta> {
    definitions().into_iter().map(ToolMeta::allowed).collect()
}

// ============================================================================
// Tool implementations
// ============================================================================

pub async fn create_wiki_entry(
    state: &ToolState,
    arguments: &HashMap<String, Value>,
) -> CallToolResult {
    let title = match arguments.get("title").and_then(|v| v.as_str()) {
        Some(t) => t,
        None => return CallToolResult::error("Missing required parameter: title"),
    };

    let slug = match arguments.get("slug").and_then(|v| v.as_str()) {
        Some(s) => s,
        None => return CallToolResult::error("Missing required parameter: slug"),
    };

    let content = match arguments.get("content").and_then(|v| v.as_str()) {
        Some(c) => c,
        None => return CallToolResult::error("Missing required parameter: content"),
    };

    // Validate slug format
    if !is_valid_slug(slug) {
        return CallToolResult::error(
            "Invalid slug: must be lowercase alphanumeric + hyphens, max 128 chars, cannot start/end with hyphen",
        );
    }

    // Check content size
    if content.len() > MAX_CONTENT_SIZE {
        return CallToolResult::error("Content exceeds maximum size of 100KB");
    }

    let status = arguments
        .get("status")
        .and_then(|v| v.as_str())
        .unwrap_or("stable");

    if !VALID_STATUSES.contains(&status) {
        return CallToolResult::error(format!(
            "Invalid status '{}': must be one of: {}",
            status,
            VALID_STATUSES.join(", ")
        ));
    }

    let summary = arguments
        .get("summary")
        .and_then(|v| v.as_str())
        .map(String::from);

    let aliases: Vec<String> = arguments
        .get("aliases")
        .and_then(|v| v.as_array())
        .map(|a| {
            a.iter()
                .filter_map(|v| v.as_str().map(String::from))
                .collect()
        })
        .unwrap_or_default();

    let tags: Vec<String> = arguments
        .get("tags")
        .and_then(|v| v.as_array())
        .map(|a| {
            a.iter()
                .filter_map(|v| v.as_str().map(String::from))
                .collect()
        })
        .unwrap_or_default();

    let supersedes = arguments
        .get("supersedes")
        .and_then(|v| v.as_str())
        .map(String::from);

    // Check slug uniqueness against cache
    if let Some(ref cache) = state.cache {
        for (_, cached) in cache.list_wiki_entries() {
            if cached.value.slug == slug {
                return CallToolResult::error(format!(
                    "Slug '{}' already in use by entry '{}'",
                    slug, cached.value.title
                ));
            }
        }
    }

    let now = Utc::now();
    let entry = WikiEntry {
        title: title.to_string(),
        slug: slug.to_string(),
        aliases,
        summary,
        content: content.to_string(),
        status: status.to_string(),
        supersedes,
        tags,
        created_at: now,
        last_updated: now,
    };

    let rkey = Tid::now().to_string();

    match state
        .atproto
        .create_record(WIKI_ENTRY_COLLECTION, Some(&rkey), &entry)
        .await
    {
        Ok(response) => {
            let entry_uri = response.uri.clone();

            // Update cache
            if let Some(cache) = &state.cache {
                cache.upsert_wiki_entry(rkey.clone(), entry.clone(), response.cid.clone());
            }

            // Auto-create wiki links from [[wiki-link]] syntax
            let links_created =
                auto_create_wiki_links(state, &entry_uri, &entry.content).await;

            CallToolResult::success(
                json!({
                    "rkey": rkey,
                    "uri": entry_uri,
                    "cid": response.cid,
                    "title": title,
                    "slug": slug,
                    "status": status,
                    "links_created": links_created,
                })
                .to_string(),
            )
        }
        Err(e) => CallToolResult::error(format!("Failed to create wiki entry: {}", e)),
    }
}

pub async fn update_wiki_entry(
    state: &ToolState,
    arguments: &HashMap<String, Value>,
) -> CallToolResult {
    let rkey = match arguments.get("rkey").and_then(|v| v.as_str()) {
        Some(r) => r,
        None => return CallToolResult::error("Missing required parameter: rkey"),
    };

    // Fetch existing entry
    let mut entry: WikiEntry = match state
        .atproto
        .get_record::<WikiEntry>(WIKI_ENTRY_COLLECTION, rkey)
        .await
    {
        Ok(record) => record.value,
        Err(e) => return CallToolResult::error(format!("Failed to get wiki entry: {}", e)),
    };

    let old_content = entry.content.clone();

    // Apply updates
    if let Some(title) = arguments.get("title").and_then(|v| v.as_str()) {
        entry.title = title.to_string();
    }

    if let Some(content) = arguments.get("content").and_then(|v| v.as_str()) {
        if content.len() > MAX_CONTENT_SIZE {
            return CallToolResult::error("Content exceeds maximum size of 100KB");
        }
        entry.content = content.to_string();
    }

    if let Some(status) = arguments.get("status").and_then(|v| v.as_str()) {
        if !VALID_STATUSES.contains(&status) {
            return CallToolResult::error(format!(
                "Invalid status '{}': must be one of: {}",
                status,
                VALID_STATUSES.join(", ")
            ));
        }
        entry.status = status.to_string();
    }

    if let Some(summary) = arguments.get("summary").and_then(|v| v.as_str()) {
        entry.summary = Some(summary.to_string());
    }

    if let Some(aliases) = arguments.get("aliases").and_then(|v| v.as_array()) {
        entry.aliases = aliases
            .iter()
            .filter_map(|v| v.as_str().map(String::from))
            .collect();
    }

    if let Some(tags) = arguments.get("tags").and_then(|v| v.as_array()) {
        entry.tags = tags
            .iter()
            .filter_map(|v| v.as_str().map(String::from))
            .collect();
    }

    if let Some(supersedes) = arguments.get("supersedes").and_then(|v| v.as_str()) {
        entry.supersedes = Some(supersedes.to_string());
    }

    entry.last_updated = Utc::now();

    match state
        .atproto
        .put_record(WIKI_ENTRY_COLLECTION, rkey, &entry)
        .await
    {
        Ok(response) => {
            let entry_uri = response.uri.clone();

            // Update cache
            if let Some(cache) = &state.cache {
                cache.upsert_wiki_entry(rkey.to_string(), entry.clone(), response.cid.clone());
            }

            // Reconcile wiki links if content changed
            let mut links_created = 0;
            let mut links_deleted = 0;
            if entry.content != old_content {
                let (created, deleted) =
                    reconcile_wiki_links(state, &entry_uri, &old_content, &entry.content).await;
                links_created = created;
                links_deleted = deleted;
            }

            CallToolResult::success(
                json!({
                    "rkey": rkey,
                    "uri": entry_uri,
                    "cid": response.cid,
                    "title": entry.title,
                    "slug": entry.slug,
                    "status": entry.status,
                    "links_created": links_created,
                    "links_deleted": links_deleted,
                })
                .to_string(),
            )
        }
        Err(e) => CallToolResult::error(format!("Failed to update wiki entry: {}", e)),
    }
}

pub async fn delete_wiki_entry(
    state: &ToolState,
    arguments: &HashMap<String, Value>,
) -> CallToolResult {
    let rkey = match arguments.get("rkey").and_then(|v| v.as_str()) {
        Some(r) => r,
        None => return CallToolResult::error("Missing required parameter: rkey"),
    };

    match state
        .atproto
        .delete_record(WIKI_ENTRY_COLLECTION, rkey)
        .await
    {
        Ok(()) => {
            if let Some(cache) = &state.cache {
                cache.delete_wiki_entry(rkey);
            }

            CallToolResult::success(
                json!({
                    "deleted": true,
                    "rkey": rkey,
                })
                .to_string(),
            )
        }
        Err(e) => CallToolResult::error(format!("Failed to delete wiki entry: {}", e)),
    }
}

pub async fn get_wiki_entry(
    state: &ToolState,
    arguments: &HashMap<String, Value>,
) -> CallToolResult {
    let rkey = match arguments.get("rkey").and_then(|v| v.as_str()) {
        Some(r) => r,
        None => return CallToolResult::error("Missing required parameter: rkey"),
    };

    // Try cache first
    if let Some(ref cache) = state.cache {
        if cache.state() == winter_atproto::SyncState::Live {
            if let Some(cached) = cache.get_wiki_entry(rkey) {
                return CallToolResult::success(
                    json!({
                        "rkey": rkey,
                        "title": cached.value.title,
                        "slug": cached.value.slug,
                        "aliases": cached.value.aliases,
                        "summary": cached.value.summary,
                        "content": cached.value.content,
                        "status": cached.value.status,
                        "supersedes": cached.value.supersedes,
                        "tags": cached.value.tags,
                        "created_at": cached.value.created_at.to_rfc3339(),
                        "last_updated": cached.value.last_updated.to_rfc3339(),
                    })
                    .to_string(),
                );
            }
        }
    }

    // Fall back to HTTP
    match state
        .atproto
        .get_record::<WikiEntry>(WIKI_ENTRY_COLLECTION, rkey)
        .await
    {
        Ok(record) => CallToolResult::success(
            json!({
                "rkey": rkey,
                "title": record.value.title,
                "slug": record.value.slug,
                "aliases": record.value.aliases,
                "summary": record.value.summary,
                "content": record.value.content,
                "status": record.value.status,
                "supersedes": record.value.supersedes,
                "tags": record.value.tags,
                "created_at": record.value.created_at.to_rfc3339(),
                "last_updated": record.value.last_updated.to_rfc3339(),
            })
            .to_string(),
        ),
        Err(e) => CallToolResult::error(format!("Failed to get wiki entry: {}", e)),
    }
}

pub async fn get_wiki_entry_by_slug(
    state: &ToolState,
    arguments: &HashMap<String, Value>,
) -> CallToolResult {
    let slug = match arguments.get("slug").and_then(|v| v.as_str()) {
        Some(s) => s,
        None => return CallToolResult::error("Missing required parameter: slug"),
    };

    // Search cache for matching slug or alias
    if let Some(ref cache) = state.cache {
        if cache.state() == winter_atproto::SyncState::Live {
            for (rkey, cached) in cache.list_wiki_entries() {
                if cached.value.slug == slug
                    || cached.value.aliases.iter().any(|a| a == slug)
                {
                    return CallToolResult::success(
                        json!({
                            "rkey": rkey,
                            "title": cached.value.title,
                            "slug": cached.value.slug,
                            "aliases": cached.value.aliases,
                            "summary": cached.value.summary,
                            "content": cached.value.content,
                            "status": cached.value.status,
                            "supersedes": cached.value.supersedes,
                            "tags": cached.value.tags,
                            "created_at": cached.value.created_at.to_rfc3339(),
                            "last_updated": cached.value.last_updated.to_rfc3339(),
                        })
                        .to_string(),
                    );
                }
            }
            return CallToolResult::error(format!("No wiki entry found for slug or alias '{}'", slug));
        }
    }

    // Fall back to listing all records via HTTP
    match state
        .atproto
        .list_all_records::<WikiEntry>(WIKI_ENTRY_COLLECTION)
        .await
    {
        Ok(records) => {
            for item in &records {
                if item.value.slug == slug
                    || item.value.aliases.iter().any(|a| a == slug)
                {
                    let rkey = item.uri.split('/').next_back().unwrap_or("");
                    return CallToolResult::success(
                        json!({
                            "rkey": rkey,
                            "title": item.value.title,
                            "slug": item.value.slug,
                            "aliases": item.value.aliases,
                            "summary": item.value.summary,
                            "content": item.value.content,
                            "status": item.value.status,
                            "supersedes": item.value.supersedes,
                            "tags": item.value.tags,
                            "created_at": item.value.created_at.to_rfc3339(),
                            "last_updated": item.value.last_updated.to_rfc3339(),
                        })
                        .to_string(),
                    );
                }
            }
            CallToolResult::error(format!("No wiki entry found for slug or alias '{}'", slug))
        }
        Err(e) => CallToolResult::error(format!("Failed to search wiki entries: {}", e)),
    }
}

pub async fn list_wiki_entries(
    state: &ToolState,
    arguments: &HashMap<String, Value>,
) -> CallToolResult {
    let tag_filter = arguments.get("tag").and_then(|v| v.as_str());
    let status_filter = arguments.get("status").and_then(|v| v.as_str());
    let search_filter = arguments.get("search").and_then(|v| v.as_str());
    let limit = arguments
        .get("limit")
        .and_then(|v| v.as_u64())
        .unwrap_or(20) as usize;

    // Try cache first
    let entries: Vec<(String, WikiEntry)> = if let Some(ref cache) = state.cache {
        if cache.state() == winter_atproto::SyncState::Live {
            cache
                .list_wiki_entries()
                .into_iter()
                .map(|(rkey, cached)| (rkey, cached.value))
                .collect()
        } else {
            match fetch_entries_via_http(state).await {
                Ok(e) => e,
                Err(result) => return result,
            }
        }
    } else {
        match fetch_entries_via_http(state).await {
            Ok(e) => e,
            Err(result) => return result,
        }
    };

    let formatted: Vec<Value> = entries
        .into_iter()
        .filter(|(_, entry)| {
            if let Some(tag) = tag_filter {
                if !entry.tags.contains(&tag.to_string()) {
                    return false;
                }
            }
            if let Some(status) = status_filter {
                if entry.status != status {
                    return false;
                }
            }
            if let Some(search) = search_filter {
                let search_lower = search.to_lowercase();
                if !entry.title.to_lowercase().contains(&search_lower)
                    && !entry.slug.to_lowercase().contains(&search_lower)
                    && !entry.content.to_lowercase().contains(&search_lower)
                {
                    return false;
                }
            }
            true
        })
        .take(limit)
        .map(|(rkey, entry)| {
            let preview = entry
                .summary
                .as_deref()
                .map(|s| s.to_string())
                .unwrap_or_else(|| truncate_for_summary(&entry.content, 120));
            json!({
                "rkey": rkey,
                "title": entry.title,
                "slug": entry.slug,
                "status": entry.status,
                "tags": entry.tags,
                "preview": preview,
                "created_at": entry.created_at.to_rfc3339(),
                "last_updated": entry.last_updated.to_rfc3339(),
            })
        })
        .collect();

    CallToolResult::success(
        json!({
            "count": formatted.len(),
            "entries": formatted,
        })
        .to_string(),
    )
}

pub async fn create_wiki_link(
    state: &ToolState,
    arguments: &HashMap<String, Value>,
) -> CallToolResult {
    let source = match arguments.get("source").and_then(|v| v.as_str()) {
        Some(s) => s,
        None => return CallToolResult::error("Missing required parameter: source"),
    };

    let target = match arguments.get("target").and_then(|v| v.as_str()) {
        Some(t) => t,
        None => return CallToolResult::error("Missing required parameter: target"),
    };

    let link_type = match arguments.get("link_type").and_then(|v| v.as_str()) {
        Some(lt) => lt,
        None => return CallToolResult::error("Missing required parameter: link_type"),
    };

    // Validate AT URI format (basic check)
    if !source.starts_with("at://") {
        return CallToolResult::error("source must be an AT URI (at://...)");
    }
    if !target.starts_with("at://") {
        return CallToolResult::error("target must be an AT URI (at://...)");
    }

    // Warn about unknown link types but don't reject (knownValues is extensible)
    if !KNOWN_LINK_TYPES.contains(&link_type) {
        tracing::warn!(link_type = %link_type, "Unknown link type (creating anyway)");
    }

    let source_anchor = arguments
        .get("source_anchor")
        .and_then(|v| v.as_str())
        .map(String::from);

    let target_anchor = arguments
        .get("target_anchor")
        .and_then(|v| v.as_str())
        .map(String::from);

    let context = arguments
        .get("context")
        .and_then(|v| v.as_str())
        .map(String::from);

    let link = WikiLink {
        source: source.to_string(),
        target: target.to_string(),
        link_type: link_type.to_string(),
        source_anchor,
        target_anchor,
        context,
        created_at: Utc::now(),
    };

    let rkey = Tid::now().to_string();

    match state
        .atproto
        .create_record(WIKI_LINK_COLLECTION, Some(&rkey), &link)
        .await
    {
        Ok(response) => {
            if let Some(cache) = &state.cache {
                cache.insert_wiki_link(rkey.clone(), link, response.cid.clone());
            }

            CallToolResult::success(
                json!({
                    "rkey": rkey,
                    "uri": response.uri,
                    "cid": response.cid,
                    "source": source,
                    "target": target,
                    "link_type": link_type,
                })
                .to_string(),
            )
        }
        Err(e) => CallToolResult::error(format!("Failed to create wiki link: {}", e)),
    }
}

pub async fn delete_wiki_link(
    state: &ToolState,
    arguments: &HashMap<String, Value>,
) -> CallToolResult {
    let rkey = match arguments.get("rkey").and_then(|v| v.as_str()) {
        Some(r) => r,
        None => return CallToolResult::error("Missing required parameter: rkey"),
    };

    match state
        .atproto
        .delete_record(WIKI_LINK_COLLECTION, rkey)
        .await
    {
        Ok(()) => {
            if let Some(cache) = &state.cache {
                cache.delete_wiki_link(rkey);
            }

            CallToolResult::success(
                json!({
                    "deleted": true,
                    "rkey": rkey,
                })
                .to_string(),
            )
        }
        Err(e) => CallToolResult::error(format!("Failed to delete wiki link: {}", e)),
    }
}

pub async fn list_wiki_links(
    state: &ToolState,
    arguments: &HashMap<String, Value>,
) -> CallToolResult {
    let source_filter = arguments.get("source").and_then(|v| v.as_str());
    let target_filter = arguments.get("target").and_then(|v| v.as_str());
    let link_type_filter = arguments.get("link_type").and_then(|v| v.as_str());
    let limit = arguments
        .get("limit")
        .and_then(|v| v.as_u64())
        .unwrap_or(50) as usize;

    // Try cache first
    let links: Vec<(String, WikiLink)> = if let Some(ref cache) = state.cache {
        if cache.state() == winter_atproto::SyncState::Live {
            cache
                .list_wiki_links()
                .into_iter()
                .map(|(rkey, cached)| (rkey, cached.value))
                .collect()
        } else {
            match fetch_links_via_http(state).await {
                Ok(l) => l,
                Err(result) => return result,
            }
        }
    } else {
        match fetch_links_via_http(state).await {
            Ok(l) => l,
            Err(result) => return result,
        }
    };

    let formatted: Vec<Value> = links
        .into_iter()
        .filter(|(_, link)| {
            if let Some(source) = source_filter {
                if link.source != source {
                    return false;
                }
            }
            if let Some(target) = target_filter {
                if link.target != target {
                    return false;
                }
            }
            if let Some(lt) = link_type_filter {
                if link.link_type != lt {
                    return false;
                }
            }
            true
        })
        .take(limit)
        .map(|(rkey, link)| {
            json!({
                "rkey": rkey,
                "source": link.source,
                "target": link.target,
                "link_type": link.link_type,
                "source_anchor": link.source_anchor,
                "target_anchor": link.target_anchor,
                "context": link.context,
                "created_at": link.created_at.to_rfc3339(),
            })
        })
        .collect();

    CallToolResult::success(
        json!({
            "count": formatted.len(),
            "links": formatted,
        })
        .to_string(),
    )
}

// ============================================================================
// Helper functions
// ============================================================================

/// Fetch wiki entries via HTTP (fallback when cache is not live).
async fn fetch_entries_via_http(
    state: &ToolState,
) -> Result<Vec<(String, WikiEntry)>, CallToolResult> {
    match state
        .atproto
        .list_all_records::<WikiEntry>(WIKI_ENTRY_COLLECTION)
        .await
    {
        Ok(records) => Ok(records
            .into_iter()
            .map(|item| {
                let rkey = item.uri.split('/').next_back().unwrap_or("").to_string();
                (rkey, item.value)
            })
            .collect()),
        Err(e) => Err(CallToolResult::error(format!(
            "Failed to list wiki entries: {}",
            e
        ))),
    }
}

/// Fetch wiki links via HTTP (fallback when cache is not live).
async fn fetch_links_via_http(
    state: &ToolState,
) -> Result<Vec<(String, WikiLink)>, CallToolResult> {
    match state
        .atproto
        .list_all_records::<WikiLink>(WIKI_LINK_COLLECTION)
        .await
    {
        Ok(records) => Ok(records
            .into_iter()
            .map(|item| {
                let rkey = item.uri.split('/').next_back().unwrap_or("").to_string();
                (rkey, item.value)
            })
            .collect()),
        Err(e) => Err(CallToolResult::error(format!(
            "Failed to list wiki links: {}",
            e
        ))),
    }
}

/// Resolve a local slug to an AT URI by searching the cache.
fn resolve_local_slug(state: &ToolState, slug: &str) -> Option<String> {
    let cache = state.cache.as_ref()?;
    let did_future = state.atproto.did();

    // Synchronous cache lookup
    for (rkey, cached) in cache.list_wiki_entries() {
        if cached.value.slug == slug || cached.value.aliases.iter().any(|a| a == slug) {
            // We need the DID but can't easily await here. Use a blocking approach.
            // Since we're already inside an async context, use tokio::task::block_in_place
            let did = tokio::task::block_in_place(|| {
                tokio::runtime::Handle::current()
                    .block_on(did_future)
            });
            if let Some(did) = did {
                return Some(format!("at://{}/{}/{}", did, WIKI_ENTRY_COLLECTION, rkey));
            }
            return None;
        }
    }
    None
}

/// Auto-create wiki links from `[[wiki-link]]` syntax in content.
///
/// For each local `[[slug]]` reference, resolves the slug and creates a WikiLink record.
/// Cross-user references (`[[handle/slug]]`, `[[did/slug]]`) are not auto-resolved.
/// Returns the number of links created.
async fn auto_create_wiki_links(
    state: &ToolState,
    source_uri: &str,
    content: &str,
) -> usize {
    let refs = parse_wiki_refs(content);
    let mut created = 0;

    for (wiki_ref, _display) in &refs {
        if let WikiRef::Local { slug } = wiki_ref {
            if let Some(target_uri) = resolve_local_slug(state, slug) {
                let link = WikiLink {
                    source: source_uri.to_string(),
                    target: target_uri,
                    link_type: "related-to".to_string(),
                    source_anchor: None,
                    target_anchor: None,
                    context: None,
                    created_at: Utc::now(),
                };

                let rkey = Tid::now().to_string();
                if let Ok(response) = state
                    .atproto
                    .create_record(WIKI_LINK_COLLECTION, Some(&rkey), &link)
                    .await
                {
                    if let Some(cache) = &state.cache {
                        cache.insert_wiki_link(rkey, link, response.cid);
                    }
                    created += 1;
                }
            }
        }
    }

    created
}

/// Reconcile wiki links when content changes.
///
/// Computes the diff between old and new wiki refs, deletes stale links and creates new ones.
/// Returns (links_created, links_deleted).
async fn reconcile_wiki_links(
    state: &ToolState,
    source_uri: &str,
    old_content: &str,
    new_content: &str,
) -> (usize, usize) {
    let old_refs: Vec<String> = parse_wiki_refs(old_content)
        .into_iter()
        .filter_map(|(r, _)| {
            if let WikiRef::Local { slug } = r {
                Some(slug)
            } else {
                None
            }
        })
        .collect();

    let new_refs: Vec<String> = parse_wiki_refs(new_content)
        .into_iter()
        .filter_map(|(r, _)| {
            if let WikiRef::Local { slug } = r {
                Some(slug)
            } else {
                None
            }
        })
        .collect();

    // Find removed and added slugs
    let removed: Vec<&String> = old_refs.iter().filter(|s| !new_refs.contains(s)).collect();
    let added: Vec<&String> = new_refs.iter().filter(|s| !old_refs.contains(s)).collect();

    let mut deleted = 0;
    let mut created = 0;

    // Delete links for removed references
    if let Some(ref cache) = state.cache {
        for slug in &removed {
            if let Some(target_uri) = resolve_local_slug(state, slug) {
                // Find existing link with this source+target
                for (rkey, cached) in cache.list_wiki_links() {
                    if cached.value.source == source_uri && cached.value.target == target_uri {
                        if state
                            .atproto
                            .delete_record(WIKI_LINK_COLLECTION, &rkey)
                            .await
                            .is_ok()
                        {
                            cache.delete_wiki_link(&rkey);
                            deleted += 1;
                        }
                        break;
                    }
                }
            }
        }
    }

    // Create links for added references
    for slug in &added {
        if let Some(target_uri) = resolve_local_slug(state, slug) {
            let link = WikiLink {
                source: source_uri.to_string(),
                target: target_uri,
                link_type: "related-to".to_string(),
                source_anchor: None,
                target_anchor: None,
                context: None,
                created_at: Utc::now(),
            };

            let rkey = Tid::now().to_string();
            if let Ok(response) = state
                .atproto
                .create_record(WIKI_LINK_COLLECTION, Some(&rkey), &link)
                .await
            {
                if let Some(cache) = &state.cache {
                    cache.insert_wiki_link(rkey, link, response.cid);
                }
                created += 1;
            }
        }
    }

    (created, deleted)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_definitions_not_empty() {
        let defs = definitions();
        assert!(!defs.is_empty());
        for def in defs {
            assert!(!def.name.is_empty());
            assert!(!def.description.is_empty());
        }
    }

    #[test]
    fn test_tools_all_allowed() {
        let tools = tools();
        assert!(!tools.is_empty());
        for tool in tools {
            assert!(tool.agent_allowed);
        }
    }

    #[test]
    fn test_is_valid_slug() {
        assert!(is_valid_slug("hello-world"));
        assert!(is_valid_slug("atproto"));
        assert!(is_valid_slug("my-page-123"));
        assert!(is_valid_slug("a"));

        assert!(!is_valid_slug(""));
        assert!(!is_valid_slug("-starts-with-hyphen"));
        assert!(!is_valid_slug("ends-with-hyphen-"));
        assert!(!is_valid_slug("HAS-UPPERCASE"));
        assert!(!is_valid_slug("has spaces"));
        assert!(!is_valid_slug("has_underscores"));
    }

    #[test]
    fn test_parse_wiki_refs_local() {
        let refs = parse_wiki_refs("Check [[atproto]] and [[federation]].");
        assert_eq!(refs.len(), 2);
        assert_eq!(
            refs[0].0,
            WikiRef::Local {
                slug: "atproto".to_string()
            }
        );
        assert_eq!(refs[0].1, None);
        assert_eq!(
            refs[1].0,
            WikiRef::Local {
                slug: "federation".to_string()
            }
        );
    }

    #[test]
    fn test_parse_wiki_refs_with_display_text() {
        let refs = parse_wiki_refs("See [[atproto|AT Protocol]] for details.");
        assert_eq!(refs.len(), 1);
        assert_eq!(
            refs[0].0,
            WikiRef::Local {
                slug: "atproto".to_string()
            }
        );
        assert_eq!(refs[0].1, Some("AT Protocol".to_string()));
    }

    #[test]
    fn test_parse_wiki_refs_by_handle() {
        let refs = parse_wiki_refs("See [[alice.bsky.social/federation]].");
        assert_eq!(refs.len(), 1);
        assert_eq!(
            refs[0].0,
            WikiRef::ByHandle {
                handle: "alice.bsky.social".to_string(),
                slug: "federation".to_string(),
            }
        );
    }

    #[test]
    fn test_parse_wiki_refs_by_did() {
        let refs = parse_wiki_refs("See [[did:plc:abc123/my-page]].");
        assert_eq!(refs.len(), 1);
        assert_eq!(
            refs[0].0,
            WikiRef::ByDid {
                did: "did:plc:abc123".to_string(),
                slug: "my-page".to_string(),
            }
        );
    }

    #[test]
    fn test_parse_wiki_refs_mixed() {
        let content = "Links: [[local-slug]], [[alice.bsky.social/page|Alice's page]], [[did:plc:xyz/other]]";
        let refs = parse_wiki_refs(content);
        assert_eq!(refs.len(), 3);
        assert!(matches!(refs[0].0, WikiRef::Local { .. }));
        assert!(matches!(refs[1].0, WikiRef::ByHandle { .. }));
        assert_eq!(refs[1].1, Some("Alice's page".to_string()));
        assert!(matches!(refs[2].0, WikiRef::ByDid { .. }));
    }

    #[test]
    fn test_parse_wiki_refs_no_refs() {
        let refs = parse_wiki_refs("No wiki links here.");
        assert!(refs.is_empty());
    }
}
