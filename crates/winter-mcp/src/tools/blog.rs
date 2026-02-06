//! Blog tools for MCP - WhiteWind integration.
//!
//! WhiteWind is a blogging platform built on ATProto.
//! Posts are stored as `com.whtwnd.blog.entry` records.

use std::collections::HashMap;

use chrono::Utc;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::protocol::{CallToolResult, ToolDefinition};
use winter_atproto::{AtUri, Tid};

use super::{ToolMeta, ToolState};

/// Collection name for WhiteWind blog entries.
const BLOG_COLLECTION: &str = "com.whtwnd.blog.entry";

/// WhiteWind blog entry record.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BlogEntry {
    /// Title of the blog post.
    pub title: String,
    /// Markdown content of the post.
    pub content: String,
    /// When the post was created.
    pub created_at: String,
    /// Whether the post is visible (draft = false means visible).
    #[serde(default)]
    pub draft: bool,
    /// Optional theme (e.g., "github-light").
    #[serde(skip_serializing_if = "Option::is_none")]
    pub theme: Option<String>,
    /// Optional OGP (Open Graph Protocol) settings.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ogp: Option<BlogOgp>,
}

/// Open Graph Protocol settings for blog entries.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BlogOgp {
    /// OGP title (falls back to post title if not set).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    /// OGP description.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
}

pub fn definitions() -> Vec<ToolDefinition> {
    vec![
        ToolDefinition {
            name: "publish_blog_post".to_string(),
            description: "Publish a blog post. Posts are stored as ATProto records and visible at greengale.app.".to_string(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "title": {
                        "type": "string",
                        "description": "Title of the blog post"
                    },
                    "content": {
                        "type": "string",
                        "description": "Markdown content of the post"
                    },
                    "draft": {
                        "type": "boolean",
                        "description": "If true, post is saved as draft (not publicly visible). Default: false"
                    },
                    "theme": {
                        "type": "string",
                        "description": "Optional theme for rendering (e.g., 'github-light', 'github-dark')"
                    },
                    "description": {
                        "type": "string",
                        "description": "Optional description for social media previews (OGP)"
                    }
                },
                "required": ["title", "content"]
            }),
        },
        ToolDefinition {
            name: "update_blog_post".to_string(),
            description: "Update an existing blog post. Only provided fields are changed; others are preserved.".to_string(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "rkey": {
                        "type": "string",
                        "description": "Record key of the blog post to update"
                    },
                    "title": {
                        "type": "string",
                        "description": "New title (optional)"
                    },
                    "content": {
                        "type": "string",
                        "description": "New markdown content (optional)"
                    },
                    "draft": {
                        "type": "boolean",
                        "description": "Set draft status (optional)"
                    },
                    "theme": {
                        "type": "string",
                        "description": "Set theme (optional)"
                    },
                    "description": {
                        "type": "string",
                        "description": "Set OGP description (optional)"
                    }
                },
                "required": ["rkey"]
            }),
        },
        ToolDefinition {
            name: "list_blog_posts".to_string(),
            description: "List all blog posts. Returns rkey, title, draft status, and created_at for each post.".to_string(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "draft": {
                        "type": "boolean",
                        "description": "Filter by draft status (true = drafts only, false = published only)"
                    },
                    "search": {
                        "type": "string",
                        "description": "Filter by title (case-insensitive substring)"
                    },
                    "limit": {
                        "type": "integer",
                        "description": "Maximum number of posts to return"
                    }
                }
            }),
        },
        ToolDefinition {
            name: "get_blog_post".to_string(),
            description: "Get a blog post by its record key, including full content.".to_string(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "rkey": {
                        "type": "string",
                        "description": "Record key of the blog post"
                    }
                },
                "required": ["rkey"]
            }),
        },
    ]
}

/// Get all blog tools with their permission metadata.
/// All blog tools are allowed for the autonomous agent.
pub fn tools() -> Vec<ToolMeta> {
    definitions().into_iter().map(ToolMeta::allowed).collect()
}

pub async fn publish_blog_post(
    state: &ToolState,
    arguments: &HashMap<String, Value>,
) -> CallToolResult {
    let title = match arguments.get("title").and_then(|v| v.as_str()) {
        Some(t) => t,
        None => return CallToolResult::error("Missing required parameter: title"),
    };

    let content = match arguments.get("content").and_then(|v| v.as_str()) {
        Some(c) => c,
        None => return CallToolResult::error("Missing required parameter: content"),
    };

    let draft = arguments
        .get("draft")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);

    let theme = arguments
        .get("theme")
        .and_then(|v| v.as_str())
        .map(String::from);

    let description = arguments
        .get("description")
        .and_then(|v| v.as_str())
        .map(String::from);

    let ogp = description.map(|desc| BlogOgp {
        title: None,
        description: Some(desc),
    });

    let entry = BlogEntry {
        title: title.to_string(),
        content: content.to_string(),
        created_at: Utc::now().to_rfc3339(),
        draft,
        theme,
        ogp,
    };

    let rkey = Tid::now().to_string();

    match state
        .atproto
        .create_record(BLOG_COLLECTION, Some(&rkey), &entry)
        .await
    {
        Ok(response) => {
            // Update cache so subsequent queries see the change immediately
            if let Some(cache) = &state.cache {
                // Convert local BlogEntry to winter_atproto::BlogEntry
                let atproto_entry = winter_atproto::BlogEntry {
                    title: entry.title.clone(),
                    content: entry.content.clone(),
                    created_at: entry.created_at.clone(),
                    draft: entry.draft,
                    theme: entry.theme.clone(),
                    ogp: entry.ogp.as_ref().map(|o| winter_atproto::BlogOgp {
                        title: o.title.clone(),
                        description: o.description.clone(),
                    }),
                };
                cache.upsert_blog_entry(rkey.clone(), atproto_entry, response.cid.clone());
            }

            let handle = state.atproto.handle().await.unwrap_or_default();
            let url = format!("https://greengale.app/{}/{}", handle, rkey);

            CallToolResult::success(
                json!({
                    "rkey": rkey,
                    "uri": response.uri,
                    "cid": response.cid,
                    "title": title,
                    "draft": draft,
                    "url": url
                })
                .to_string(),
            )
        }
        Err(e) => CallToolResult::error(format!("Failed to publish blog post: {}", e)),
    }
}

pub async fn update_blog_post(
    state: &ToolState,
    arguments: &HashMap<String, Value>,
) -> CallToolResult {
    let rkey = match arguments.get("rkey").and_then(|v| v.as_str()) {
        Some(r) => r,
        None => return CallToolResult::error("Missing required parameter: rkey"),
    };

    // Fetch existing blog entry
    let mut entry: BlogEntry = match state
        .atproto
        .get_record::<BlogEntry>(BLOG_COLLECTION, rkey)
        .await
    {
        Ok(record) => record.value,
        Err(e) => return CallToolResult::error(format!("Failed to get blog post: {}", e)),
    };

    // Apply updates (only for provided fields)
    if let Some(title) = arguments.get("title").and_then(|v| v.as_str()) {
        entry.title = title.to_string();
    }

    if let Some(content) = arguments.get("content").and_then(|v| v.as_str()) {
        entry.content = content.to_string();
    }

    if let Some(draft) = arguments.get("draft").and_then(|v| v.as_bool()) {
        entry.draft = draft;
    }

    if let Some(theme) = arguments.get("theme").and_then(|v| v.as_str()) {
        entry.theme = Some(theme.to_string());
    }

    if let Some(description) = arguments.get("description").and_then(|v| v.as_str()) {
        entry.ogp = Some(BlogOgp {
            title: entry.ogp.as_ref().and_then(|o| o.title.clone()),
            description: Some(description.to_string()),
        });
    }

    // Put the updated record
    match state
        .atproto
        .put_record(BLOG_COLLECTION, rkey, &entry)
        .await
    {
        Ok(response) => {
            // Update cache with the modified entry
            if let Some(cache) = &state.cache {
                // Convert local BlogEntry to winter_atproto::BlogEntry
                let atproto_entry = winter_atproto::BlogEntry {
                    title: entry.title.clone(),
                    content: entry.content.clone(),
                    created_at: entry.created_at.clone(),
                    draft: entry.draft,
                    theme: entry.theme.clone(),
                    ogp: entry.ogp.as_ref().map(|o| winter_atproto::BlogOgp {
                        title: o.title.clone(),
                        description: o.description.clone(),
                    }),
                };
                cache.upsert_blog_entry(rkey.to_string(), atproto_entry, response.cid.clone());
            }

            let handle = state.atproto.handle().await.unwrap_or_default();
            let url = format!("https://greengale.app/{}/{}", handle, rkey);

            CallToolResult::success(
                json!({
                    "rkey": rkey,
                    "uri": response.uri,
                    "cid": response.cid,
                    "title": entry.title,
                    "draft": entry.draft,
                    "url": url
                })
                .to_string(),
            )
        }
        Err(e) => CallToolResult::error(format!("Failed to update blog post: {}", e)),
    }
}

pub async fn list_blog_posts(
    state: &ToolState,
    arguments: &HashMap<String, Value>,
) -> CallToolResult {
    let draft_filter = arguments.get("draft").and_then(|v| v.as_bool());
    let search_filter = arguments.get("search").and_then(|v| v.as_str());
    let limit = arguments.get("limit").and_then(|v| v.as_u64());

    let records = match state
        .atproto
        .list_all_records::<BlogEntry>(BLOG_COLLECTION)
        .await
    {
        Ok(r) => r,
        Err(e) => return CallToolResult::error(format!("Failed to list blog posts: {}", e)),
    };

    let handle = state.atproto.handle().await.unwrap_or_default();

    let posts: Vec<Value> = records
        .iter()
        .filter(|item| {
            // Filter by draft status
            if let Some(draft) = draft_filter
                && item.value.draft != draft
            {
                return false;
            }
            // Filter by title (case-insensitive substring)
            if let Some(search) = search_filter
                && !item
                    .value
                    .title
                    .to_lowercase()
                    .contains(&search.to_lowercase())
            {
                return false;
            }
            true
        })
        .take(limit.unwrap_or(usize::MAX as u64) as usize)
        .map(|item| {
            let rkey = AtUri::extract_rkey(&item.uri);
            let url = format!("https://greengale.app/{}/{}", handle, rkey);
            json!({
                "rkey": rkey,
                "title": item.value.title,
                "draft": item.value.draft,
                "created_at": item.value.created_at,
                "url": url
            })
        })
        .collect();

    CallToolResult::success(
        json!({
            "posts": posts,
            "count": posts.len()
        })
        .to_string(),
    )
}

pub async fn get_blog_post(
    state: &ToolState,
    arguments: &HashMap<String, Value>,
) -> CallToolResult {
    let rkey = match arguments.get("rkey").and_then(|v| v.as_str()) {
        Some(r) => r,
        None => return CallToolResult::error("Missing required parameter: rkey"),
    };

    match state
        .atproto
        .get_record::<BlogEntry>(BLOG_COLLECTION, rkey)
        .await
    {
        Ok(record) => {
            let handle = state.atproto.handle().await.unwrap_or_default();
            let url = format!("https://greengale.app/{}/{}", handle, rkey);

            CallToolResult::success(
                json!({
                    "rkey": rkey,
                    "title": record.value.title,
                    "content": record.value.content,
                    "draft": record.value.draft,
                    "theme": record.value.theme,
                    "ogp": record.value.ogp,
                    "created_at": record.value.created_at,
                    "url": url
                })
                .to_string(),
            )
        }
        Err(e) => CallToolResult::error(format!("Failed to get blog post: {}", e)),
    }
}
