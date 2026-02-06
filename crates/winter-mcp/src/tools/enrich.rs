//! Query and enrich tool for chaining datalog queries with Bluesky API enrichment.

use std::collections::{HashMap, HashSet};

use futures_util::stream::{self, StreamExt};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tracing::debug;

use crate::bluesky::BlueskyError;
use crate::protocol::{CallToolResult, ToolDefinition};

use super::{ToolMeta, ToolState};

/// Types of enrichment available.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EnrichmentType {
    /// Get a user's profile (expects DID)
    Profile,
    /// Get a user's recent posts (expects DID)
    AuthorFeed,
    /// Search for posts (expects query string)
    SearchPosts,
    /// Get thread context (expects post URI)
    ThreadContext,
    /// Get a user's followers (expects DID)
    Followers,
    /// Get accounts a user follows (expects DID)
    Follows,
}

impl EnrichmentType {
    fn as_str(&self) -> &'static str {
        match self {
            EnrichmentType::Profile => "profile",
            EnrichmentType::AuthorFeed => "author_feed",
            EnrichmentType::SearchPosts => "search_posts",
            EnrichmentType::ThreadContext => "thread_context",
            EnrichmentType::Followers => "followers",
            EnrichmentType::Follows => "follows",
        }
    }
}

/// How to handle failures for an enrichment.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FailureMode {
    /// Log error, include in result, continue processing
    #[default]
    Continue,
    /// Omit the tuple from results if this enrichment fails
    SkipTuple,
    /// Stop entire operation, return error with partial results
    Halt,
}

/// Specification for a single enrichment.
#[derive(Debug, Clone, Deserialize)]
pub struct EnrichmentSpec {
    /// Which column (0-indexed) to use as the enrichment key
    pub column: usize,
    /// Type of enrichment to perform
    #[serde(rename = "type")]
    pub enrichment_type: EnrichmentType,
    /// Options for the enrichment (e.g., limit)
    #[serde(default)]
    pub options: EnrichmentOptions,
    /// How to handle failures (defaults to global setting)
    pub on_failure: Option<FailureMode>,
    /// Number of retries before applying on_failure (default: 0)
    /// Reserved for future implementation of retry logic.
    #[serde(default)]
    #[allow(dead_code)]
    pub max_retries: u32,
}

/// Options for enrichment calls.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct EnrichmentOptions {
    /// Limit for list results (author_feed, search_posts, followers, follows)
    pub limit: Option<u8>,
    /// Since timestamp for search_posts
    pub since: Option<String>,
    /// Depth for thread_context
    pub depth: Option<u16>,
}

/// Result of a single enrichment call.
#[derive(Debug, Clone, Serialize)]
pub struct EnrichmentResult {
    pub success: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

impl EnrichmentResult {
    fn success(data: Value) -> Self {
        Self {
            success: true,
            data: Some(data),
            error: None,
        }
    }

    fn error(msg: String) -> Self {
        Self {
            success: false,
            data: None,
            error: Some(msg),
        }
    }
}

/// Cache key for deduplication.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct CacheKey {
    column: usize,
    key: String,
    enrichment_type: EnrichmentType,
    options_hash: u64,
}

impl CacheKey {
    fn new(column: usize, key: &str, spec: &EnrichmentSpec) -> Self {
        use std::hash::{Hash, Hasher};
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        spec.options.limit.hash(&mut hasher);
        spec.options.since.hash(&mut hasher);
        spec.options.depth.hash(&mut hasher);

        Self {
            column,
            key: key.to_string(),
            enrichment_type: spec.enrichment_type,
            options_hash: hasher.finish(),
        }
    }
}

pub fn definitions() -> Vec<ToolDefinition> {
    vec![ToolDefinition {
        name: "query_and_enrich".to_string(),
        description: r#"Query facts using datalog, then enrich results with Bluesky API data.

This tool chains a datalog query with API enrichment calls, deduplicating lookups
for efficiency. Each enrichment specifies which result column to use as its input key.

## Enrichment Types

| Type | Input | Description |
|------|-------|-------------|
| profile | DID | Get user profile (handle, bio, counts) |
| author_feed | DID | Get user's recent posts |
| search_posts | query string | Search for posts matching the query |
| thread_context | post URI | Get full thread with participation metrics |
| followers | DID | Get accounts following the user |
| follows | DID | Get accounts the user follows |

## Failure Handling

Global `on_failure` sets the default; each enrichment can override:
- `continue` (default): Include error in result, keep processing
- `skip_tuple`: Omit tuple from results if enrichment fails
- `halt`: Stop immediately, return partial results with error

## Example

```json
{
  "query": "engage_candidate(Person, Topic, _)",
  "enrichments": [
    {"column": 0, "type": "profile"},
    {"column": 0, "type": "author_feed", "options": {"limit": 5}},
    {"column": 1, "type": "search_posts", "options": {"limit": 3}}
  ],
  "max_parallel": 5
}
```

Returns query results with enrichment data attached, deduped by unique keys."#
            .to_string(),
        input_schema: json!({
            "type": "object",
            "properties": {
                "query": {
                    "type": "string",
                    "description": "Datalog query to execute (same as query_facts)"
                },
                "extra_rules": {
                    "type": "string",
                    "description": "Optional ad-hoc rules for the query"
                },
                "extra_facts": {
                    "type": "array",
                    "items": { "type": "string" },
                    "description": "Optional ephemeral facts to inject"
                },
                "enrichments": {
                    "type": "array",
                    "items": {
                        "type": "object",
                        "properties": {
                            "column": {
                                "type": "integer",
                                "description": "Which column (0-indexed) to use as the enrichment key"
                            },
                            "type": {
                                "type": "string",
                                "enum": ["profile", "author_feed", "search_posts", "thread_context", "followers", "follows"],
                                "description": "Type of enrichment"
                            },
                            "options": {
                                "type": "object",
                                "properties": {
                                    "limit": {
                                        "type": "integer",
                                        "description": "Limit for list results (1-100)"
                                    },
                                    "since": {
                                        "type": "string",
                                        "description": "Since timestamp for search_posts"
                                    },
                                    "depth": {
                                        "type": "integer",
                                        "description": "Depth for thread_context"
                                    }
                                }
                            },
                            "on_failure": {
                                "type": "string",
                                "enum": ["continue", "skip_tuple", "halt"],
                                "description": "How to handle failures (overrides global)"
                            },
                            "max_retries": {
                                "type": "integer",
                                "description": "Retries before applying on_failure (default: 0)"
                            }
                        },
                        "required": ["column", "type"]
                    },
                    "description": "Enrichments to apply to query results"
                },
                "max_parallel": {
                    "type": "integer",
                    "description": "Maximum concurrent API calls (default: 5)"
                },
                "on_failure": {
                    "type": "string",
                    "enum": ["continue", "skip_tuple", "halt"],
                    "description": "Default failure handling for enrichments"
                }
            },
            "required": ["query", "enrichments"]
        }),
    }]
}

/// Get all enrich tools with their permission metadata.
pub fn tools() -> Vec<ToolMeta> {
    definitions().into_iter().map(ToolMeta::allowed).collect()
}

/// Execute the query_and_enrich tool.
pub async fn query_and_enrich(
    state: &ToolState,
    arguments: &HashMap<String, Value>,
) -> CallToolResult {
    // Parse query
    let query = match arguments.get("query").and_then(|v| v.as_str()) {
        Some(q) => q.trim(),
        None => return CallToolResult::error("Missing required parameter: query"),
    };

    // Parse enrichments
    let enrichments: Vec<EnrichmentSpec> = match arguments.get("enrichments") {
        Some(v) => match serde_json::from_value(v.clone()) {
            Ok(e) => e,
            Err(e) => {
                return CallToolResult::error(format!("Invalid enrichments: {}", e));
            }
        },
        None => return CallToolResult::error("Missing required parameter: enrichments"),
    };

    if enrichments.is_empty() {
        return CallToolResult::error("enrichments array cannot be empty");
    }

    // Parse options
    let extra_rules = arguments.get("extra_rules").and_then(|v| v.as_str());
    let extra_facts: Option<Vec<String>> = arguments
        .get("extra_facts")
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|v| v.as_str().map(String::from))
                .collect()
        });

    let max_parallel = arguments
        .get("max_parallel")
        .and_then(|v| v.as_u64())
        .map(|n| n.clamp(1, 20) as usize)
        .unwrap_or(5);

    let global_on_failure: FailureMode = arguments
        .get("on_failure")
        .and_then(|v| v.as_str())
        .and_then(|s| match s {
            "continue" => Some(FailureMode::Continue),
            "skip_tuple" => Some(FailureMode::SkipTuple),
            "halt" => Some(FailureMode::Halt),
            _ => None,
        })
        .unwrap_or_default();

    // Check for Bluesky client
    let bluesky = match &state.bluesky {
        Some(b) => b,
        None => return CallToolResult::error("Bluesky client not configured"),
    };

    // Execute the datalog query
    let tuples = if let Some(ref datalog_cache) = state.datalog_cache {
        match datalog_cache
            .execute_query_with_facts(query, extra_rules, extra_facts.as_deref())
            .await
        {
            Ok(t) => t,
            Err(e) => return CallToolResult::error(format!("Query failed: {}", e)),
        }
    } else {
        return CallToolResult::error("No datalog cache available");
    };

    if tuples.is_empty() {
        return CallToolResult::success(
            json!({
                "query": query,
                "count": 0,
                "results": [],
                "cache_stats": {"unique_keys": 0, "total_lookups": 0},
                "errors": []
            })
            .to_string(),
        );
    }

    // Validate column indices
    let max_column = tuples
        .iter()
        .map(|t| t.len())
        .max()
        .unwrap_or(0)
        .saturating_sub(1);
    for spec in &enrichments {
        if spec.column > max_column {
            return CallToolResult::error(format!(
                "Enrichment column {} exceeds tuple size (max: {})",
                spec.column, max_column
            ));
        }
    }

    // Collect unique keys per (column, enrichment_type, options)
    let mut unique_keys: HashSet<CacheKey> = HashSet::new();
    for tuple in &tuples {
        for spec in &enrichments {
            if let Some(val) = tuple.get(spec.column) {
                let cache_key = CacheKey::new(spec.column, val, spec);
                unique_keys.insert(cache_key);
            }
        }
    }

    let total_lookups = tuples.len() * enrichments.len();
    let unique_key_count = unique_keys.len();

    debug!(
        total_lookups = total_lookups,
        unique_keys = unique_key_count,
        "deduplication stats"
    );

    // Perform enrichments in parallel with deduplication
    let mut cache: HashMap<CacheKey, EnrichmentResult> = HashMap::new();
    let mut errors: Vec<Value> = Vec::new();
    let mut halted = false;

    // Convert to vec for parallel processing
    let keys_vec: Vec<(CacheKey, EnrichmentSpec)> = unique_keys
        .into_iter()
        .map(|k| {
            let spec = enrichments
                .iter()
                .find(|s| {
                    s.column == k.column && s.enrichment_type == k.enrichment_type && {
                        let test_key = CacheKey::new(k.column, &k.key, s);
                        test_key.options_hash == k.options_hash
                    }
                })
                .cloned()
                .unwrap();
            (k, spec)
        })
        .collect();

    // Process in parallel batches
    let results: Vec<(CacheKey, EnrichmentResult)> = stream::iter(keys_vec)
        .map(|(cache_key, spec)| {
            let key = cache_key.key.clone();
            async move {
                let result =
                    execute_enrichment(bluesky, &key, &spec.enrichment_type, &spec.options).await;
                (cache_key, result)
            }
        })
        .buffer_unordered(max_parallel)
        .collect()
        .await;

    // Process results and check for halt conditions
    for (cache_key, result) in results {
        let spec = enrichments
            .iter()
            .find(|s| {
                s.column == cache_key.column && s.enrichment_type == cache_key.enrichment_type && {
                    let test_key = CacheKey::new(cache_key.column, &cache_key.key, s);
                    test_key.options_hash == cache_key.options_hash
                }
            })
            .unwrap();

        let on_failure = spec.on_failure.unwrap_or(global_on_failure);

        if !result.success {
            errors.push(json!({
                "key": cache_key.key,
                "column": cache_key.column,
                "type": cache_key.enrichment_type.as_str(),
                "error": result.error
            }));

            if on_failure == FailureMode::Halt {
                halted = true;
                cache.insert(cache_key, result);
                break;
            }
        }

        cache.insert(cache_key, result);
    }

    // Build output results
    let mut output_results: Vec<Value> = Vec::new();

    'tuple_loop: for tuple in &tuples {
        let mut enrichment_data: HashMap<String, HashMap<String, Value>> = HashMap::new();

        for spec in &enrichments {
            let on_failure = spec.on_failure.unwrap_or(global_on_failure);

            if let Some(val) = tuple.get(spec.column) {
                let cache_key = CacheKey::new(spec.column, val, spec);

                if let Some(result) = cache.get(&cache_key) {
                    // Check if we should skip this tuple
                    if !result.success && on_failure == FailureMode::SkipTuple {
                        continue 'tuple_loop;
                    }

                    let col_key = spec.column.to_string();
                    let type_key = spec.enrichment_type.as_str().to_string();

                    enrichment_data
                        .entry(col_key)
                        .or_default()
                        .insert(type_key, serde_json::to_value(result).unwrap());
                }
            }
        }

        output_results.push(json!({
            "tuple": tuple,
            "enrichments": enrichment_data
        }));
    }

    let mut response = json!({
        "query": query,
        "count": output_results.len(),
        "results": output_results,
        "cache_stats": {
            "unique_keys": unique_key_count,
            "total_lookups": total_lookups
        },
        "errors": errors
    });

    if halted {
        response["halted"] = json!(true);
    }

    CallToolResult::success(response.to_string())
}

/// Execute a single enrichment call.
async fn execute_enrichment(
    bluesky: &crate::bluesky::BlueskyClient,
    key: &str,
    enrichment_type: &EnrichmentType,
    options: &EnrichmentOptions,
) -> EnrichmentResult {
    match enrichment_type {
        EnrichmentType::Profile => execute_profile(bluesky, key).await,
        EnrichmentType::AuthorFeed => execute_author_feed(bluesky, key, options.limit).await,
        EnrichmentType::SearchPosts => {
            execute_search_posts(bluesky, key, options.limit, options.since.as_deref()).await
        }
        EnrichmentType::ThreadContext => execute_thread_context(bluesky, key, options.depth).await,
        EnrichmentType::Followers => execute_followers(bluesky, key, options.limit).await,
        EnrichmentType::Follows => execute_follows(bluesky, key, options.limit).await,
    }
}

async fn execute_profile(bluesky: &crate::bluesky::BlueskyClient, did: &str) -> EnrichmentResult {
    if !did.starts_with("did:") {
        return EnrichmentResult::error(format!("profile enrichment expects a DID, got: {}", did));
    }

    match bluesky.get_profile(did).await {
        Ok(profile) => EnrichmentResult::success(json!(profile)),
        Err(e) => EnrichmentResult::error(format_bluesky_error(e)),
    }
}

async fn execute_author_feed(
    bluesky: &crate::bluesky::BlueskyClient,
    did: &str,
    limit: Option<u8>,
) -> EnrichmentResult {
    if !did.starts_with("did:") {
        return EnrichmentResult::error(format!(
            "author_feed enrichment expects a DID, got: {}",
            did
        ));
    }

    match bluesky.get_author_feed(did, limit).await {
        Ok(posts) => EnrichmentResult::success(json!(posts)),
        Err(e) => EnrichmentResult::error(format_bluesky_error(e)),
    }
}

async fn execute_search_posts(
    bluesky: &crate::bluesky::BlueskyClient,
    query: &str,
    limit: Option<u8>,
    since: Option<&str>,
) -> EnrichmentResult {
    match bluesky
        .search_posts(query, None, since, None, None, None, None, limit, None)
        .await
    {
        Ok((posts, cursor)) => EnrichmentResult::success(json!({
            "posts": posts,
            "cursor": cursor
        })),
        Err(e) => EnrichmentResult::error(format_bluesky_error(e)),
    }
}

async fn execute_thread_context(
    bluesky: &crate::bluesky::BlueskyClient,
    uri: &str,
    depth: Option<u16>,
) -> EnrichmentResult {
    if !uri.starts_with("at://") {
        return EnrichmentResult::error(format!(
            "thread_context enrichment expects an AT URI, got: {}",
            uri
        ));
    }

    match bluesky.get_post_thread(uri, depth).await {
        Ok(context) => EnrichmentResult::success(json!(context)),
        Err(e) => EnrichmentResult::error(format_bluesky_error(e)),
    }
}

async fn execute_followers(
    bluesky: &crate::bluesky::BlueskyClient,
    did: &str,
    limit: Option<u8>,
) -> EnrichmentResult {
    if !did.starts_with("did:") {
        return EnrichmentResult::error(format!(
            "followers enrichment expects a DID, got: {}",
            did
        ));
    }

    match bluesky.get_followers(did, limit).await {
        Ok(followers) => EnrichmentResult::success(json!(followers)),
        Err(e) => EnrichmentResult::error(format_bluesky_error(e)),
    }
}

async fn execute_follows(
    bluesky: &crate::bluesky::BlueskyClient,
    did: &str,
    limit: Option<u8>,
) -> EnrichmentResult {
    if !did.starts_with("did:") {
        return EnrichmentResult::error(format!("follows enrichment expects a DID, got: {}", did));
    }

    match bluesky.get_follows(did, limit).await {
        Ok(follows) => EnrichmentResult::success(json!(follows)),
        Err(e) => EnrichmentResult::error(format_bluesky_error(e)),
    }
}

fn format_bluesky_error(e: BlueskyError) -> String {
    match e {
        BlueskyError::RateLimited { endpoint } => {
            format!(
                "rate limited{}",
                endpoint.map(|e| format!(" on {}", e)).unwrap_or_default()
            )
        }
        other => other.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_enrichment_type_serialization() {
        assert_eq!(EnrichmentType::Profile.as_str(), "profile");
        assert_eq!(EnrichmentType::AuthorFeed.as_str(), "author_feed");
        assert_eq!(EnrichmentType::SearchPosts.as_str(), "search_posts");
        assert_eq!(EnrichmentType::ThreadContext.as_str(), "thread_context");
        assert_eq!(EnrichmentType::Followers.as_str(), "followers");
        assert_eq!(EnrichmentType::Follows.as_str(), "follows");
    }

    #[test]
    fn test_failure_mode_default() {
        let mode: FailureMode = Default::default();
        assert_eq!(mode, FailureMode::Continue);
    }

    #[test]
    fn test_enrichment_result_success() {
        let result = EnrichmentResult::success(json!({"foo": "bar"}));
        assert!(result.success);
        assert!(result.data.is_some());
        assert!(result.error.is_none());
    }

    #[test]
    fn test_enrichment_result_error() {
        let result = EnrichmentResult::error("something went wrong".to_string());
        assert!(!result.success);
        assert!(result.data.is_none());
        assert_eq!(result.error, Some("something went wrong".to_string()));
    }

    #[test]
    fn test_cache_key_equality() {
        let spec = EnrichmentSpec {
            column: 0,
            enrichment_type: EnrichmentType::Profile,
            options: EnrichmentOptions::default(),
            on_failure: None,
            max_retries: 0,
        };

        let key1 = CacheKey::new(0, "did:plc:abc", &spec);
        let key2 = CacheKey::new(0, "did:plc:abc", &spec);
        let key3 = CacheKey::new(0, "did:plc:xyz", &spec);

        assert_eq!(key1, key2);
        assert_ne!(key1, key3);
    }

    #[test]
    fn test_cache_key_different_options() {
        let spec1 = EnrichmentSpec {
            column: 0,
            enrichment_type: EnrichmentType::AuthorFeed,
            options: EnrichmentOptions {
                limit: Some(5),
                ..Default::default()
            },
            on_failure: None,
            max_retries: 0,
        };

        let spec2 = EnrichmentSpec {
            column: 0,
            enrichment_type: EnrichmentType::AuthorFeed,
            options: EnrichmentOptions {
                limit: Some(10),
                ..Default::default()
            },
            on_failure: None,
            max_retries: 0,
        };

        let key1 = CacheKey::new(0, "did:plc:abc", &spec1);
        let key2 = CacheKey::new(0, "did:plc:abc", &spec2);

        // Different options should produce different keys
        assert_ne!(key1, key2);
    }

    #[test]
    fn test_definitions_not_empty() {
        let defs = definitions();
        assert!(!defs.is_empty());
        assert_eq!(defs[0].name, "query_and_enrich");
    }

    #[test]
    fn test_tools_allowed() {
        let tools = tools();
        assert!(!tools.is_empty());
        for tool in &tools {
            assert!(tool.agent_allowed);
        }
    }

    #[test]
    fn test_enrichment_spec_deserialization() {
        let json = r#"{
            "column": 0,
            "type": "profile"
        }"#;

        let spec: EnrichmentSpec = serde_json::from_str(json).unwrap();
        assert_eq!(spec.column, 0);
        assert_eq!(spec.enrichment_type, EnrichmentType::Profile);
        assert!(spec.on_failure.is_none());
    }

    #[test]
    fn test_enrichment_spec_with_options() {
        let json = r#"{
            "column": 1,
            "type": "author_feed",
            "options": {"limit": 10},
            "on_failure": "skip_tuple"
        }"#;

        let spec: EnrichmentSpec = serde_json::from_str(json).unwrap();
        assert_eq!(spec.column, 1);
        assert_eq!(spec.enrichment_type, EnrichmentType::AuthorFeed);
        assert_eq!(spec.options.limit, Some(10));
        assert_eq!(spec.on_failure, Some(FailureMode::SkipTuple));
    }
}
