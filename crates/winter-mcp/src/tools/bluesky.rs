//! Bluesky tools for MCP.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use serde_json::{Value, json};
use winter_atproto::{ByteSlice, Facet, FacetFeature};

use crate::bluesky::{ImageInput, PostRef};
use crate::protocol::{CallToolResult, ToolDefinition};

use super::{ToolMeta, ToolState};

use base64::Engine;

/// Infer MIME type from a file extension.
fn mime_from_extension(path: &Path) -> Option<&'static str> {
    match path.extension().and_then(|e| e.to_str()) {
        Some("jpg" | "jpeg") => Some("image/jpeg"),
        Some("png") => Some("image/png"),
        Some("webp") => Some("image/webp"),
        Some("gif") => Some("image/gif"),
        _ => None,
    }
}

/// Resolve and validate an image path within the workspace.
///
/// The path can be absolute (must be inside workspace) or relative
/// (resolved against workspace root). Path traversal is rejected.
fn resolve_workspace_path(path: &str, workspace: &Path) -> Result<PathBuf, String> {
    let candidate = if Path::new(path).is_absolute() {
        PathBuf::from(path)
    } else {
        workspace.join(path)
    };

    // Canonicalize to resolve symlinks and ..
    let resolved = candidate
        .canonicalize()
        .map_err(|e| format!("cannot access '{}': {}", path, e))?;

    // Must be inside workspace
    if !resolved.starts_with(workspace) {
        return Err(format!(
            "path '{}' is outside the workspace",
            path
        ));
    }

    Ok(resolved)
}

/// Parse images from the JSON arguments.
///
/// Each image needs `alt` text plus one of:
/// - `path`: file path (relative to workspace, or absolute within workspace)
/// - `data`: base64-encoded image bytes
fn parse_images(arguments: &HashMap<String, Value>) -> Result<Vec<ImageInput>, String> {
    let images_value = match arguments.get("images") {
        Some(v) => v,
        None => return Ok(Vec::new()),
    };

    let images_array = match images_value.as_array() {
        Some(arr) => arr,
        None => return Ok(Vec::new()),
    };

    let workspace = std::env::var("WINTER_WORKSPACE")
        .ok()
        .filter(|p| !p.is_empty())
        .map(PathBuf::from);

    let mut images = Vec::new();
    for (i, img) in images_array.iter().enumerate() {
        let alt = img
            .get("alt")
            .and_then(|v| v.as_str())
            .ok_or_else(|| format!("images[{}]: missing 'alt' field", i))?;

        let has_path = img.get("path").and_then(|v| v.as_str()).is_some();
        let has_data = img.get("data").and_then(|v| v.as_str()).is_some();

        let (data, mime_type) = if has_path {
            let path_str = img
                .get("path")
                .and_then(|v| v.as_str())
                .ok_or_else(|| format!("images[{}]: 'path' must be a string", i))?;

            let workspace = workspace.as_ref().ok_or_else(|| {
                format!(
                    "images[{}]: file paths require WINTER_WORKSPACE to be set",
                    i
                )
            })?;

            let resolved = resolve_workspace_path(path_str, workspace)
                .map_err(|e| format!("images[{}]: {}", i, e))?;

            let mime = img
                .get("mime_type")
                .and_then(|v| v.as_str())
                .map(String::from)
                .or_else(|| mime_from_extension(&resolved).map(String::from))
                .ok_or_else(|| {
                    format!(
                        "images[{}]: cannot infer MIME type from '{}' — set mime_type explicitly",
                        i, path_str
                    )
                })?;

            let bytes = std::fs::read(&resolved)
                .map_err(|e| format!("images[{}]: failed to read '{}': {}", i, path_str, e))?;

            (bytes, mime)
        } else if has_data {
            let data_b64 = img
                .get("data")
                .and_then(|v| v.as_str())
                .ok_or_else(|| format!("images[{}]: 'data' must be a string", i))?;

            let mime = img
                .get("mime_type")
                .and_then(|v| v.as_str())
                .unwrap_or("image/jpeg")
                .to_string();

            let bytes = base64::engine::general_purpose::STANDARD
                .decode(data_b64)
                .map_err(|e| format!("images[{}]: invalid base64: {}", i, e))?;

            (bytes, mime)
        } else {
            return Err(format!(
                "images[{}]: must provide either 'path' (workspace file) or 'data' (base64)",
                i
            ));
        };

        images.push(ImageInput {
            data,
            mime_type,
            alt: alt.to_string(),
        });
    }

    Ok(images)
}

/// Parse facets from the JSON arguments.
fn parse_facets(arguments: &HashMap<String, Value>) -> Option<Vec<Facet>> {
    let facets_value = arguments.get("facets")?;
    let facets_array = facets_value.as_array()?;

    let facets: Vec<Facet> = facets_array
        .iter()
        .filter_map(|f| {
            let byte_start = f.get("byte_start")?.as_u64()?;
            let byte_end = f.get("byte_end")?.as_u64()?;

            let mut features = Vec::new();

            // Check for mention
            if let Some(did) = f.get("mention_did").and_then(|v| v.as_str()) {
                features.push(FacetFeature::Mention {
                    did: did.to_string(),
                });
            }

            // Check for link
            if let Some(uri) = f.get("link_uri").and_then(|v| v.as_str()) {
                features.push(FacetFeature::Link {
                    uri: uri.to_string(),
                });
            }

            // Check for tag
            if let Some(tag) = f.get("tag").and_then(|v| v.as_str()) {
                features.push(FacetFeature::Tag {
                    tag: tag.to_string(),
                });
            }

            // Must have at least one feature
            if features.is_empty() {
                return None;
            }

            Some(Facet {
                index: ByteSlice {
                    byte_start,
                    byte_end,
                },
                features,
            })
        })
        .collect();

    if facets.is_empty() {
        None
    } else {
        Some(facets)
    }
}

pub fn definitions() -> Vec<ToolDefinition> {
    vec![
        ToolDefinition {
            name: "post_to_bluesky".to_string(),
            description: "Post a new message to Bluesky, optionally with images".to_string(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "text": {
                        "type": "string",
                        "description": "The text content of the post (max 300 graphemes)"
                    },
                    "images": {
                        "type": "array",
                        "description": "Images to attach (max 4). Provide either a workspace file path or base64 data for each image.",
                        "items": {
                            "type": "object",
                            "properties": {
                                "path": { "type": "string", "description": "File path relative to the workspace (e.g., 'output/chart.png'). MIME type is inferred from extension." },
                                "data": { "type": "string", "description": "Base64-encoded image data (alternative to path)" },
                                "alt": { "type": "string", "description": "Alt text description (required for accessibility)" },
                                "mime_type": { "type": "string", "description": "MIME type (auto-detected from path extension, or default image/jpeg for base64). Supported: image/jpeg, image/png, image/webp, image/gif" }
                            },
                            "required": ["alt"]
                        }
                    },
                    "facets": {
                        "type": "array",
                        "description": "Rich text facets for mentions, links, and hashtags. If provided, auto-detection is skipped.",
                        "items": {
                            "type": "object",
                            "properties": {
                                "byte_start": { "type": "integer", "description": "Start byte index in the text" },
                                "byte_end": { "type": "integer", "description": "End byte index in the text" },
                                "mention_did": { "type": "string", "description": "DID for mention facet (e.g., did:plc:xxx)" },
                                "link_uri": { "type": "string", "description": "URI for link facet" },
                                "tag": { "type": "string", "description": "Hashtag (without #)" }
                            },
                            "required": ["byte_start", "byte_end"]
                        }
                    }
                },
                "required": ["text"]
            }),
        },
        ToolDefinition {
            name: "reply_to_bluesky".to_string(),
            description: "Reply to an existing Bluesky post, optionally with images".to_string(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "text": {
                        "type": "string",
                        "description": "The text content of the reply (max 300 graphemes)"
                    },
                    "parent_uri": {
                        "type": "string",
                        "description": "AT URI of the post to reply to"
                    },
                    "parent_cid": {
                        "type": "string",
                        "description": "CID of the post to reply to"
                    },
                    "root_uri": {
                        "type": "string",
                        "description": "AT URI of the thread root (same as parent for direct replies)"
                    },
                    "root_cid": {
                        "type": "string",
                        "description": "CID of the thread root"
                    },
                    "images": {
                        "type": "array",
                        "description": "Images to attach (max 4). Provide either a workspace file path or base64 data for each image.",
                        "items": {
                            "type": "object",
                            "properties": {
                                "path": { "type": "string", "description": "File path relative to the workspace (e.g., 'output/chart.png'). MIME type is inferred from extension." },
                                "data": { "type": "string", "description": "Base64-encoded image data (alternative to path)" },
                                "alt": { "type": "string", "description": "Alt text description (required for accessibility)" },
                                "mime_type": { "type": "string", "description": "MIME type (auto-detected from path extension, or default image/jpeg for base64). Supported: image/jpeg, image/png, image/webp, image/gif" }
                            },
                            "required": ["alt"]
                        }
                    },
                    "facets": {
                        "type": "array",
                        "description": "Rich text facets for mentions, links, and hashtags. If provided, auto-detection is skipped.",
                        "items": {
                            "type": "object",
                            "properties": {
                                "byte_start": { "type": "integer", "description": "Start byte index in the text" },
                                "byte_end": { "type": "integer", "description": "End byte index in the text" },
                                "mention_did": { "type": "string", "description": "DID for mention facet (e.g., did:plc:xxx)" },
                                "link_uri": { "type": "string", "description": "URI for link facet" },
                                "tag": { "type": "string", "description": "Hashtag (without #)" }
                            },
                            "required": ["byte_start", "byte_end"]
                        }
                    }
                },
                "required": ["text", "parent_uri", "parent_cid", "root_uri", "root_cid"]
            }),
        },
        ToolDefinition {
            name: "send_bluesky_dm".to_string(),
            description: "Send a direct message to a Bluesky user (creates conversation if needed)".to_string(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "recipient_did": {
                        "type": "string",
                        "description": "DID of the recipient (e.g., did:plc:xxx)"
                    },
                    "text": {
                        "type": "string",
                        "description": "The message text"
                    },
                    "facets": {
                        "type": "array",
                        "description": "Rich text facets for mentions, links, and hashtags. If provided, auto-detection is skipped.",
                        "items": {
                            "type": "object",
                            "properties": {
                                "byte_start": { "type": "integer", "description": "Start byte index in the text" },
                                "byte_end": { "type": "integer", "description": "End byte index in the text" },
                                "mention_did": { "type": "string", "description": "DID for mention facet (e.g., did:plc:xxx)" },
                                "link_uri": { "type": "string", "description": "URI for link facet" },
                                "tag": { "type": "string", "description": "Hashtag (without #)" }
                            },
                            "required": ["byte_start", "byte_end"]
                        }
                    }
                },
                "required": ["recipient_did", "text"]
            }),
        },
        ToolDefinition {
            name: "reply_to_dm".to_string(),
            description: "Reply to an existing DM conversation. Use this when you have a convo_id from a received DM.".to_string(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "convo_id": {
                        "type": "string",
                        "description": "The conversation ID to reply to"
                    },
                    "text": {
                        "type": "string",
                        "description": "The message text"
                    },
                    "facets": {
                        "type": "array",
                        "description": "Rich text facets for mentions, links, and hashtags. If provided, auto-detection is skipped.",
                        "items": {
                            "type": "object",
                            "properties": {
                                "byte_start": { "type": "integer", "description": "Start byte index in the text" },
                                "byte_end": { "type": "integer", "description": "End byte index in the text" },
                                "mention_did": { "type": "string", "description": "DID for mention facet (e.g., did:plc:xxx)" },
                                "link_uri": { "type": "string", "description": "URI for link facet" },
                                "tag": { "type": "string", "description": "Hashtag (without #)" }
                            },
                            "required": ["byte_start", "byte_end"]
                        }
                    }
                },
                "required": ["convo_id", "text"]
            }),
        },
        ToolDefinition {
            name: "like_post".to_string(),
            description: "Like a Bluesky post".to_string(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "uri": {
                        "type": "string",
                        "description": "AT URI of the post to like"
                    },
                    "cid": {
                        "type": "string",
                        "description": "CID of the post to like"
                    }
                },
                "required": ["uri", "cid"]
            }),
        },
        ToolDefinition {
            name: "follow_user".to_string(),
            description: "Follow a Bluesky user".to_string(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "did": {
                        "type": "string",
                        "description": "DID of the user to follow"
                    }
                },
                "required": ["did"]
            }),
        },
        ToolDefinition {
            name: "get_timeline".to_string(),
            description: "Get your Bluesky home timeline".to_string(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "limit": {
                        "type": "integer",
                        "description": "Maximum number of posts to return (default 20, max 100)"
                    }
                }
            }),
        },
        ToolDefinition {
            name: "get_notifications".to_string(),
            description: "Get recent Bluesky notifications".to_string(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "limit": {
                        "type": "integer",
                        "description": "Maximum number of notifications to return (default 20, max 100)"
                    }
                }
            }),
        },
        ToolDefinition {
            name: "search_posts".to_string(),
            description: "Search for posts across Bluesky by keyword, hashtag, author, or date range. Use this to discover conversations about topics you care about. Note: finding a conversation doesn't mean you're welcome in it—consider developing your own heuristics (via identity/rules) for when engagement is appropriate.".to_string(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "query": {
                        "type": "string",
                        "description": "Search query (Lucene syntax supported)"
                    },
                    "author": {
                        "type": "string",
                        "description": "Filter by author handle or DID (optional)"
                    },
                    "since": {
                        "type": "string",
                        "description": "Filter after datetime ISO 8601 (optional)"
                    },
                    "until": {
                        "type": "string",
                        "description": "Filter before datetime ISO 8601 (optional)"
                    },
                    "lang": {
                        "type": "string",
                        "description": "Filter by language code (optional)"
                    },
                    "tag": {
                        "type": "array",
                        "items": { "type": "string" },
                        "description": "Filter by hashtags (optional)"
                    },
                    "sort": {
                        "type": "string",
                        "enum": ["top", "latest"],
                        "description": "Sort order: 'top' or 'latest' (default: latest)"
                    },
                    "limit": {
                        "type": "integer",
                        "description": "Maximum results 1-100 (default 25)"
                    },
                    "cursor": {
                        "type": "string",
                        "description": "Pagination cursor (optional)"
                    }
                },
                "required": ["query"]
            }),
        },
        ToolDefinition {
            name: "search_users".to_string(),
            description: "Search for Bluesky users by name, handle, or bio. Use this to discover people working on topics you find interesting. Consider whether to follow, engage, or simply observe.".to_string(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "query": {
                        "type": "string",
                        "description": "Search query (name, handle, bio)"
                    },
                    "limit": {
                        "type": "integer",
                        "description": "Maximum results 1-100 (default 25)"
                    },
                    "cursor": {
                        "type": "string",
                        "description": "Pagination cursor (optional)"
                    }
                },
                "required": ["query"]
            }),
        },
        ToolDefinition {
            name: "get_thread_context".to_string(),
            description: "Get the full context of a Bluesky thread. Returns all posts in the thread tree, list of participants, and your participation metrics (reply count, last reply time, posts since your last reply). Use this before replying to a thread to understand the full conversation.".to_string(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "uri": {
                        "type": "string",
                        "description": "AT URI of any post in the thread (typically the root or the post you're responding to)"
                    },
                    "depth": {
                        "type": "integer",
                        "description": "Maximum reply depth to fetch (default 6, max 1000)"
                    }
                },
                "required": ["uri"]
            }),
        },
        ToolDefinition {
            name: "mute_user".to_string(),
            description: "Mute a Bluesky user. Muted users won't appear in your timeline or notifications, but they can still see your posts and interact with you.".to_string(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "did": {
                        "type": "string",
                        "description": "DID of the user to mute (e.g., did:plc:xxx)"
                    }
                },
                "required": ["did"]
            }),
        },
        ToolDefinition {
            name: "unmute_user".to_string(),
            description: "Unmute a previously muted Bluesky user.".to_string(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "did": {
                        "type": "string",
                        "description": "DID of the user to unmute (e.g., did:plc:xxx)"
                    }
                },
                "required": ["did"]
            }),
        },
        ToolDefinition {
            name: "block_user".to_string(),
            description: "Block a Bluesky user. Blocked users cannot see your posts, mention you, or interact with you in any way. This is a stronger action than muting.".to_string(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "did": {
                        "type": "string",
                        "description": "DID of the user to block (e.g., did:plc:xxx)"
                    }
                },
                "required": ["did"]
            }),
        },
        ToolDefinition {
            name: "unblock_user".to_string(),
            description: "Unblock a previously blocked Bluesky user.".to_string(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "block_uri": {
                        "type": "string",
                        "description": "AT URI of the block record (returned when blocking)"
                    }
                },
                "required": ["block_uri"]
            }),
        },
        ToolDefinition {
            name: "mute_thread".to_string(),
            description: "Mute a Bluesky thread. Muted threads won't generate notifications for new replies.".to_string(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "root_uri": {
                        "type": "string",
                        "description": "AT URI of the thread root post"
                    }
                },
                "required": ["root_uri"]
            }),
        },
        ToolDefinition {
            name: "unmute_thread".to_string(),
            description: "Unmute a previously muted Bluesky thread.".to_string(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "root_uri": {
                        "type": "string",
                        "description": "AT URI of the thread root post"
                    }
                },
                "required": ["root_uri"]
            }),
        },
        ToolDefinition {
            name: "delete_post".to_string(),
            description: "Delete a Bluesky post or reply. This action is irreversible.".to_string(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "post_uri": {
                        "type": "string",
                        "description": "AT URI of the post to delete (e.g., at://did:plc:xxx/app.bsky.feed.post/rkey)"
                    }
                },
                "required": ["post_uri"]
            }),
        },
    ]
}

/// Get all Bluesky tools with their permission metadata.
/// All Bluesky tools are allowed for the autonomous agent.
pub fn tools() -> Vec<ToolMeta> {
    definitions().into_iter().map(ToolMeta::allowed).collect()
}

pub async fn post_to_bluesky(
    state: &ToolState,
    arguments: &HashMap<String, Value>,
) -> CallToolResult {
    let text = match arguments.get("text").and_then(|v| v.as_str()) {
        Some(t) => t,
        None => return CallToolResult::error("Missing required parameter: text"),
    };

    let facets = parse_facets(arguments);

    let images = match parse_images(arguments) {
        Ok(imgs) => imgs,
        Err(e) => return CallToolResult::error(e),
    };

    let client = match &state.bluesky {
        Some(c) => c,
        None => return CallToolResult::error("Bluesky client not configured"),
    };

    // Use the appropriate method based on whether we have images
    let result = if images.is_empty() {
        client.post(text, facets).await
    } else {
        client.post_with_images(text, images, facets).await
    };

    match result {
        Ok(post_ref) => CallToolResult::success(
            json!({
                "uri": post_ref.uri,
                "cid": post_ref.cid
            })
            .to_string(),
        ),
        Err(e) => CallToolResult::error(format!("Failed to post: {}", e)),
    }
}

pub async fn reply_to_bluesky(
    state: &ToolState,
    arguments: &HashMap<String, Value>,
) -> CallToolResult {
    let text = match arguments.get("text").and_then(|v| v.as_str()) {
        Some(t) => t,
        None => return CallToolResult::error("Missing required parameter: text"),
    };

    let parent_uri = match arguments.get("parent_uri").and_then(|v| v.as_str()) {
        Some(u) => u,
        None => return CallToolResult::error("Missing required parameter: parent_uri"),
    };

    let parent_cid = match arguments.get("parent_cid").and_then(|v| v.as_str()) {
        Some(c) => c,
        None => return CallToolResult::error("Missing required parameter: parent_cid"),
    };

    let root_uri = match arguments.get("root_uri").and_then(|v| v.as_str()) {
        Some(u) => u,
        None => return CallToolResult::error("Missing required parameter: root_uri"),
    };

    let root_cid = match arguments.get("root_cid").and_then(|v| v.as_str()) {
        Some(c) => c,
        None => return CallToolResult::error("Missing required parameter: root_cid"),
    };

    let facets = parse_facets(arguments);

    let images = match parse_images(arguments) {
        Ok(imgs) => imgs,
        Err(e) => return CallToolResult::error(e),
    };

    let client = match &state.bluesky {
        Some(c) => c,
        None => return CallToolResult::error("Bluesky client not configured"),
    };

    let parent = PostRef {
        uri: parent_uri.to_string(),
        cid: parent_cid.to_string(),
    };

    let root = PostRef {
        uri: root_uri.to_string(),
        cid: root_cid.to_string(),
    };

    // Use the appropriate method based on whether we have images
    let result = if images.is_empty() {
        client.reply(text, &parent, &root, facets).await
    } else {
        client
            .reply_with_images(text, &parent, &root, images, facets)
            .await
    };

    match result {
        Ok(post_ref) => CallToolResult::success(
            json!({
                "uri": post_ref.uri,
                "cid": post_ref.cid
            })
            .to_string(),
        ),
        Err(e) => CallToolResult::error(format!("Failed to reply: {}", e)),
    }
}

pub async fn send_bluesky_dm(
    state: &ToolState,
    arguments: &HashMap<String, Value>,
) -> CallToolResult {
    let recipient_did = match arguments.get("recipient_did").and_then(|v| v.as_str()) {
        Some(d) => d,
        None => return CallToolResult::error("Missing required parameter: recipient_did"),
    };

    let text = match arguments.get("text").and_then(|v| v.as_str()) {
        Some(t) => t,
        None => return CallToolResult::error("Missing required parameter: text"),
    };

    let facets = parse_facets(arguments);

    let client = match &state.bluesky {
        Some(c) => c,
        None => return CallToolResult::error("Bluesky client not configured"),
    };

    match client.send_dm(recipient_did, text, facets).await {
        Ok(message_id) => CallToolResult::success(
            json!({
                "message_id": message_id
            })
            .to_string(),
        ),
        Err(e) => CallToolResult::error(format!("Failed to send DM: {}", e)),
    }
}

pub async fn reply_to_dm(state: &ToolState, arguments: &HashMap<String, Value>) -> CallToolResult {
    let convo_id = match arguments.get("convo_id").and_then(|v| v.as_str()) {
        Some(c) => c,
        None => return CallToolResult::error("Missing required parameter: convo_id"),
    };

    let text = match arguments.get("text").and_then(|v| v.as_str()) {
        Some(t) => t,
        None => return CallToolResult::error("Missing required parameter: text"),
    };

    let facets = parse_facets(arguments);

    let client = match &state.bluesky {
        Some(c) => c,
        None => return CallToolResult::error("Bluesky client not configured"),
    };

    match client.send_dm_to_convo(convo_id, text, facets).await {
        Ok(message_id) => CallToolResult::success(
            json!({
                "message_id": message_id
            })
            .to_string(),
        ),
        Err(e) => CallToolResult::error(format!("Failed to reply to DM: {}", e)),
    }
}

pub async fn like_post(state: &ToolState, arguments: &HashMap<String, Value>) -> CallToolResult {
    let uri = match arguments.get("uri").and_then(|v| v.as_str()) {
        Some(u) => u,
        None => return CallToolResult::error("Missing required parameter: uri"),
    };

    let cid = match arguments.get("cid").and_then(|v| v.as_str()) {
        Some(c) => c,
        None => return CallToolResult::error("Missing required parameter: cid"),
    };

    let client = match &state.bluesky {
        Some(c) => c,
        None => return CallToolResult::error("Bluesky client not configured"),
    };

    match client.like(uri, cid).await {
        Ok(like_uri) => CallToolResult::success(
            json!({
                "like_uri": like_uri
            })
            .to_string(),
        ),
        Err(e) => CallToolResult::error(format!("Failed to like: {}", e)),
    }
}

pub async fn follow_user(state: &ToolState, arguments: &HashMap<String, Value>) -> CallToolResult {
    let did = match arguments.get("did").and_then(|v| v.as_str()) {
        Some(d) => d,
        None => return CallToolResult::error("Missing required parameter: did"),
    };

    let client = match &state.bluesky {
        Some(c) => c,
        None => return CallToolResult::error("Bluesky client not configured"),
    };

    match client.follow(did).await {
        Ok(follow_uri) => CallToolResult::success(
            json!({
                "follow_uri": follow_uri
            })
            .to_string(),
        ),
        Err(e) => CallToolResult::error(format!("Failed to follow: {}", e)),
    }
}

pub async fn get_timeline(state: &ToolState, arguments: &HashMap<String, Value>) -> CallToolResult {
    let limit = arguments
        .get("limit")
        .and_then(|v| v.as_u64())
        .map(|l| l.min(100) as u8);

    let client = match &state.bluesky {
        Some(c) => c,
        None => return CallToolResult::error("Bluesky client not configured"),
    };

    match client.get_timeline(limit).await {
        Ok(posts) => {
            let result: Vec<Value> = posts
                .into_iter()
                .map(|p| {
                    json!({
                        "uri": p.uri,
                        "cid": p.cid,
                        "author_did": p.author_did,
                        "author_handle": p.author_handle,
                        "author_name": p.author_name,
                        "text": p.text,
                        "created_at": p.created_at,
                        "like_count": p.like_count,
                        "repost_count": p.repost_count,
                        "reply_count": p.reply_count
                    })
                })
                .collect();
            CallToolResult::success(serde_json::to_string(&result).unwrap_or_default())
        }
        Err(e) => CallToolResult::error(format!("Failed to get timeline: {}", e)),
    }
}

pub async fn get_notifications(
    state: &mut ToolState,
    arguments: &HashMap<String, Value>,
) -> CallToolResult {
    let limit = arguments
        .get("limit")
        .and_then(|v| v.as_u64())
        .map(|l| l.min(100) as u8);

    let client = match &mut state.bluesky {
        Some(c) => c,
        None => return CallToolResult::error("Bluesky client not configured"),
    };

    match client.get_notifications(limit).await {
        Ok(notifications) => {
            let result: Vec<Value> = notifications
                .into_iter()
                .map(|n| {
                    json!({
                        "reason": n.reason,
                        "author_did": n.author_did,
                        "author_handle": n.author_handle,
                        "text": n.text,
                        "uri": n.uri,
                        "cid": n.cid,
                        "parent": n.parent.map(|p| json!({"uri": p.uri, "cid": p.cid})),
                        "root": n.root.map(|r| json!({"uri": r.uri, "cid": r.cid}))
                    })
                })
                .collect();
            CallToolResult::success(serde_json::to_string(&result).unwrap_or_default())
        }
        Err(e) => CallToolResult::error(format!("Failed to get notifications: {}", e)),
    }
}

pub async fn search_posts(state: &ToolState, arguments: &HashMap<String, Value>) -> CallToolResult {
    let query = match arguments.get("query").and_then(|v| v.as_str()) {
        Some(q) => q,
        None => return CallToolResult::error("Missing required parameter: query"),
    };

    let author = arguments.get("author").and_then(|v| v.as_str());
    let since = arguments.get("since").and_then(|v| v.as_str());
    let until = arguments.get("until").and_then(|v| v.as_str());
    let lang = arguments.get("lang").and_then(|v| v.as_str());
    let tag = arguments.get("tag").and_then(|v| {
        v.as_array().map(|arr| {
            arr.iter()
                .filter_map(|t| t.as_str().map(String::from))
                .collect()
        })
    });
    let sort = arguments.get("sort").and_then(|v| v.as_str());
    let limit = arguments
        .get("limit")
        .and_then(|v| v.as_u64())
        .map(|l| l.min(100) as u8);
    let cursor = arguments.get("cursor").and_then(|v| v.as_str());

    let client = match &state.bluesky {
        Some(c) => c,
        None => return CallToolResult::error("Bluesky client not configured"),
    };

    match client
        .search_posts(query, author, since, until, lang, tag, sort, limit, cursor)
        .await
    {
        Ok((posts, next_cursor)) => {
            let result: Vec<Value> = posts
                .into_iter()
                .map(|p| {
                    json!({
                        "uri": p.uri,
                        "cid": p.cid,
                        "author_did": p.author_did,
                        "author_handle": p.author_handle,
                        "author_name": p.author_name,
                        "text": p.text,
                        "created_at": p.created_at,
                        "like_count": p.like_count,
                        "repost_count": p.repost_count,
                        "reply_count": p.reply_count
                    })
                })
                .collect();
            let response = json!({
                "posts": result,
                "cursor": next_cursor
            });
            CallToolResult::success(serde_json::to_string(&response).unwrap_or_default())
        }
        Err(e) => CallToolResult::error(format!("Failed to search posts: {}", e)),
    }
}

pub async fn search_users(state: &ToolState, arguments: &HashMap<String, Value>) -> CallToolResult {
    let query = match arguments.get("query").and_then(|v| v.as_str()) {
        Some(q) => q,
        None => return CallToolResult::error("Missing required parameter: query"),
    };

    let limit = arguments
        .get("limit")
        .and_then(|v| v.as_u64())
        .map(|l| l.min(100) as u8);
    let cursor = arguments.get("cursor").and_then(|v| v.as_str());

    let client = match &state.bluesky {
        Some(c) => c,
        None => return CallToolResult::error("Bluesky client not configured"),
    };

    match client.search_users(query, limit, cursor).await {
        Ok((users, next_cursor)) => {
            let result: Vec<Value> = users
                .into_iter()
                .map(|u| {
                    json!({
                        "did": u.did,
                        "handle": u.handle,
                        "display_name": u.display_name,
                        "description": u.description,
                        "avatar": u.avatar
                    })
                })
                .collect();
            let response = json!({
                "users": result,
                "cursor": next_cursor
            });
            CallToolResult::success(serde_json::to_string(&response).unwrap_or_default())
        }
        Err(e) => CallToolResult::error(format!("Failed to search users: {}", e)),
    }
}

pub async fn get_thread_context(
    state: &ToolState,
    arguments: &HashMap<String, Value>,
) -> CallToolResult {
    let uri = match arguments.get("uri").and_then(|v| v.as_str()) {
        Some(u) => u,
        None => return CallToolResult::error("Missing required parameter: uri"),
    };

    let depth = arguments
        .get("depth")
        .and_then(|v| v.as_u64())
        .map(|d| d.min(1000) as u16);

    let client = match &state.bluesky {
        Some(c) => c,
        None => return CallToolResult::error("Bluesky client not configured"),
    };

    match client.get_post_thread(uri, depth).await {
        Ok(context) => {
            let posts: Vec<Value> = context
                .posts
                .iter()
                .map(|p| {
                    json!({
                        "uri": p.uri,
                        "cid": p.cid,
                        "author_did": p.author_did,
                        "author_handle": p.author_handle,
                        "text": p.text,
                        "created_at": p.created_at,
                        "reply_count": p.reply_count,
                        "parent_uri": p.parent_uri,
                        "depth": p.depth
                    })
                })
                .collect();

            let response = json!({
                "root": {
                    "uri": context.root.uri,
                    "cid": context.root.cid,
                    "author_did": context.root.author_did,
                    "author_handle": context.root.author_handle,
                    "text": context.root.text,
                    "created_at": context.root.created_at,
                    "reply_count": context.root.reply_count
                },
                "posts": posts,
                "participants": context.participants,
                "total_replies": context.total_replies,
                "my_reply_count": context.my_reply_count,
                "my_last_reply_at": context.my_last_reply_at,
                "posts_since_my_last_reply": context.posts_since_my_last_reply
            });
            CallToolResult::success(serde_json::to_string(&response).unwrap_or_default())
        }
        Err(e) => CallToolResult::error(format!("Failed to get thread context: {}", e)),
    }
}

pub async fn mute_user(state: &ToolState, arguments: &HashMap<String, Value>) -> CallToolResult {
    let did = match arguments.get("did").and_then(|v| v.as_str()) {
        Some(d) => d,
        None => return CallToolResult::error("Missing required parameter: did"),
    };

    let client = match &state.bluesky {
        Some(c) => c,
        None => return CallToolResult::error("Bluesky client not configured"),
    };

    match client.mute(did).await {
        Ok(()) => CallToolResult::success(
            json!({
                "muted": did
            })
            .to_string(),
        ),
        Err(e) => CallToolResult::error(format!("Failed to mute user: {}", e)),
    }
}

pub async fn unmute_user(state: &ToolState, arguments: &HashMap<String, Value>) -> CallToolResult {
    let did = match arguments.get("did").and_then(|v| v.as_str()) {
        Some(d) => d,
        None => return CallToolResult::error("Missing required parameter: did"),
    };

    let client = match &state.bluesky {
        Some(c) => c,
        None => return CallToolResult::error("Bluesky client not configured"),
    };

    match client.unmute(did).await {
        Ok(()) => CallToolResult::success(
            json!({
                "unmuted": did
            })
            .to_string(),
        ),
        Err(e) => CallToolResult::error(format!("Failed to unmute user: {}", e)),
    }
}

pub async fn block_user(state: &ToolState, arguments: &HashMap<String, Value>) -> CallToolResult {
    let did = match arguments.get("did").and_then(|v| v.as_str()) {
        Some(d) => d,
        None => return CallToolResult::error("Missing required parameter: did"),
    };

    let client = match &state.bluesky {
        Some(c) => c,
        None => return CallToolResult::error("Bluesky client not configured"),
    };

    match client.block(did).await {
        Ok(block_uri) => CallToolResult::success(
            json!({
                "blocked": did,
                "block_uri": block_uri
            })
            .to_string(),
        ),
        Err(e) => CallToolResult::error(format!("Failed to block user: {}", e)),
    }
}

pub async fn unblock_user(state: &ToolState, arguments: &HashMap<String, Value>) -> CallToolResult {
    let block_uri = match arguments.get("block_uri").and_then(|v| v.as_str()) {
        Some(u) => u,
        None => return CallToolResult::error("Missing required parameter: block_uri"),
    };

    let client = match &state.bluesky {
        Some(c) => c,
        None => return CallToolResult::error("Bluesky client not configured"),
    };

    match client.unblock(block_uri).await {
        Ok(()) => CallToolResult::success(
            json!({
                "unblocked": block_uri
            })
            .to_string(),
        ),
        Err(e) => CallToolResult::error(format!("Failed to unblock user: {}", e)),
    }
}

pub async fn mute_thread(state: &ToolState, arguments: &HashMap<String, Value>) -> CallToolResult {
    let root_uri = match arguments.get("root_uri").and_then(|v| v.as_str()) {
        Some(u) => u,
        None => return CallToolResult::error("Missing required parameter: root_uri"),
    };

    let client = match &state.bluesky {
        Some(c) => c,
        None => return CallToolResult::error("Bluesky client not configured"),
    };

    match client.mute_thread(root_uri).await {
        Ok(()) => CallToolResult::success(
            json!({
                "muted_thread": root_uri
            })
            .to_string(),
        ),
        Err(e) => CallToolResult::error(format!("Failed to mute thread: {}", e)),
    }
}

pub async fn unmute_thread(
    state: &ToolState,
    arguments: &HashMap<String, Value>,
) -> CallToolResult {
    let root_uri = match arguments.get("root_uri").and_then(|v| v.as_str()) {
        Some(u) => u,
        None => return CallToolResult::error("Missing required parameter: root_uri"),
    };

    let client = match &state.bluesky {
        Some(c) => c,
        None => return CallToolResult::error("Bluesky client not configured"),
    };

    match client.unmute_thread(root_uri).await {
        Ok(()) => CallToolResult::success(
            json!({
                "unmuted_thread": root_uri
            })
            .to_string(),
        ),
        Err(e) => CallToolResult::error(format!("Failed to unmute thread: {}", e)),
    }
}

pub async fn delete_post(state: &ToolState, arguments: &HashMap<String, Value>) -> CallToolResult {
    let post_uri = match arguments.get("post_uri").and_then(|v| v.as_str()) {
        Some(u) => u,
        None => return CallToolResult::error("Missing required parameter: post_uri"),
    };

    let client = match &state.bluesky {
        Some(c) => c,
        None => return CallToolResult::error("Bluesky client not configured"),
    };

    match client.delete_post(post_uri).await {
        Ok(()) => CallToolResult::success(
            json!({
                "deleted": true,
                "post_uri": post_uri
            })
            .to_string(),
        ),
        Err(e) => CallToolResult::error(format!("Failed to delete post: {}", e)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    #[test]
    fn mime_from_extension_known_types() {
        assert_eq!(
            mime_from_extension(Path::new("photo.jpg")),
            Some("image/jpeg")
        );
        assert_eq!(
            mime_from_extension(Path::new("photo.jpeg")),
            Some("image/jpeg")
        );
        assert_eq!(
            mime_from_extension(Path::new("image.png")),
            Some("image/png")
        );
        assert_eq!(
            mime_from_extension(Path::new("image.webp")),
            Some("image/webp")
        );
        assert_eq!(
            mime_from_extension(Path::new("anim.gif")),
            Some("image/gif")
        );
    }

    #[test]
    fn mime_from_extension_unknown() {
        assert_eq!(mime_from_extension(Path::new("file.txt")), None);
        assert_eq!(mime_from_extension(Path::new("file")), None);
        assert_eq!(mime_from_extension(Path::new("file.bmp")), None);
    }

    #[test]
    fn resolve_workspace_relative_path() {
        let dir = TempDir::new().unwrap();
        let workspace = dir.path().canonicalize().unwrap();
        fs::write(workspace.join("test.png"), b"fake").unwrap();

        let resolved = resolve_workspace_path("test.png", &workspace).unwrap();
        assert!(resolved.starts_with(&workspace));
        assert!(resolved.ends_with("test.png"));
    }

    #[test]
    fn resolve_workspace_rejects_traversal() {
        let dir = TempDir::new().unwrap();
        let workspace = dir.path().canonicalize().unwrap();

        let result = resolve_workspace_path("../../../etc/passwd", &workspace);
        assert!(result.is_err());
    }

    #[test]
    fn resolve_workspace_rejects_nonexistent() {
        let dir = TempDir::new().unwrap();
        let workspace = dir.path().canonicalize().unwrap();

        let result = resolve_workspace_path("nonexistent.png", &workspace);
        assert!(result.is_err());
    }

    #[test]
    fn parse_images_empty_when_no_key() {
        let args = HashMap::new();
        let result = parse_images(&args).unwrap();
        assert!(result.is_empty());
    }

    #[test]
    fn parse_images_empty_when_not_array() {
        let mut args = HashMap::new();
        args.insert("images".to_string(), serde_json::json!("not an array"));
        let result = parse_images(&args).unwrap();
        assert!(result.is_empty());
    }

    #[test]
    fn parse_images_requires_alt() {
        let mut args = HashMap::new();
        args.insert(
            "images".to_string(),
            serde_json::json!([{"data": "aGVsbG8="}]),
        );
        let result = parse_images(&args);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("missing 'alt'"));
    }

    #[test]
    fn parse_images_base64_with_alt() {
        let mut args = HashMap::new();
        args.insert(
            "images".to_string(),
            serde_json::json!([{
                "alt": "a test image",
                "data": "aGVsbG8=",
                "mime_type": "image/png"
            }]),
        );
        let result = parse_images(&args).unwrap();
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].alt, "a test image");
        assert_eq!(result[0].mime_type, "image/png");
    }
}
