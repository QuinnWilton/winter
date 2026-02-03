//! Note tools for MCP.

use std::collections::HashMap;

use chrono::Utc;
use serde_json::{Value, json};

use crate::protocol::{CallToolResult, ToolDefinition};
use winter_atproto::{Note, Tid};

use super::ToolState;

/// Collection name for notes.
const NOTE_COLLECTION: &str = "diy.razorgirl.winter.note";

/// Maximum content size (50KB).
const MAX_CONTENT_SIZE: usize = 50 * 1024;

pub fn definitions() -> Vec<ToolDefinition> {
    vec![
        ToolDefinition {
            name: "create_note".to_string(),
            description: "Create a new note. Notes are free-form markdown for investigations, summaries, and reflections.".to_string(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "title": {
                        "type": "string",
                        "description": "Title of the note"
                    },
                    "content": {
                        "type": "string",
                        "description": "Markdown content (max 50KB)"
                    },
                    "category": {
                        "type": "string",
                        "description": "Optional category for organization"
                    },
                    "tags": {
                        "type": "array",
                        "items": { "type": "string" },
                        "description": "Optional tags for categorization"
                    }
                },
                "required": ["title", "content"]
            }),
        },
        ToolDefinition {
            name: "get_note".to_string(),
            description: "Get a note by its record key.".to_string(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "rkey": {
                        "type": "string",
                        "description": "Record key of the note"
                    }
                },
                "required": ["rkey"]
            }),
        },
        ToolDefinition {
            name: "list_notes".to_string(),
            description: "List all notes, optionally filtered by category or tags.".to_string(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "category": {
                        "type": "string",
                        "description": "Filter by category"
                    },
                    "tag": {
                        "type": "string",
                        "description": "Filter by tag"
                    },
                    "search": {
                        "type": "string",
                        "description": "Filter by title or content (case-insensitive substring)"
                    },
                    "limit": {
                        "type": "integer",
                        "description": "Maximum number of notes to return (default 20)"
                    }
                }
            }),
        },
    ]
}

pub async fn create_note(state: &ToolState, arguments: &HashMap<String, Value>) -> CallToolResult {
    let title = match arguments.get("title").and_then(|v| v.as_str()) {
        Some(t) => t,
        None => return CallToolResult::error("Missing required parameter: title"),
    };

    let content = match arguments.get("content").and_then(|v| v.as_str()) {
        Some(c) => c,
        None => return CallToolResult::error("Missing required parameter: content"),
    };

    // Check content size
    if content.len() > MAX_CONTENT_SIZE {
        return CallToolResult::error("Content exceeds maximum size of 50KB");
    }

    let category = arguments
        .get("category")
        .and_then(|v| v.as_str())
        .map(String::from);

    let tags: Vec<String> = arguments
        .get("tags")
        .and_then(|v| v.as_array())
        .map(|a| {
            a.iter()
                .filter_map(|v| v.as_str().map(String::from))
                .collect()
        })
        .unwrap_or_default();

    let now = Utc::now();
    let note = Note {
        title: title.to_string(),
        content: content.to_string(),
        category,
        related_facts: Vec::new(),
        tags,
        created_at: now,
        last_updated: now,
    };

    let rkey = Tid::now().to_string();

    match state
        .atproto
        .create_record(NOTE_COLLECTION, Some(&rkey), &note)
        .await
    {
        Ok(response) => {
            // Update cache so subsequent queries see the change immediately
            if let Some(cache) = &state.cache {
                cache.upsert_note(rkey.clone(), note.clone(), response.cid.clone());
            }
            CallToolResult::success(
                json!({
                    "rkey": rkey,
                    "uri": response.uri,
                    "cid": response.cid,
                    "title": title
                })
                .to_string(),
            )
        }
        Err(e) => CallToolResult::error(format!("Failed to create note: {}", e)),
    }
}

pub async fn get_note(state: &ToolState, arguments: &HashMap<String, Value>) -> CallToolResult {
    let rkey = match arguments.get("rkey").and_then(|v| v.as_str()) {
        Some(r) => r,
        None => return CallToolResult::error("Missing required parameter: rkey"),
    };

    match state
        .atproto
        .get_record::<Note>(NOTE_COLLECTION, rkey)
        .await
    {
        Ok(record) => CallToolResult::success(
            json!({
                "rkey": rkey,
                "title": record.value.title,
                "content": record.value.content,
                "category": record.value.category,
                "tags": record.value.tags,
                "created_at": record.value.created_at.to_rfc3339(),
                "last_updated": record.value.last_updated.to_rfc3339()
            })
            .to_string(),
        ),
        Err(e) => CallToolResult::error(format!("Failed to get note: {}", e)),
    }
}

pub async fn list_notes(state: &ToolState, arguments: &HashMap<String, Value>) -> CallToolResult {
    let category_filter = arguments.get("category").and_then(|v| v.as_str());
    let tag_filter = arguments.get("tag").and_then(|v| v.as_str());
    let search_filter = arguments.get("search").and_then(|v| v.as_str());
    let limit = arguments
        .get("limit")
        .and_then(|v| v.as_u64())
        .unwrap_or(20) as usize;

    // Try cache first, fall back to HTTP
    let notes = if let Some(ref cache) = state.cache {
        if cache.state() == winter_atproto::SyncState::Live {
            tracing::debug!("using cache for list_notes");
            cache
                .list_notes()
                .into_iter()
                .map(|(rkey, cached)| winter_atproto::ListRecordItem {
                    uri: format!("at://did/{}:{}", NOTE_COLLECTION, rkey),
                    cid: cached.cid,
                    value: cached.value,
                })
                .collect()
        } else {
            match state
                .atproto
                .list_all_records::<Note>(NOTE_COLLECTION)
                .await
            {
                Ok(records) => records,
                Err(e) => return CallToolResult::error(format!("Failed to list notes: {}", e)),
            }
        }
    } else {
        match state
            .atproto
            .list_all_records::<Note>(NOTE_COLLECTION)
            .await
        {
            Ok(records) => records,
            Err(e) => return CallToolResult::error(format!("Failed to list notes: {}", e)),
        }
    };

    let formatted: Vec<Value> = notes
        .into_iter()
        .filter(|item| {
            // Filter by category if specified
            if let Some(cat) = category_filter
                && item.value.category.as_deref() != Some(cat)
            {
                return false;
            }
            // Filter by tag if specified
            if let Some(tag) = tag_filter
                && !item.value.tags.contains(&tag.to_string())
            {
                return false;
            }
            // Filter by title or content (case-insensitive substring)
            if let Some(search) = search_filter {
                let search_lower = search.to_lowercase();
                if !item.value.title.to_lowercase().contains(&search_lower)
                    && !item.value.content.to_lowercase().contains(&search_lower)
                {
                    return false;
                }
            }
            true
        })
        .take(limit)
        .map(|item| {
            let rkey = item.uri.split('/').next_back().unwrap_or("");
            // Truncate content for listing
            let preview = if item.value.content.len() > 100 {
                format!("{}...", &item.value.content[..100])
            } else {
                item.value.content.clone()
            };
            json!({
                "rkey": rkey,
                "title": item.value.title,
                "preview": preview,
                "category": item.value.category,
                "tags": item.value.tags,
                "created_at": item.value.created_at.to_rfc3339()
            })
        })
        .collect();

    CallToolResult::success(
        json!({
            "count": formatted.len(),
            "notes": formatted
        })
        .to_string(),
    )
}
