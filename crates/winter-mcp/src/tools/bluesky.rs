//! Bluesky tools for MCP.

use std::collections::HashMap;

use serde_json::{Value, json};

use crate::bluesky::PostRef;
use crate::protocol::{CallToolResult, ToolDefinition};

use super::ToolState;

pub fn definitions() -> Vec<ToolDefinition> {
    vec![
        ToolDefinition {
            name: "post_to_bluesky".to_string(),
            description: "Post a new message to Bluesky".to_string(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "text": {
                        "type": "string",
                        "description": "The text content of the post (max 300 characters)"
                    }
                },
                "required": ["text"]
            }),
        },
        ToolDefinition {
            name: "reply_to_bluesky".to_string(),
            description: "Reply to an existing Bluesky post".to_string(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "text": {
                        "type": "string",
                        "description": "The text content of the reply"
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
    ]
}

pub async fn post_to_bluesky(
    state: &ToolState,
    arguments: &HashMap<String, Value>,
) -> CallToolResult {
    let text = match arguments.get("text").and_then(|v| v.as_str()) {
        Some(t) => t,
        None => return CallToolResult::error("Missing required parameter: text"),
    };

    let client = match &state.bluesky {
        Some(c) => c,
        None => return CallToolResult::error("Bluesky client not configured"),
    };

    match client.post(text).await {
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

    match client.reply(text, &parent, &root).await {
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

    let client = match &state.bluesky {
        Some(c) => c,
        None => return CallToolResult::error("Bluesky client not configured"),
    };

    match client.send_dm(recipient_did, text).await {
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

    let client = match &state.bluesky {
        Some(c) => c,
        None => return CallToolResult::error("Bluesky client not configured"),
    };

    match client.send_dm_to_convo(convo_id, text).await {
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

pub async fn unmute_thread(state: &ToolState, arguments: &HashMap<String, Value>) -> CallToolResult {
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
