//! Web routes.

use std::sync::Arc;

use axum::{
    Form, Router,
    extract::{Path, State},
    response::{Html, IntoResponse, Json, Redirect},
    routing::{get, post},
};
use chrono::Utc;
use serde::Deserialize;
use serde_json::json;
use tokio::sync::{RwLock, broadcast};
use tower_http::services::ServeDir;
use tracing::warn;

use winter_atproto::{
    AtprotoClient, CustomTool, DIRECTIVE_COLLECTION, Directive, DirectiveKind, FACT_COLLECTION,
    FACT_DECLARATION_COLLECTION, Fact, FactDeclArg, FactDeclaration, IDENTITY_COLLECTION,
    IDENTITY_KEY, Identity, JOB_COLLECTION, Job, JobSchedule, JobStatus, NOTE_COLLECTION, Note,
    RULE_COLLECTION, Rule, SECRET_META_COLLECTION, SECRET_META_KEY, SecretMeta, THOUGHT_COLLECTION,
    TOOL_APPROVAL_COLLECTION, TOOL_COLLECTION, Thought, Tid, ToolApproval, ToolApprovalStatus,
    WIKI_ENTRY_COLLECTION, WIKI_LINK_COLLECTION, WikiEntry, WikiLink,
};
use winter_mcp::SecretManager;

use crate::sse::create_sse_stream;
use crate::thought_stream::subscribe_thoughts;

/// Shared state for the web server.
pub struct AppState {
    pub client: AtprotoClient,
    pub thought_tx: broadcast::Sender<String>,
    /// Secret manager for custom tools (optional).
    pub secrets: Option<Arc<RwLock<SecretManager>>>,
}

/// Create the web router.
///
/// If `firehose_url` and `did` are provided, subscribes to real-time thought updates.
pub fn create_router(
    client: AtprotoClient,
    static_dir: Option<&str>,
    firehose_url: Option<String>,
    did: Option<String>,
) -> Router {
    create_router_with_secrets(client, static_dir, firehose_url, did, None)
}

/// Create the web router with optional secret manager.
pub fn create_router_with_secrets(
    client: AtprotoClient,
    static_dir: Option<&str>,
    firehose_url: Option<String>,
    did: Option<String>,
    secrets: Option<SecretManager>,
) -> Router {
    let (thought_tx, _) = broadcast::channel(100);

    let state = Arc::new(AppState {
        client,
        thought_tx: thought_tx.clone(),
        secrets: secrets.map(|s| Arc::new(RwLock::new(s))),
    });

    // Subscribe to firehose for real-time thought updates
    if let (Some(firehose_url), Some(did)) = (firehose_url, did) {
        tokio::spawn(async move {
            subscribe_thoughts(firehose_url, did, thought_tx).await;
        });
    }

    let mut router = Router::new()
        .route("/", get(index))
        .route("/stream", get(stream_page))
        // Facts
        .route("/facts", get(facts_page))
        .route("/facts/new", get(fact_new))
        .route("/facts/{rkey}", get(fact_detail))
        .route("/facts/{rkey}/edit", get(fact_edit))
        .route("/api/facts", post(create_fact))
        .route("/api/facts/{rkey}", post(update_fact))
        .route("/api/facts/{rkey}/delete", post(delete_fact))
        // Rules
        .route("/rules", get(rules_page))
        .route("/rules/new", get(rule_new))
        .route("/rules/{rkey}", get(rule_detail))
        .route("/rules/{rkey}/edit", get(rule_edit))
        .route("/api/rules", post(create_rule))
        .route("/api/rules/{rkey}", post(update_rule))
        .route("/api/rules/{rkey}/delete", post(delete_rule))
        // Jobs
        .route("/jobs", get(jobs_page))
        .route("/jobs/new", get(job_new))
        .route("/jobs/{rkey}", get(job_detail))
        .route("/jobs/{rkey}/edit", get(job_edit))
        .route("/api/jobs", post(create_job))
        .route("/api/jobs/{rkey}", post(update_job))
        .route("/api/jobs/{rkey}/delete", post(delete_job))
        // Notes
        .route("/notes", get(notes_page))
        .route("/notes/new", get(note_new))
        .route("/notes/{rkey}", get(note_detail))
        .route("/notes/{rkey}/edit", get(note_edit))
        .route("/api/notes", post(create_note))
        .route("/api/notes/{rkey}", post(update_note))
        .route("/api/notes/{rkey}/delete", post(delete_note))
        // Wiki
        .route("/wiki", get(wiki_page))
        .route("/wiki/new", get(wiki_new))
        .route("/wiki/{slug_or_rkey}", get(wiki_detail))
        .route("/wiki/{rkey}/edit", get(wiki_edit))
        .route("/api/wiki", post(create_wiki_entry_web))
        .route("/api/wiki/{rkey}", post(update_wiki_entry_web))
        .route("/api/wiki/{rkey}/delete", post(delete_wiki_entry_web))
        // Directives
        .route("/directives", get(directives_page))
        .route("/directives/new", get(directive_new))
        .route("/directives/{rkey}", get(directive_detail))
        .route("/directives/{rkey}/edit", get(directive_edit))
        .route("/api/directives", post(create_directive))
        .route("/api/directives/{rkey}", post(update_directive))
        .route("/api/directives/{rkey}/delete", post(delete_directive))
        // Declarations
        .route("/declarations", get(declarations_page))
        .route("/declarations/new", get(declaration_new))
        .route("/declarations/{rkey}", get(declaration_detail))
        .route("/declarations/{rkey}/edit", get(declaration_edit))
        .route("/api/declarations", post(create_declaration))
        .route("/api/declarations/{rkey}", post(update_declaration))
        .route("/api/declarations/{rkey}/delete", post(delete_declaration))
        // Identity
        .route("/identity", get(identity_page))
        // Tools
        .route("/tools", get(tools_page))
        .route("/tools/{rkey}", get(tool_detail))
        // Secrets
        .route("/secrets", get(secrets_page))
        .route("/api/secrets", post(create_secret))
        .route("/api/secrets/{name}", post(update_secret))
        .route("/api/secrets/{name}/delete", post(delete_secret))
        // Other
        .route("/health", get(health))
        .route("/api/thoughts/sse", get(thoughts_sse))
        .with_state(state);

    // Serve static files if directory provided
    if let Some(dir) = static_dir {
        router = router.nest_service("/static", ServeDir::new(dir));
    }

    router
}

async fn index() -> impl IntoResponse {
    Html(INDEX_HTML)
}

async fn health(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    // Check identity loaded
    let identity_ok = state
        .client
        .get_record::<Identity>(IDENTITY_COLLECTION, IDENTITY_KEY)
        .await
        .is_ok();

    // Count recent thoughts
    let thought_count = state
        .client
        .list_records::<Thought>(THOUGHT_COLLECTION, Some(10), None)
        .await
        .map(|r| r.records.len())
        .unwrap_or(0);

    Json(json!({
        "status": if identity_ok { "ok" } else { "degraded" },
        "identity_loaded": identity_ok,
        "recent_thoughts": thought_count,
        "version": env!("CARGO_PKG_VERSION")
    }))
}

async fn stream_page(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    // Load recent thoughts
    let thoughts = match state
        .client
        .list_records::<Thought>(THOUGHT_COLLECTION, Some(50), None)
        .await
    {
        Ok(r) => r.records,
        Err(e) => {
            warn!(error = %e, "failed to load thoughts for stream page");
            Vec::new()
        }
    };

    let mut thought_html = String::new();
    for item in &thoughts {
        let kind = thought_kind_to_string(&item.value.kind);
        let kind_display = kind.replace('_', " ");
        let content = format_thought_content(&kind, &item.value.content);
        let rel_time = format_relative_time(item.value.created_at);
        let abs_time = item.value.created_at.to_rfc3339();

        let duration_html = item
            .value
            .duration_ms
            .map(|ms| format!(r#"<span class="duration">({}ms)</span>"#, ms))
            .unwrap_or_default();

        let trigger_html = item
            .value
            .trigger
            .as_ref()
            .map(|t| {
                format!(
                    r#"<div class="trigger" data-trigger="{}">{}</div>"#,
                    html_escape(t),
                    html_escape(t)
                )
            })
            .unwrap_or_default();

        let tags_html = if item.value.tags.is_empty() {
            String::new()
        } else {
            let tags: Vec<String> = item
                .value
                .tags
                .iter()
                .map(|t| {
                    format!(
                        r#"<span class="tag" data-tag="{}">{}</span>"#,
                        html_escape(t),
                        html_escape(t)
                    )
                })
                .collect();
            format!(r#"<div class="tags">{}</div>"#, tags.join(""))
        };

        // Escape trigger for data attribute
        let trigger_attr = item
            .value
            .trigger
            .as_ref()
            .map(|t| format!(r#" data-trigger="{}""#, html_escape(t)))
            .unwrap_or_default();

        // Escape tags for data attribute (JSON array)
        let tags_attr = if item.value.tags.is_empty() {
            String::new()
        } else {
            let tags_json = serde_json::to_string(&item.value.tags).unwrap_or_default();
            format!(r#" data-tags="{}""#, html_escape(&tags_json))
        };

        thought_html.push_str(&format!(
            r#"<div class="thought {kind}"{trigger_attr}{tags_attr}>
                <div class="thought-header">
                    <span class="kind">{kind_display}</span>
                    <span class="time" title="{abs_time}">{rel_time}{duration_html}</span>
                </div>
                <div class="content">{content}</div>
                <button class="expand-btn">Expand</button>
                {trigger_html}
                {tags_html}
            </div>"#,
        ));
    }

    Html(STREAM_HTML.replace("<!-- THOUGHTS -->", &thought_html))
}

/// Format thought content based on kind.
fn format_thought_content(kind: &str, content: &str) -> String {
    if kind == "tool_call" {
        format_tool_call_content(content)
    } else {
        html_escape(content)
    }
}

/// Format tool call content with syntax highlighting.
///
/// Handles three formats:
/// 1. JSON format (new): `{"tool":"name","args":{...},"result":{...},"summary":"..."}`
/// 2. Text format (previous): "Called tool_name\nArgs:\n{...}\nResult: ..."
/// 3. Legacy format: "Called tool_name [args] - FAILED"
fn format_tool_call_content(content: &str) -> String {
    // Try parsing as JSON first (new format)
    let trimmed = content.trim();
    if trimmed.starts_with('{') && trimmed.ends_with('}') {
        match serde_json::from_str::<serde_json::Value>(trimmed) {
            Ok(json) => return format_tool_call_json(&json),
            Err(e) => {
                // Log the error for debugging
                tracing::warn!(error = %e, content_len = trimmed.len(), "Failed to parse tool call JSON");
                // Try a simple regex fallback to at least show the tool name
                if let Some(tool_match) = extract_json_field(trimmed, "tool") {
                    return format!(
                        r#"<div class="tool-header"><span class="tool-name">{}</span></div><pre class="tool-json">{}</pre>"#,
                        html_escape(&tool_match),
                        syntax_highlight_json(&html_escape(trimmed))
                    );
                }
            }
        }
    }

    let lines: Vec<&str> = content.lines().collect();
    if lines.is_empty() {
        return html_escape(content);
    }

    let first_line = lines[0];

    // Text format: "Called tool_name" or "Called tool_name - FAILED" on first line
    if first_line.starts_with("Called ") && !first_line.contains('[') {
        let rest = &first_line[7..]; // Skip "Called "
        let (tool_name, failed) = if let Some(stripped) = rest.strip_suffix(" - FAILED") {
            (stripped, true)
        } else {
            (rest, false)
        };

        let mut html = format!(
            r#"<div class="tool-header"><span class="tool-name">{}</span>"#,
            html_escape(tool_name)
        );
        if failed {
            html.push_str(r#" <span class="tool-failed">FAILED</span>"#);
        }
        html.push_str("</div>");

        // Parse sections (Args, Result)
        let mut current_section: Option<&str> = None;
        let mut current_content = String::new();

        for line in lines.iter().skip(1) {
            if *line == "Args:"
                || *line == "Result:"
                || *line == "Error:"
                || line.starts_with("Result: ")
            {
                // Flush previous section
                if let Some(section) = current_section
                    && !current_content.is_empty()
                {
                    html.push_str(&format_tool_section(section, &current_content));
                }
                if let Some(result_content) = line.strip_prefix("Result: ") {
                    // Inline result (summary format)
                    current_section = Some("Result");
                    current_content = result_content.to_string();
                } else {
                    current_section = Some(line.trim_end_matches(':'));
                    current_content.clear();
                }
            } else if current_section.is_some() {
                if !current_content.is_empty() {
                    current_content.push('\n');
                }
                current_content.push_str(line);
            }
        }

        // Flush final section
        if let Some(section) = current_section
            && !current_content.is_empty()
        {
            html.push_str(&format_tool_section(section, &current_content));
        }

        return html;
    }

    // Legacy format: "Called tool_name [args] - FAILED" or "Called tool_name [args]"
    if let Some(rest) = content.strip_prefix("Called ") {
        let parts: Vec<&str> = rest.splitn(2, ' ').collect();
        if let Some(tool_name) = parts.first() {
            let tool_name = tool_name.trim();
            let mut html = format!(
                r#"<span class="tool-name">{}</span>"#,
                html_escape(tool_name)
            );

            if let Some(remainder) = parts.get(1)
                && let (Some(args_start), Some(args_end)) =
                    (remainder.find('['), remainder.rfind(']'))
            {
                let args = &remainder[args_start + 1..args_end];
                if !args.is_empty() {
                    html.push_str(&format!(
                        r#" <span class="tool-args">{}</span>"#,
                        html_escape(args)
                    ));
                }

                let after_args = &remainder[args_end + 1..];
                if after_args.contains("FAILED") {
                    html.push_str(r#" <span class="tool-failed">FAILED</span>"#);
                }
            }
            return html;
        }
    }

    html_escape(content)
}

/// Format tool call from JSON structure.
fn format_tool_call_json(json: &serde_json::Value) -> String {
    let tool_name = json
        .get("tool")
        .and_then(|v| v.as_str())
        .unwrap_or("unknown");
    let failed = json
        .get("failed")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);

    let mut html = String::from(r#"<div class="tool-header">"#);

    // Link at the top (if present)
    if let Some(link) = json.get("link").and_then(|v| v.as_str()) {
        html.push_str(&format!(
            r#"<a href="{}" class="tool-link-btn">View →</a>"#,
            html_escape(link)
        ));
    }

    html.push_str(&format!(
        r#"<span class="tool-name">{}</span>"#,
        html_escape(tool_name)
    ));
    if failed {
        html.push_str(r#" <span class="tool-failed">FAILED</span>"#);
    }
    html.push_str("</div>");

    // Error section (for failed calls)
    if let Some(error) = json.get("error").and_then(|v| v.as_str()) {
        html.push_str(&format!(
            r#"<div class="tool-error">{}</div>"#,
            html_escape(error)
        ));
    }

    // Summary (quick view)
    if let Some(summary) = json.get("summary").and_then(|v| v.as_str()) {
        // Remove the "View: ..." line from summary since we show link separately
        let clean_summary: String = summary
            .lines()
            .filter(|line| !line.starts_with("View:"))
            .collect::<Vec<_>>()
            .join("\n");
        if !clean_summary.is_empty() {
            html.push_str(&format!(
                r#"<div class="tool-summary">{}</div>"#,
                html_escape(&clean_summary)
            ));
        }
    }

    // Args section
    if let Some(args) = json.get("args")
        && let Ok(args_json) = serde_json::to_string_pretty(args)
    {
        html.push_str(&format_tool_section("Args", &args_json));
    }

    // Result section (collapsible, starts closed for large results)
    if let Some(result) = json.get("result")
        && let Ok(result_json) = serde_json::to_string_pretty(result)
    {
        // Use closed details for results to avoid cluttering the UI
        html.push_str(&format!(
            r#"<details class="tool-section"><summary class="tool-section-header">Result</summary><pre class="tool-json">{}</pre></details>"#,
            syntax_highlight_json(&html_escape(&result_json))
        ));
    }

    html
}

/// Extract a simple string field from JSON using regex (fallback when parsing fails).
fn extract_json_field(json_str: &str, field: &str) -> Option<String> {
    use regex::Regex;
    // Match "field":"value" pattern
    let pattern = format!(
        r#""{}"\s*:\s*"([^"\\]*(?:\\.[^"\\]*)*)""#,
        regex::escape(field)
    );
    Regex::new(&pattern)
        .ok()
        .and_then(|re| re.captures(json_str))
        .and_then(|caps| caps.get(1))
        .map(|m| m.as_str().to_string())
}

/// Format a tool section (Args or Result) with JSON syntax highlighting.
fn format_tool_section(section_name: &str, content: &str) -> String {
    if section_name == "Error" {
        return format!(r#"<div class="tool-error">{}</div>"#, html_escape(content));
    }
    format!(
        r#"<details class="tool-section" open><summary class="tool-section-header">{}</summary><pre class="tool-json">{}</pre></details>"#,
        section_name,
        syntax_highlight_json(&html_escape(content))
    )
}

/// Simple JSON syntax highlighting.
fn syntax_highlight_json(escaped_json: &str) -> String {
    use regex::Regex;

    // Highlight strings
    let string_re = Regex::new(r#""([^"\\]*(\\.[^"\\]*)*)""#).unwrap();
    let with_strings =
        string_re.replace_all(escaped_json, r#"<span class="json-string">$0</span>"#);

    // Highlight numbers
    let number_re = Regex::new(r"\b(-?\d+\.?\d*)\b").unwrap();
    let with_numbers =
        number_re.replace_all(&with_strings, r#"<span class="json-number">$1</span>"#);

    // Highlight keywords
    let keyword_re = Regex::new(r"\b(true|false|null)\b").unwrap();
    keyword_re
        .replace_all(&with_numbers, r#"<span class="json-keyword">$1</span>"#)
        .to_string()
}

/// Convert ThoughtKind to snake_case string for CSS classes.
fn thought_kind_to_string(kind: &winter_atproto::ThoughtKind) -> String {
    use winter_atproto::ThoughtKind;
    match kind {
        ThoughtKind::Insight => "insight",
        ThoughtKind::Question => "question",
        ThoughtKind::Plan => "plan",
        ThoughtKind::Reflection => "reflection",
        ThoughtKind::Error => "error",
        ThoughtKind::Response => "response",
        ThoughtKind::ToolCall => "tool_call",
    }
    .to_string()
}

/// Format a timestamp as relative time (e.g., "2 minutes ago").
fn format_relative_time(dt: chrono::DateTime<chrono::Utc>) -> String {
    let now = chrono::Utc::now();
    let diff = now.signed_duration_since(dt);

    let seconds = diff.num_seconds();
    if seconds < 60 {
        return "just now".to_string();
    }

    let minutes = diff.num_minutes();
    if minutes == 1 {
        return "1 minute ago".to_string();
    }
    if minutes < 60 {
        return format!("{} minutes ago", minutes);
    }

    let hours = diff.num_hours();
    if hours == 1 {
        return "1 hour ago".to_string();
    }
    if hours < 24 {
        return format!("{} hours ago", hours);
    }

    let days = diff.num_days();
    if days == 1 {
        return "yesterday".to_string();
    }
    if days < 7 {
        return format!("{} days ago", days);
    }

    // Fall back to date
    dt.format("%b %d").to_string()
}

/// Format a future timestamp as relative time (e.g., "in 5 minutes").
fn format_relative_future_time(dt: chrono::DateTime<chrono::Utc>) -> String {
    let now = chrono::Utc::now();
    let diff = dt.signed_duration_since(now);

    let seconds = diff.num_seconds();
    if seconds < 0 {
        return "now".to_string(); // Already past
    }
    if seconds < 60 {
        return "in a few seconds".to_string();
    }

    let minutes = diff.num_minutes();
    if minutes == 1 {
        return "in 1 minute".to_string();
    }
    if minutes < 60 {
        return format!("in {} minutes", minutes);
    }

    let hours = diff.num_hours();
    if hours == 1 {
        return "in 1 hour".to_string();
    }
    if hours < 24 {
        return format!("in {} hours", hours);
    }

    let days = diff.num_days();
    if days == 1 {
        return "tomorrow".to_string();
    }
    if days < 7 {
        return format!("in {} days", days);
    }

    // Fall back to date
    dt.format("%b %d").to_string()
}

async fn facts_page(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    let facts = match state.client.list_all_records::<Fact>(FACT_COLLECTION).await {
        Ok(f) => f,
        Err(e) => {
            warn!(error = %e, "failed to load facts for facts page");
            Vec::new()
        }
    };

    let mut facts_html = String::new();
    for item in &facts {
        let rkey = item.uri.split('/').next_back().unwrap_or("");
        facts_html.push_str(&format!(
            r#"<tr onclick="window.location='/facts/{rkey}'" style="cursor:pointer">
                <td><a href="/facts/{rkey}"><code>{}</code></a></td>
                <td>{}</td>
                <td>{}</td>
                <td>{:.2}</td>
                <td>{}</td>
            </tr>"#,
            html_escape(rkey),
            html_escape(&item.value.predicate),
            html_escape(&item.value.args.join(", ")),
            item.value.confidence.unwrap_or(1.0),
            item.value.tags.join(", ")
        ));
    }

    Html(
        FACTS_HTML
            .replace("<!-- FACTS -->", &facts_html)
            .replace("<!-- COUNT -->", &facts.len().to_string()),
    )
}

async fn fact_detail(
    State(state): State<Arc<AppState>>,
    Path(rkey): Path<String>,
) -> impl IntoResponse {
    let fact = match state
        .client
        .get_record::<Fact>(FACT_COLLECTION, &rkey)
        .await
    {
        Ok(f) => f.value,
        Err(_) => return Html(FACT_NOT_FOUND_HTML.to_string()),
    };

    let source_html = fact
        .source
        .as_ref()
        .map(|s| {
            format!(
                "<p><strong>Source:</strong> <code>{}</code></p>",
                html_escape(s)
            )
        })
        .unwrap_or_default();

    let tags_html = if fact.tags.is_empty() {
        String::new()
    } else {
        format!("<p><strong>Tags:</strong> {}</p>", fact.tags.join(", "))
    };

    Html(
        FACT_DETAIL_HTML
            .replace("<!-- RKEY -->", &rkey)
            .replace("<!-- PREDICATE -->", &html_escape(&fact.predicate))
            .replace("<!-- ARGS -->", &html_escape(&fact.args.join(", ")))
            .replace(
                "<!-- CONFIDENCE -->",
                &format!("{:.2}", fact.confidence.unwrap_or(1.0)),
            )
            .replace("<!-- SOURCE -->", &source_html)
            .replace("<!-- TAGS -->", &tags_html)
            .replace(
                "<!-- CREATED_AT -->",
                &fact.created_at.format("%Y-%m-%d %H:%M UTC").to_string(),
            ),
    )
}

async fn fact_new() -> impl IntoResponse {
    Html(
        FACT_FORM_HTML
            .replace("<!-- TITLE -->", "New Fact")
            .replace("<!-- ACTION -->", "/api/facts")
            .replace("<!-- RKEY -->", "")
            .replace("<!-- PREDICATE -->", "")
            .replace("<!-- ARGS -->", "")
            .replace("<!-- CONFIDENCE -->", "1.0")
            .replace("<!-- SOURCE -->", "")
            .replace("<!-- TAGS -->", ""),
    )
}

async fn fact_edit(
    State(state): State<Arc<AppState>>,
    Path(rkey): Path<String>,
) -> impl IntoResponse {
    let fact = match state
        .client
        .get_record::<Fact>(FACT_COLLECTION, &rkey)
        .await
    {
        Ok(f) => f.value,
        Err(_) => return Html(FACT_NOT_FOUND_HTML.to_string()),
    };

    Html(
        FACT_FORM_HTML
            .replace("<!-- TITLE -->", "Edit Fact")
            .replace("<!-- ACTION -->", &format!("/api/facts/{}", rkey))
            .replace("<!-- RKEY -->", &rkey)
            .replace("<!-- PREDICATE -->", &html_escape(&fact.predicate))
            .replace("<!-- ARGS -->", &html_escape(&fact.args.join(", ")))
            .replace(
                "<!-- CONFIDENCE -->",
                &format!("{:.2}", fact.confidence.unwrap_or(1.0)),
            )
            .replace(
                "<!-- SOURCE -->",
                &html_escape(fact.source.as_deref().unwrap_or("")),
            )
            .replace("<!-- TAGS -->", &fact.tags.join(", ")),
    )
}

async fn create_fact(
    State(state): State<Arc<AppState>>,
    Form(form): Form<FactForm>,
) -> impl IntoResponse {
    let args: Vec<String> = form
        .args
        .split(',')
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect();

    let fact = Fact {
        predicate: form.predicate,
        args,
        confidence: form.confidence,
        source: form.source.filter(|s| !s.is_empty()),
        supersedes: None,
        tags: parse_comma_separated(&form.tags),
        created_at: Utc::now(),
    };

    let rkey = Tid::now().to_string();
    match state
        .client
        .create_record(FACT_COLLECTION, Some(&rkey), &fact)
        .await
    {
        Ok(_) => Redirect::to(&format!("/facts/{}", rkey)),
        Err(e) => {
            warn!(error = %e, "failed to create fact");
            Redirect::to("/facts")
        }
    }
}

async fn update_fact(
    State(state): State<Arc<AppState>>,
    Path(rkey): Path<String>,
    Form(form): Form<FactForm>,
) -> impl IntoResponse {
    let existing = match state
        .client
        .get_record::<Fact>(FACT_COLLECTION, &rkey)
        .await
    {
        Ok(f) => f.value,
        Err(_) => return Redirect::to("/facts"),
    };

    let args: Vec<String> = form
        .args
        .split(',')
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect();

    let fact = Fact {
        predicate: form.predicate,
        args,
        confidence: form.confidence,
        source: form.source.filter(|s| !s.is_empty()),
        supersedes: existing.supersedes,
        tags: parse_comma_separated(&form.tags),
        created_at: existing.created_at,
    };

    match state.client.put_record(FACT_COLLECTION, &rkey, &fact).await {
        Ok(_) => Redirect::to(&format!("/facts/{}", rkey)),
        Err(e) => {
            warn!(error = %e, "failed to update fact");
            Redirect::to(&format!("/facts/{}", rkey))
        }
    }
}

async fn delete_fact(
    State(state): State<Arc<AppState>>,
    Path(rkey): Path<String>,
) -> impl IntoResponse {
    let _ = state.client.delete_record(FACT_COLLECTION, &rkey).await;
    Redirect::to("/facts")
}

async fn identity_page(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    let identity = state
        .client
        .get_record::<Identity>(IDENTITY_COLLECTION, IDENTITY_KEY)
        .await;

    // Load directives
    let directives = match state
        .client
        .list_all_records::<Directive>(DIRECTIVE_COLLECTION)
        .await
    {
        Ok(records) => records
            .into_iter()
            .map(|r| r.value)
            .filter(|d| d.active)
            .collect::<Vec<_>>(),
        Err(e) => {
            warn!(error = %e, "failed to load directives");
            Vec::new()
        }
    };

    // Group directives by kind
    let mut self_concepts: Vec<&Directive> = Vec::new();
    let mut values: Vec<&Directive> = Vec::new();
    let mut interests: Vec<&Directive> = Vec::new();
    let mut beliefs: Vec<&Directive> = Vec::new();
    let mut guidelines: Vec<&Directive> = Vec::new();
    let mut boundaries: Vec<&Directive> = Vec::new();
    let mut aspirations: Vec<&Directive> = Vec::new();

    for d in &directives {
        match d.kind {
            DirectiveKind::SelfConcept => self_concepts.push(d),
            DirectiveKind::Value => values.push(d),
            DirectiveKind::Interest => interests.push(d),
            DirectiveKind::Belief => beliefs.push(d),
            DirectiveKind::Guideline => guidelines.push(d),
            DirectiveKind::Boundary => boundaries.push(d),
            DirectiveKind::Aspiration => aspirations.push(d),
        }
    }

    // Build HTML for self-concept (prose)
    let self_description_html = if self_concepts.is_empty() {
        "(No self-concept directives)".to_string()
    } else {
        self_concepts
            .iter()
            .map(|d| html_escape(&d.content))
            .collect::<Vec<_>>()
            .join("\n\n")
    };

    // Build HTML for values
    let values_html: String = values
        .iter()
        .map(|d| {
            format!(
                "<li>{}</li>",
                html_escape(d.summary.as_ref().unwrap_or(&d.content))
            )
        })
        .collect();

    // Build HTML for interests
    let interests_html: String = interests
        .iter()
        .map(|d| {
            format!(
                "<li>{}</li>",
                html_escape(d.summary.as_ref().unwrap_or(&d.content))
            )
        })
        .collect();

    match identity {
        Ok(record) => {
            let id = &record.value;
            Html(
                IDENTITY_HTML
                    .replace("<!-- SELF_DESCRIPTION -->", &self_description_html)
                    .replace("<!-- VALUES -->", &values_html)
                    .replace("<!-- INTERESTS -->", &interests_html)
                    .replace("<!-- CREATED -->", &id.created_at.to_rfc3339())
                    .replace("<!-- UPDATED -->", &id.last_updated.to_rfc3339()),
            )
        }
        Err(_) => Html(IDENTITY_NOT_FOUND_HTML.to_string()),
    }
}

async fn jobs_page(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    let jobs = match state.client.list_all_records::<Job>(JOB_COLLECTION).await {
        Ok(j) => j,
        Err(e) => {
            warn!(error = %e, "failed to load jobs for jobs page");
            Vec::new()
        }
    };

    let mut jobs_html = String::new();
    for item in &jobs {
        let rkey = item.uri.split('/').next_back().unwrap_or("");
        let schedule = match &item.value.schedule {
            JobSchedule::Once { at } => format!("once at {}", at.to_rfc3339()),
            JobSchedule::Interval { seconds } => format!("every {}s", seconds),
        };
        let status = format!("{:?}", item.value.status).to_lowercase();

        jobs_html.push_str(&format!(
            r#"<tr onclick="window.location='/jobs/{rkey}'" style="cursor:pointer">
                <td><a href="/jobs/{rkey}"><code>{}</code></a></td>
                <td>{}</td>
                <td>{}</td>
                <td><span class="status {}">{}</span></td>
                <td>{}</td>
            </tr>"#,
            html_escape(rkey),
            html_escape(&item.value.name),
            schedule,
            status,
            status,
            item.value
                .next_run
                .map(format_relative_future_time)
                .unwrap_or_else(|| "-".to_string())
        ));
    }

    Html(
        JOBS_HTML
            .replace("<!-- JOBS -->", &jobs_html)
            .replace("<!-- COUNT -->", &jobs.len().to_string()),
    )
}

async fn job_detail(
    State(state): State<Arc<AppState>>,
    Path(rkey): Path<String>,
) -> impl IntoResponse {
    let job = match state.client.get_record::<Job>(JOB_COLLECTION, &rkey).await {
        Ok(j) => j.value,
        Err(_) => return Html(JOB_NOT_FOUND_HTML.to_string()),
    };

    let schedule_html = match &job.schedule {
        JobSchedule::Once { at } => format!("Once at {}", at.to_rfc3339()),
        JobSchedule::Interval { seconds } => format!("Every {} seconds", seconds),
    };

    let status = match &job.status {
        JobStatus::Pending => "pending",
        JobStatus::Running => "running",
        JobStatus::Completed => "completed",
        JobStatus::Failed { .. } => "failed",
    };

    let status_detail = match &job.status {
        JobStatus::Failed { error } => {
            format!("<p class=\"error\">Error: {}</p>", html_escape(error))
        }
        _ => String::new(),
    };

    Html(
        JOB_DETAIL_HTML
            .replace("<!-- RKEY -->", &rkey)
            .replace("<!-- NAME -->", &html_escape(&job.name))
            .replace("<!-- INSTRUCTIONS -->", &html_escape(&job.instructions))
            .replace("<!-- SCHEDULE -->", &schedule_html)
            .replace("<!-- STATUS -->", status)
            .replace("<!-- STATUS_DETAIL -->", &status_detail)
            .replace(
                "<!-- LAST_RUN -->",
                &job.last_run
                    .map(|dt| dt.format("%Y-%m-%d %H:%M UTC").to_string())
                    .unwrap_or_else(|| "Never".to_string()),
            )
            .replace(
                "<!-- NEXT_RUN -->",
                &job.next_run
                    .map(format_relative_future_time)
                    .unwrap_or_else(|| "-".to_string()),
            )
            .replace("<!-- FAILURE_COUNT -->", &job.failure_count.to_string())
            .replace(
                "<!-- CREATED_AT -->",
                &job.created_at.format("%Y-%m-%d %H:%M UTC").to_string(),
            ),
    )
}

async fn job_new() -> impl IntoResponse {
    Html(
        JOB_FORM_HTML
            .replace("<!-- TITLE -->", "New Job")
            .replace("<!-- ACTION -->", "/api/jobs")
            .replace("<!-- RKEY -->", "")
            .replace("<!-- NAME -->", "")
            .replace("<!-- INSTRUCTIONS -->", "")
            .replace("<!-- SCHEDULE_ONCE_CHECKED -->", "checked")
            .replace("<!-- SCHEDULE_INTERVAL_CHECKED -->", "")
            .replace("<!-- SCHEDULE_AT -->", "")
            .replace("<!-- SCHEDULE_SECONDS -->", ""),
    )
}

async fn job_edit(
    State(state): State<Arc<AppState>>,
    Path(rkey): Path<String>,
) -> impl IntoResponse {
    let job = match state.client.get_record::<Job>(JOB_COLLECTION, &rkey).await {
        Ok(j) => j.value,
        Err(_) => return Html(JOB_NOT_FOUND_HTML.to_string()),
    };

    let (once_checked, interval_checked, schedule_at, schedule_seconds) = match &job.schedule {
        JobSchedule::Once { at } => ("checked", "", at.to_rfc3339(), String::new()),
        JobSchedule::Interval { seconds } => ("", "checked", String::new(), seconds.to_string()),
    };

    Html(
        JOB_FORM_HTML
            .replace("<!-- TITLE -->", "Edit Job")
            .replace("<!-- ACTION -->", &format!("/api/jobs/{}", rkey))
            .replace("<!-- RKEY -->", &rkey)
            .replace("<!-- NAME -->", &html_escape(&job.name))
            .replace("<!-- INSTRUCTIONS -->", &html_escape(&job.instructions))
            .replace("<!-- SCHEDULE_ONCE_CHECKED -->", once_checked)
            .replace("<!-- SCHEDULE_INTERVAL_CHECKED -->", interval_checked)
            .replace("<!-- SCHEDULE_AT -->", &schedule_at)
            .replace("<!-- SCHEDULE_SECONDS -->", &schedule_seconds),
    )
}

async fn create_job(
    State(state): State<Arc<AppState>>,
    Form(form): Form<JobForm>,
) -> impl IntoResponse {
    let schedule = if form.schedule_type == "interval" {
        JobSchedule::Interval {
            seconds: form.schedule_seconds.unwrap_or(3600),
        }
    } else {
        let at = form
            .schedule_at
            .and_then(|s| chrono::DateTime::parse_from_rfc3339(&s).ok())
            .map(|dt| dt.with_timezone(&Utc))
            .unwrap_or_else(|| Utc::now() + chrono::Duration::hours(1));
        JobSchedule::Once { at }
    };

    let next_run = match &schedule {
        JobSchedule::Once { at } => Some(*at),
        JobSchedule::Interval { seconds } => {
            Some(Utc::now() + chrono::Duration::seconds(*seconds as i64))
        }
    };

    let job = Job {
        name: form.name,
        instructions: form.instructions,
        schedule,
        status: JobStatus::Pending,
        last_run: None,
        next_run,
        failure_count: 0,
        created_at: Utc::now(),
    };

    let rkey = Tid::now().to_string();
    match state
        .client
        .create_record(JOB_COLLECTION, Some(&rkey), &job)
        .await
    {
        Ok(_) => Redirect::to(&format!("/jobs/{}", rkey)),
        Err(e) => {
            warn!(error = %e, "failed to create job");
            Redirect::to("/jobs")
        }
    }
}

async fn update_job(
    State(state): State<Arc<AppState>>,
    Path(rkey): Path<String>,
    Form(form): Form<JobForm>,
) -> impl IntoResponse {
    let existing = match state.client.get_record::<Job>(JOB_COLLECTION, &rkey).await {
        Ok(j) => j.value,
        Err(_) => return Redirect::to("/jobs"),
    };

    let schedule = if form.schedule_type == "interval" {
        JobSchedule::Interval {
            seconds: form.schedule_seconds.unwrap_or(3600),
        }
    } else {
        let at = form
            .schedule_at
            .and_then(|s| chrono::DateTime::parse_from_rfc3339(&s).ok())
            .map(|dt| dt.with_timezone(&Utc))
            .unwrap_or_else(|| Utc::now() + chrono::Duration::hours(1));
        JobSchedule::Once { at }
    };

    let next_run = match &schedule {
        JobSchedule::Once { at } => Some(*at),
        JobSchedule::Interval { seconds } => existing
            .last_run
            .map(|lr| lr + chrono::Duration::seconds(*seconds as i64))
            .or_else(|| Some(Utc::now() + chrono::Duration::seconds(*seconds as i64))),
    };

    let job = Job {
        name: form.name,
        instructions: form.instructions,
        schedule,
        status: existing.status,
        last_run: existing.last_run,
        next_run,
        failure_count: existing.failure_count,
        created_at: existing.created_at,
    };

    match state.client.put_record(JOB_COLLECTION, &rkey, &job).await {
        Ok(_) => Redirect::to(&format!("/jobs/{}", rkey)),
        Err(e) => {
            warn!(error = %e, "failed to update job");
            Redirect::to(&format!("/jobs/{}", rkey))
        }
    }
}

async fn delete_job(
    State(state): State<Arc<AppState>>,
    Path(rkey): Path<String>,
) -> impl IntoResponse {
    let _ = state.client.delete_record(JOB_COLLECTION, &rkey).await;
    Redirect::to("/jobs")
}

// =============================================================================
// Rules CRUD
// =============================================================================

async fn rules_page(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    let rules = match state.client.list_all_records::<Rule>(RULE_COLLECTION).await {
        Ok(r) => r,
        Err(e) => {
            warn!(error = %e, "failed to load rules for rules page");
            Vec::new()
        }
    };

    let mut rules_html = String::new();
    for item in &rules {
        let rkey = item.uri.split('/').next_back().unwrap_or("");
        let enabled_class = if item.value.enabled {
            "enabled"
        } else {
            "disabled"
        };

        rules_html.push_str(&format!(
            r#"<tr onclick="window.location='/rules/{rkey}'" style="cursor:pointer">
                <td><a href="/rules/{rkey}"><code>{}</code></a></td>
                <td>{}</td>
                <td><code>{}</code></td>
                <td><span class="status {enabled_class}">{}</span></td>
                <td>{}</td>
            </tr>"#,
            html_escape(rkey),
            html_escape(&item.value.name),
            html_escape(&item.value.head),
            if item.value.enabled {
                "enabled"
            } else {
                "disabled"
            },
            item.value.priority
        ));
    }

    Html(
        RULES_HTML
            .replace("<!-- RULES -->", &rules_html)
            .replace("<!-- COUNT -->", &rules.len().to_string()),
    )
}

async fn rule_detail(
    State(state): State<Arc<AppState>>,
    Path(rkey): Path<String>,
) -> impl IntoResponse {
    let rule = match state
        .client
        .get_record::<Rule>(RULE_COLLECTION, &rkey)
        .await
    {
        Ok(r) => r.value,
        Err(_) => return Html(RULE_NOT_FOUND_HTML.to_string()),
    };

    let body_html = rule
        .body
        .iter()
        .map(|b| format!("<li><code>{}</code></li>", html_escape(b)))
        .collect::<String>();

    let constraints_html = if rule.constraints.is_empty() {
        String::new()
    } else {
        format!(
            "<h3>Constraints</h3><ul>{}</ul>",
            rule.constraints
                .iter()
                .map(|c| format!("<li><code>{}</code></li>", html_escape(c)))
                .collect::<String>()
        )
    };

    Html(
        RULE_DETAIL_HTML
            .replace("<!-- RKEY -->", &rkey)
            .replace("<!-- NAME -->", &html_escape(&rule.name))
            .replace("<!-- DESCRIPTION -->", &html_escape(&rule.description))
            .replace("<!-- HEAD -->", &html_escape(&rule.head))
            .replace("<!-- BODY -->", &body_html)
            .replace("<!-- CONSTRAINTS -->", &constraints_html)
            .replace(
                "<!-- ENABLED -->",
                if rule.enabled { "enabled" } else { "disabled" },
            )
            .replace("<!-- PRIORITY -->", &rule.priority.to_string())
            .replace(
                "<!-- CREATED_AT -->",
                &rule.created_at.format("%Y-%m-%d %H:%M UTC").to_string(),
            ),
    )
}

async fn rule_new() -> impl IntoResponse {
    Html(
        RULE_FORM_HTML
            .replace("<!-- TITLE -->", "New Rule")
            .replace("<!-- ACTION -->", "/api/rules")
            .replace("<!-- RKEY -->", "")
            .replace("<!-- NAME -->", "")
            .replace("<!-- DESCRIPTION -->", "")
            .replace("<!-- HEAD -->", "")
            .replace("<!-- BODY -->", "")
            .replace("<!-- CONSTRAINTS -->", "")
            .replace("<!-- ENABLED_CHECKED -->", "checked")
            .replace("<!-- PRIORITY -->", "0"),
    )
}

async fn rule_edit(
    State(state): State<Arc<AppState>>,
    Path(rkey): Path<String>,
) -> impl IntoResponse {
    let rule = match state
        .client
        .get_record::<Rule>(RULE_COLLECTION, &rkey)
        .await
    {
        Ok(r) => r.value,
        Err(_) => return Html(RULE_NOT_FOUND_HTML.to_string()),
    };

    Html(
        RULE_FORM_HTML
            .replace("<!-- TITLE -->", "Edit Rule")
            .replace("<!-- ACTION -->", &format!("/api/rules/{}", rkey))
            .replace("<!-- RKEY -->", &rkey)
            .replace("<!-- NAME -->", &html_escape(&rule.name))
            .replace("<!-- DESCRIPTION -->", &html_escape(&rule.description))
            .replace("<!-- HEAD -->", &html_escape(&rule.head))
            .replace("<!-- BODY -->", &rule.body.join("\n"))
            .replace("<!-- CONSTRAINTS -->", &rule.constraints.join("\n"))
            .replace(
                "<!-- ENABLED_CHECKED -->",
                if rule.enabled { "checked" } else { "" },
            )
            .replace("<!-- PRIORITY -->", &rule.priority.to_string()),
    )
}

async fn create_rule(
    State(state): State<Arc<AppState>>,
    Form(form): Form<RuleForm>,
) -> impl IntoResponse {
    let rule = Rule {
        name: form.name,
        description: form.description.unwrap_or_default(),
        head: form.head,
        body: parse_newline_separated(&form.body),
        constraints: form
            .constraints
            .map(|c| parse_newline_separated(&c))
            .unwrap_or_default(),
        enabled: form.enabled.is_some(),
        priority: form.priority.unwrap_or(0),
        created_at: Utc::now(),
    };

    let rkey = Tid::now().to_string();
    match state
        .client
        .create_record(RULE_COLLECTION, Some(&rkey), &rule)
        .await
    {
        Ok(_) => Redirect::to(&format!("/rules/{}", rkey)),
        Err(e) => {
            warn!(error = %e, "failed to create rule");
            Redirect::to("/rules")
        }
    }
}

async fn update_rule(
    State(state): State<Arc<AppState>>,
    Path(rkey): Path<String>,
    Form(form): Form<RuleForm>,
) -> impl IntoResponse {
    let existing = match state
        .client
        .get_record::<Rule>(RULE_COLLECTION, &rkey)
        .await
    {
        Ok(r) => r.value,
        Err(_) => return Redirect::to("/rules"),
    };

    let rule = Rule {
        name: form.name,
        description: form.description.unwrap_or_default(),
        head: form.head,
        body: parse_newline_separated(&form.body),
        constraints: form
            .constraints
            .map(|c| parse_newline_separated(&c))
            .unwrap_or_default(),
        enabled: form.enabled.is_some(),
        priority: form.priority.unwrap_or(0),
        created_at: existing.created_at,
    };

    match state.client.put_record(RULE_COLLECTION, &rkey, &rule).await {
        Ok(_) => Redirect::to(&format!("/rules/{}", rkey)),
        Err(e) => {
            warn!(error = %e, "failed to update rule");
            Redirect::to(&format!("/rules/{}", rkey))
        }
    }
}

async fn delete_rule(
    State(state): State<Arc<AppState>>,
    Path(rkey): Path<String>,
) -> impl IntoResponse {
    let _ = state.client.delete_record(RULE_COLLECTION, &rkey).await;
    Redirect::to("/rules")
}

// =============================================================================
// Directives CRUD
// =============================================================================

async fn directives_page(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    let directives = match state
        .client
        .list_all_records::<Directive>(DIRECTIVE_COLLECTION)
        .await
    {
        Ok(d) => d,
        Err(e) => {
            warn!(error = %e, "failed to load directives for directives page");
            Vec::new()
        }
    };

    let mut directives_html = String::new();
    for item in &directives {
        let rkey = item.uri.split('/').next_back().unwrap_or("");
        let active_class = if item.value.active {
            "active"
        } else {
            "inactive"
        };
        let summary = item
            .value
            .summary
            .as_ref()
            .map(|s| truncate_chars(s, 50))
            .unwrap_or_else(|| truncate_chars(&item.value.content, 50));

        directives_html.push_str(&format!(
            r#"<tr onclick="window.location='/directives/{rkey}'" style="cursor:pointer">
                <td><a href="/directives/{rkey}"><code>{}</code></a></td>
                <td>{}</td>
                <td>{}</td>
                <td><span class="status {active_class}">{}</span></td>
                <td>{}</td>
            </tr>"#,
            html_escape(rkey),
            html_escape(&format!("{}", item.value.kind)),
            html_escape(&summary),
            if item.value.active {
                "active"
            } else {
                "inactive"
            },
            item.value.priority
        ));
    }

    Html(
        DIRECTIVES_HTML
            .replace("<!-- DIRECTIVES -->", &directives_html)
            .replace("<!-- COUNT -->", &directives.len().to_string()),
    )
}

async fn directive_detail(
    State(state): State<Arc<AppState>>,
    Path(rkey): Path<String>,
) -> impl IntoResponse {
    let directive = match state
        .client
        .get_record::<Directive>(DIRECTIVE_COLLECTION, &rkey)
        .await
    {
        Ok(d) => d.value,
        Err(_) => return Html(DIRECTIVE_NOT_FOUND_HTML.to_string()),
    };

    let summary_html = directive
        .summary
        .as_ref()
        .map(|s| format!("<p><strong>Summary:</strong> {}</p>", html_escape(s)))
        .unwrap_or_default();

    let source_html = directive
        .source
        .as_ref()
        .map(|s| format!("<p><strong>Source:</strong> {}</p>", html_escape(s)))
        .unwrap_or_default();

    let tags_html = if directive.tags.is_empty() {
        String::new()
    } else {
        format!(
            "<p><strong>Tags:</strong> {}</p>",
            directive.tags.join(", ")
        )
    };

    Html(
        DIRECTIVE_DETAIL_HTML
            .replace("<!-- RKEY -->", &rkey)
            .replace("<!-- KIND -->", &format!("{}", directive.kind))
            .replace("<!-- CONTENT -->", &html_escape(&directive.content))
            .replace("<!-- SUMMARY -->", &summary_html)
            .replace(
                "<!-- ACTIVE -->",
                if directive.active {
                    "active"
                } else {
                    "inactive"
                },
            )
            .replace(
                "<!-- CONFIDENCE -->",
                &format!("{:.2}", directive.confidence.unwrap_or(1.0)),
            )
            .replace("<!-- SOURCE -->", &source_html)
            .replace("<!-- PRIORITY -->", &directive.priority.to_string())
            .replace("<!-- TAGS -->", &tags_html)
            .replace(
                "<!-- CREATED_AT -->",
                &directive
                    .created_at
                    .format("%Y-%m-%d %H:%M UTC")
                    .to_string(),
            )
            .replace(
                "<!-- UPDATED_AT -->",
                &directive
                    .last_updated
                    .map(|dt| dt.format("%Y-%m-%d %H:%M UTC").to_string())
                    .unwrap_or_else(|| "-".to_string()),
            ),
    )
}

async fn directive_new() -> impl IntoResponse {
    Html(
        DIRECTIVE_FORM_HTML
            .replace("<!-- TITLE -->", "New Directive")
            .replace("<!-- ACTION -->", "/api/directives")
            .replace("<!-- RKEY -->", "")
            .replace("<!-- KIND_VALUE_SELECTED -->", "")
            .replace("<!-- KIND_INTEREST_SELECTED -->", "")
            .replace("<!-- KIND_BELIEF_SELECTED -->", "")
            .replace("<!-- KIND_GUIDELINE_SELECTED -->", "")
            .replace("<!-- KIND_SELF_CONCEPT_SELECTED -->", "")
            .replace("<!-- KIND_BOUNDARY_SELECTED -->", "")
            .replace("<!-- KIND_ASPIRATION_SELECTED -->", "")
            .replace("<!-- CONTENT -->", "")
            .replace("<!-- SUMMARY -->", "")
            .replace("<!-- ACTIVE_CHECKED -->", "checked")
            .replace("<!-- CONFIDENCE -->", "1.0")
            .replace("<!-- SOURCE -->", "")
            .replace("<!-- PRIORITY -->", "0")
            .replace("<!-- TAGS -->", ""),
    )
}

async fn directive_edit(
    State(state): State<Arc<AppState>>,
    Path(rkey): Path<String>,
) -> impl IntoResponse {
    let directive = match state
        .client
        .get_record::<Directive>(DIRECTIVE_COLLECTION, &rkey)
        .await
    {
        Ok(d) => d.value,
        Err(_) => return Html(DIRECTIVE_NOT_FOUND_HTML.to_string()),
    };

    let kind_selections = [
        ("VALUE", DirectiveKind::Value),
        ("INTEREST", DirectiveKind::Interest),
        ("BELIEF", DirectiveKind::Belief),
        ("GUIDELINE", DirectiveKind::Guideline),
        ("SELF_CONCEPT", DirectiveKind::SelfConcept),
        ("BOUNDARY", DirectiveKind::Boundary),
        ("ASPIRATION", DirectiveKind::Aspiration),
    ];

    let mut html = DIRECTIVE_FORM_HTML
        .replace("<!-- TITLE -->", "Edit Directive")
        .replace("<!-- ACTION -->", &format!("/api/directives/{}", rkey))
        .replace("<!-- RKEY -->", &rkey)
        .replace("<!-- CONTENT -->", &html_escape(&directive.content))
        .replace(
            "<!-- SUMMARY -->",
            &html_escape(directive.summary.as_deref().unwrap_or("")),
        )
        .replace(
            "<!-- ACTIVE_CHECKED -->",
            if directive.active { "checked" } else { "" },
        )
        .replace(
            "<!-- CONFIDENCE -->",
            &format!("{:.2}", directive.confidence.unwrap_or(1.0)),
        )
        .replace(
            "<!-- SOURCE -->",
            &html_escape(directive.source.as_deref().unwrap_or("")),
        )
        .replace("<!-- PRIORITY -->", &directive.priority.to_string())
        .replace("<!-- TAGS -->", &directive.tags.join(", "));

    for (name, kind) in &kind_selections {
        let placeholder = format!("<!-- KIND_{}_SELECTED -->", name);
        let selected = if &directive.kind == kind {
            "selected"
        } else {
            ""
        };
        html = html.replace(&placeholder, selected);
    }

    Html(html)
}

async fn create_directive(
    State(state): State<Arc<AppState>>,
    Form(form): Form<DirectiveForm>,
) -> impl IntoResponse {
    let kind = match form.kind.as_str() {
        "value" => DirectiveKind::Value,
        "interest" => DirectiveKind::Interest,
        "belief" => DirectiveKind::Belief,
        "guideline" => DirectiveKind::Guideline,
        "self_concept" => DirectiveKind::SelfConcept,
        "boundary" => DirectiveKind::Boundary,
        "aspiration" => DirectiveKind::Aspiration,
        _ => DirectiveKind::Guideline,
    };

    let now = Utc::now();
    let directive = Directive {
        kind,
        content: form.content,
        summary: form.summary.filter(|s| !s.is_empty()),
        active: form.active.is_some(),
        confidence: form.confidence,
        source: form.source.filter(|s| !s.is_empty()),
        supersedes: None,
        tags: parse_comma_separated(&form.tags),
        priority: form.priority.unwrap_or(0),
        created_at: now,
        last_updated: Some(now),
    };

    let rkey = Tid::now().to_string();
    match state
        .client
        .create_record(DIRECTIVE_COLLECTION, Some(&rkey), &directive)
        .await
    {
        Ok(_) => Redirect::to(&format!("/directives/{}", rkey)),
        Err(e) => {
            warn!(error = %e, "failed to create directive");
            Redirect::to("/directives")
        }
    }
}

async fn update_directive(
    State(state): State<Arc<AppState>>,
    Path(rkey): Path<String>,
    Form(form): Form<DirectiveForm>,
) -> impl IntoResponse {
    let existing = match state
        .client
        .get_record::<Directive>(DIRECTIVE_COLLECTION, &rkey)
        .await
    {
        Ok(d) => d.value,
        Err(_) => return Redirect::to("/directives"),
    };

    let kind = match form.kind.as_str() {
        "value" => DirectiveKind::Value,
        "interest" => DirectiveKind::Interest,
        "belief" => DirectiveKind::Belief,
        "guideline" => DirectiveKind::Guideline,
        "self_concept" => DirectiveKind::SelfConcept,
        "boundary" => DirectiveKind::Boundary,
        "aspiration" => DirectiveKind::Aspiration,
        _ => existing.kind,
    };

    let directive = Directive {
        kind,
        content: form.content,
        summary: form.summary.filter(|s| !s.is_empty()),
        active: form.active.is_some(),
        confidence: form.confidence,
        source: form.source.filter(|s| !s.is_empty()),
        supersedes: existing.supersedes,
        tags: parse_comma_separated(&form.tags),
        priority: form.priority.unwrap_or(0),
        created_at: existing.created_at,
        last_updated: Some(Utc::now()),
    };

    match state
        .client
        .put_record(DIRECTIVE_COLLECTION, &rkey, &directive)
        .await
    {
        Ok(_) => Redirect::to(&format!("/directives/{}", rkey)),
        Err(e) => {
            warn!(error = %e, "failed to update directive");
            Redirect::to(&format!("/directives/{}", rkey))
        }
    }
}

async fn delete_directive(
    State(state): State<Arc<AppState>>,
    Path(rkey): Path<String>,
) -> impl IntoResponse {
    let _ = state
        .client
        .delete_record(DIRECTIVE_COLLECTION, &rkey)
        .await;
    Redirect::to("/directives")
}

// =============================================================================
// Declarations
// =============================================================================

async fn declarations_page(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    let declarations = match state
        .client
        .list_all_records::<FactDeclaration>(FACT_DECLARATION_COLLECTION)
        .await
    {
        Ok(d) => d,
        Err(e) => {
            warn!(error = %e, "failed to load declarations for declarations page");
            Vec::new()
        }
    };

    let mut declarations_html = String::new();
    for item in &declarations {
        let rkey = item.uri.split('/').next_back().unwrap_or("");
        let description_preview = truncate_chars(&item.value.description, 60);

        declarations_html.push_str(&format!(
            r#"<tr onclick="window.location='/declarations/{rkey}'" style="cursor:pointer">
                <td><a href="/declarations/{rkey}"><code>{}</code></a></td>
                <td><code>{}</code></td>
                <td>{}</td>
                <td>{}</td>
                <td>{}</td>
            </tr>"#,
            html_escape(rkey),
            html_escape(&item.value.predicate),
            item.value.args.len(),
            html_escape(&description_preview),
            item.value.tags.join(", ")
        ));
    }

    Html(
        DECLARATIONS_HTML
            .replace("<!-- DECLARATIONS -->", &declarations_html)
            .replace("<!-- COUNT -->", &declarations.len().to_string()),
    )
}

async fn declaration_detail(
    State(state): State<Arc<AppState>>,
    Path(rkey): Path<String>,
) -> impl IntoResponse {
    let declaration = match state
        .client
        .get_record::<FactDeclaration>(FACT_DECLARATION_COLLECTION, &rkey)
        .await
    {
        Ok(d) => d.value,
        Err(_) => return Html(DECLARATION_NOT_FOUND_HTML.to_string()),
    };

    // Build args table rows
    let mut args_html = String::new();
    for (i, arg) in declaration.args.iter().enumerate() {
        args_html.push_str(&format!(
            r#"<tr>
                <td>{}</td>
                <td><code>{}</code></td>
                <td><code>{}</code></td>
                <td>{}</td>
            </tr>"#,
            i,
            html_escape(&arg.name),
            html_escape(&arg.r#type),
            html_escape(arg.description.as_deref().unwrap_or("-"))
        ));
    }

    let tags_html = if declaration.tags.is_empty() {
        String::new()
    } else {
        format!(
            "<p><strong>Tags:</strong> {}</p>",
            declaration.tags.join(", ")
        )
    };

    Html(
        DECLARATION_DETAIL_HTML
            .replace("<!-- RKEY -->", &rkey)
            .replace("<!-- PREDICATE -->", &html_escape(&declaration.predicate))
            .replace("<!-- ARGS_TABLE -->", &args_html)
            .replace("<!-- ARG_COUNT -->", &declaration.args.len().to_string())
            .replace(
                "<!-- DESCRIPTION -->",
                &html_escape(&declaration.description),
            )
            .replace("<!-- TAGS -->", &tags_html)
            .replace(
                "<!-- CREATED_AT -->",
                &declaration
                    .created_at
                    .format("%Y-%m-%d %H:%M UTC")
                    .to_string(),
            )
            .replace(
                "<!-- UPDATED_AT -->",
                &declaration
                    .last_updated
                    .map(|dt| dt.format("%Y-%m-%d %H:%M UTC").to_string())
                    .unwrap_or_else(|| "-".to_string()),
            ),
    )
}

async fn declaration_new() -> impl IntoResponse {
    Html(
        DECLARATION_FORM_HTML
            .replace("<!-- TITLE -->", "New Declaration")
            .replace("<!-- ACTION -->", "/api/declarations")
            .replace("<!-- RKEY -->", "")
            .replace("<!-- PREDICATE -->", "")
            .replace("<!-- ARGS_JSON -->", "[]")
            .replace("<!-- DESCRIPTION -->", "")
            .replace("<!-- TAGS -->", ""),
    )
}

async fn declaration_edit(
    State(state): State<Arc<AppState>>,
    Path(rkey): Path<String>,
) -> impl IntoResponse {
    let declaration = match state
        .client
        .get_record::<FactDeclaration>(FACT_DECLARATION_COLLECTION, &rkey)
        .await
    {
        Ok(d) => d.value,
        Err(_) => return Html(DECLARATION_NOT_FOUND_HTML.to_string()),
    };

    // Serialize args to JSON for the form
    let args_json =
        serde_json::to_string_pretty(&declaration.args).unwrap_or_else(|_| "[]".to_string());

    Html(
        DECLARATION_FORM_HTML
            .replace("<!-- TITLE -->", "Edit Declaration")
            .replace("<!-- ACTION -->", &format!("/api/declarations/{}", rkey))
            .replace("<!-- RKEY -->", &rkey)
            .replace("<!-- PREDICATE -->", &html_escape(&declaration.predicate))
            .replace("<!-- ARGS_JSON -->", &html_escape(&args_json))
            .replace(
                "<!-- DESCRIPTION -->",
                &html_escape(&declaration.description),
            )
            .replace("<!-- TAGS -->", &declaration.tags.join(", ")),
    )
}

async fn create_declaration(
    State(state): State<Arc<AppState>>,
    Form(form): Form<FactDeclarationForm>,
) -> impl IntoResponse {
    // Parse args from JSON
    let args: Vec<FactDeclArg> = serde_json::from_str(&form.args_json).unwrap_or_default();

    let now = Utc::now();
    let declaration = FactDeclaration {
        predicate: form.predicate,
        args,
        description: form.description,
        tags: parse_comma_separated(&form.tags),
        created_at: now,
        last_updated: Some(now),
    };

    let rkey = Tid::now().to_string();
    match state
        .client
        .create_record(FACT_DECLARATION_COLLECTION, Some(&rkey), &declaration)
        .await
    {
        Ok(_) => Redirect::to(&format!("/declarations/{}", rkey)),
        Err(e) => {
            warn!(error = %e, "failed to create declaration");
            Redirect::to("/declarations")
        }
    }
}

async fn update_declaration(
    State(state): State<Arc<AppState>>,
    Path(rkey): Path<String>,
    Form(form): Form<FactDeclarationForm>,
) -> impl IntoResponse {
    let existing = match state
        .client
        .get_record::<FactDeclaration>(FACT_DECLARATION_COLLECTION, &rkey)
        .await
    {
        Ok(d) => d.value,
        Err(_) => return Redirect::to("/declarations"),
    };

    // Parse args from JSON
    let args: Vec<FactDeclArg> = serde_json::from_str(&form.args_json).unwrap_or_default();

    let declaration = FactDeclaration {
        predicate: form.predicate,
        args,
        description: form.description,
        tags: parse_comma_separated(&form.tags),
        created_at: existing.created_at,
        last_updated: Some(Utc::now()),
    };

    match state
        .client
        .put_record(FACT_DECLARATION_COLLECTION, &rkey, &declaration)
        .await
    {
        Ok(_) => Redirect::to(&format!("/declarations/{}", rkey)),
        Err(e) => {
            warn!(error = %e, "failed to update declaration");
            Redirect::to(&format!("/declarations/{}", rkey))
        }
    }
}

async fn delete_declaration(
    State(state): State<Arc<AppState>>,
    Path(rkey): Path<String>,
) -> impl IntoResponse {
    let _ = state
        .client
        .delete_record(FACT_DECLARATION_COLLECTION, &rkey)
        .await;
    Redirect::to("/declarations")
}

async fn notes_page(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    let notes = match state.client.list_all_records::<Note>(NOTE_COLLECTION).await {
        Ok(n) => n,
        Err(e) => {
            warn!(error = %e, "failed to load notes for notes page");
            Vec::new()
        }
    };

    let mut notes_html = String::new();
    for item in &notes {
        let rkey = item.uri.split('/').next_back().unwrap_or("");
        let preview = truncate_chars(&item.value.content, 100);

        notes_html.push_str(&format!(
            r#"<div class="note">
                <div class="note-header">
                    <a href="/notes/{}" class="title">{}</a>
                    <span class="category">{}</span>
                </div>
                <div class="preview">{}</div>
                <div class="meta">
                    <span class="rkey">{}</span>
                    <span class="tags">{}</span>
                </div>
            </div>"#,
            rkey,
            html_escape(&item.value.title),
            item.value.category.as_deref().unwrap_or(""),
            html_escape(&preview),
            rkey,
            item.value.tags.join(", ")
        ));
    }

    Html(
        NOTES_HTML
            .replace("<!-- NOTES -->", &notes_html)
            .replace("<!-- COUNT -->", &notes.len().to_string()),
    )
}

async fn note_detail(
    State(state): State<Arc<AppState>>,
    Path(rkey): Path<String>,
) -> impl IntoResponse {
    let note = match state
        .client
        .get_record::<Note>(NOTE_COLLECTION, &rkey)
        .await
    {
        Ok(n) => n.value,
        Err(_) => return Html(NOTE_NOT_FOUND_HTML.to_string()),
    };

    let tags_html = if note.tags.is_empty() {
        String::new()
    } else {
        format!(
            "<p class=\"tags\">Tags: {}</p>",
            note.tags
                .iter()
                .map(|t| html_escape(t))
                .collect::<Vec<_>>()
                .join(", ")
        )
    };

    let category_html = note
        .category
        .as_ref()
        .map(|c| format!("<span class=\"category\">{}</span>", html_escape(c)))
        .unwrap_or_default();

    let related_facts_html = if note.related_facts.is_empty() {
        String::new()
    } else {
        format!(
            "<h2>Related Facts</h2><ul class=\"related-facts\">{}</ul>",
            note.related_facts
                .iter()
                .map(|f| format!("<li><code>{}</code></li>", html_escape(f)))
                .collect::<String>()
        )
    };

    Html(
        NOTE_DETAIL_HTML
            .replace("<!-- RKEY -->", &rkey)
            .replace("<!-- TITLE -->", &html_escape(&note.title))
            .replace("<!-- CATEGORY -->", &category_html)
            .replace("<!-- CONTENT -->", &html_escape(&note.content))
            .replace("<!-- TAGS -->", &tags_html)
            .replace("<!-- RELATED_FACTS -->", &related_facts_html)
            .replace(
                "<!-- CREATED_AT -->",
                &note.created_at.format("%Y-%m-%d %H:%M UTC").to_string(),
            )
            .replace(
                "<!-- UPDATED_AT -->",
                &note.last_updated.format("%Y-%m-%d %H:%M UTC").to_string(),
            ),
    )
}

async fn note_new() -> impl IntoResponse {
    Html(
        NOTE_FORM_HTML
            .replace("<!-- TITLE -->", "New Note")
            .replace("<!-- ACTION -->", "/api/notes")
            .replace("<!-- RKEY -->", "")
            .replace("<!-- NOTE_TITLE -->", "")
            .replace("<!-- CONTENT -->", "")
            .replace("<!-- CATEGORY -->", "")
            .replace("<!-- TAGS -->", ""),
    )
}

async fn note_edit(
    State(state): State<Arc<AppState>>,
    Path(rkey): Path<String>,
) -> impl IntoResponse {
    let note = match state
        .client
        .get_record::<Note>(NOTE_COLLECTION, &rkey)
        .await
    {
        Ok(n) => n.value,
        Err(_) => return Html(NOTE_NOT_FOUND_HTML.to_string()),
    };

    Html(
        NOTE_FORM_HTML
            .replace("<!-- TITLE -->", "Edit Note")
            .replace("<!-- ACTION -->", &format!("/api/notes/{}", rkey))
            .replace("<!-- RKEY -->", &rkey)
            .replace("<!-- NOTE_TITLE -->", &html_escape(&note.title))
            .replace("<!-- CONTENT -->", &html_escape(&note.content))
            .replace(
                "<!-- CATEGORY -->",
                &html_escape(note.category.as_deref().unwrap_or("")),
            )
            .replace("<!-- TAGS -->", &note.tags.join(", ")),
    )
}

async fn create_note(
    State(state): State<Arc<AppState>>,
    Form(form): Form<NoteForm>,
) -> impl IntoResponse {
    let now = Utc::now();
    let note = Note {
        title: form.title,
        content: form.content,
        category: form.category.filter(|s| !s.is_empty()),
        related_facts: Vec::new(),
        tags: parse_comma_separated(&form.tags),
        created_at: now,
        last_updated: now,
    };

    let rkey = Tid::now().to_string();
    match state
        .client
        .create_record(NOTE_COLLECTION, Some(&rkey), &note)
        .await
    {
        Ok(_) => Redirect::to(&format!("/notes/{}", rkey)),
        Err(e) => {
            warn!(error = %e, "failed to create note");
            Redirect::to("/notes")
        }
    }
}

async fn update_note(
    State(state): State<Arc<AppState>>,
    Path(rkey): Path<String>,
    Form(form): Form<NoteForm>,
) -> impl IntoResponse {
    let existing = match state
        .client
        .get_record::<Note>(NOTE_COLLECTION, &rkey)
        .await
    {
        Ok(n) => n.value,
        Err(_) => return Redirect::to("/notes"),
    };

    let note = Note {
        title: form.title,
        content: form.content,
        category: form.category.filter(|s| !s.is_empty()),
        related_facts: existing.related_facts,
        tags: parse_comma_separated(&form.tags),
        created_at: existing.created_at,
        last_updated: Utc::now(),
    };

    match state.client.put_record(NOTE_COLLECTION, &rkey, &note).await {
        Ok(_) => Redirect::to(&format!("/notes/{}", rkey)),
        Err(e) => {
            warn!(error = %e, "failed to update note");
            Redirect::to(&format!("/notes/{}", rkey))
        }
    }
}

async fn delete_note(
    State(state): State<Arc<AppState>>,
    Path(rkey): Path<String>,
) -> impl IntoResponse {
    let _ = state.client.delete_record(NOTE_COLLECTION, &rkey).await;
    Redirect::to("/notes")
}

// =============================================================================
// Wiki
// =============================================================================

async fn wiki_page(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    let entries = match state
        .client
        .list_all_records::<WikiEntry>(WIKI_ENTRY_COLLECTION)
        .await
    {
        Ok(n) => n,
        Err(e) => {
            warn!(error = %e, "failed to load wiki entries");
            Vec::new()
        }
    };

    let mut entries_html = String::new();
    for item in &entries {
        let rkey = item.uri.split('/').next_back().unwrap_or("");
        let preview = item
            .value
            .summary
            .as_deref()
            .unwrap_or_else(|| &item.value.content);
        let preview = truncate_chars(preview, 120);
        let status_class = match item.value.status.as_str() {
            "draft" => "status-draft",
            "deprecated" => "status-deprecated",
            _ => "status-stable",
        };

        entries_html.push_str(&format!(
            r#"<div class="wiki-entry">
                <div class="entry-header">
                    <a href="/wiki/{}" class="title">{}</a>
                    <span class="status {}">{}</span>
                </div>
                <div class="slug">/{}</div>
                <div class="preview">{}</div>
                <div class="meta">
                    <span class="rkey">{}</span>
                    <span class="tags">{}</span>
                </div>
            </div>"#,
            html_escape(&item.value.slug),
            html_escape(&item.value.title),
            status_class,
            html_escape(&item.value.status),
            html_escape(&item.value.slug),
            html_escape(&preview),
            rkey,
            item.value.tags.join(", ")
        ));
    }

    Html(
        WIKI_HTML
            .replace("<!-- ENTRIES -->", &entries_html)
            .replace("<!-- COUNT -->", &entries.len().to_string()),
    )
}

async fn wiki_detail(
    State(state): State<Arc<AppState>>,
    Path(slug_or_rkey): Path<String>,
) -> impl IntoResponse {
    // Try to find by slug first, then by rkey
    let entries = match state
        .client
        .list_all_records::<WikiEntry>(WIKI_ENTRY_COLLECTION)
        .await
    {
        Ok(n) => n,
        Err(_) => return Html(WIKI_NOT_FOUND_HTML.to_string()),
    };

    let found = entries.iter().find(|item| {
        item.value.slug == slug_or_rkey
            || item.value.aliases.iter().any(|a| a == &slug_or_rkey)
            || item.uri.split('/').next_back() == Some(&slug_or_rkey)
    });

    let item = match found {
        Some(item) => item,
        None => return Html(WIKI_NOT_FOUND_HTML.to_string()),
    };

    let rkey = item.uri.split('/').next_back().unwrap_or("");
    let entry = &item.value;

    // Render wiki links in content
    let rendered_content = render_wiki_content(&html_escape(&entry.content), &entries);

    let tags_html = if entry.tags.is_empty() {
        String::new()
    } else {
        format!(
            "<p class=\"tags\">Tags: {}</p>",
            entry
                .tags
                .iter()
                .map(|t| html_escape(t))
                .collect::<Vec<_>>()
                .join(", ")
        )
    };

    let aliases_html = if entry.aliases.is_empty() {
        String::new()
    } else {
        format!(
            "<p class=\"aliases\">Aliases: {}</p>",
            entry
                .aliases
                .iter()
                .map(|a| html_escape(a))
                .collect::<Vec<_>>()
                .join(", ")
        )
    };

    let summary_html = entry
        .summary
        .as_ref()
        .map(|s| format!("<p class=\"summary\">{}</p>", html_escape(s)))
        .unwrap_or_default();

    let supersedes_html = entry
        .supersedes
        .as_ref()
        .map(|s| format!("<p class=\"supersedes\">Supersedes: <code>{}</code></p>", html_escape(s)))
        .unwrap_or_default();

    let status_class = match entry.status.as_str() {
        "draft" => "status-draft",
        "deprecated" => "status-deprecated",
        _ => "status-stable",
    };

    // Find backlinks (entries that link to this one via wiki links)
    let backlinks_html = build_backlinks_html(state.as_ref(), rkey).await;

    Html(
        WIKI_DETAIL_HTML
            .replace("<!-- RKEY -->", rkey)
            .replace("<!-- TITLE -->", &html_escape(&entry.title))
            .replace("<!-- SLUG -->", &html_escape(&entry.slug))
            .replace("<!-- STATUS -->", &html_escape(&entry.status))
            .replace("<!-- STATUS_CLASS -->", status_class)
            .replace("<!-- SUMMARY -->", &summary_html)
            .replace("<!-- CONTENT -->", &rendered_content)
            .replace("<!-- TAGS -->", &tags_html)
            .replace("<!-- ALIASES -->", &aliases_html)
            .replace("<!-- SUPERSEDES -->", &supersedes_html)
            .replace("<!-- BACKLINKS -->", &backlinks_html)
            .replace(
                "<!-- CREATED_AT -->",
                &entry.created_at.format("%Y-%m-%d %H:%M UTC").to_string(),
            )
            .replace(
                "<!-- UPDATED_AT -->",
                &entry.last_updated.format("%Y-%m-%d %H:%M UTC").to_string(),
            ),
    )
}

async fn wiki_new() -> impl IntoResponse {
    Html(
        WIKI_FORM_HTML
            .replace("<!-- TITLE -->", "New Wiki Entry")
            .replace("<!-- ACTION -->", "/api/wiki")
            .replace("<!-- RKEY -->", "")
            .replace("<!-- ENTRY_TITLE -->", "")
            .replace("<!-- SLUG -->", "")
            .replace("<!-- CONTENT -->", "")
            .replace("<!-- STATUS -->", "stable")
            .replace("<!-- SUMMARY -->", "")
            .replace("<!-- ALIASES -->", "")
            .replace("<!-- TAGS -->", "")
            .replace("<!-- SUPERSEDES -->", ""),
    )
}

async fn wiki_edit(
    State(state): State<Arc<AppState>>,
    Path(rkey): Path<String>,
) -> impl IntoResponse {
    let entry = match state
        .client
        .get_record::<WikiEntry>(WIKI_ENTRY_COLLECTION, &rkey)
        .await
    {
        Ok(n) => n.value,
        Err(_) => return Html(WIKI_NOT_FOUND_HTML.to_string()),
    };

    Html(
        WIKI_FORM_HTML
            .replace("<!-- TITLE -->", "Edit Wiki Entry")
            .replace("<!-- ACTION -->", &format!("/api/wiki/{}", rkey))
            .replace("<!-- RKEY -->", &rkey)
            .replace("<!-- ENTRY_TITLE -->", &html_escape(&entry.title))
            .replace("<!-- SLUG -->", &html_escape(&entry.slug))
            .replace("<!-- CONTENT -->", &html_escape(&entry.content))
            .replace("<!-- STATUS -->", &html_escape(&entry.status))
            .replace(
                "<!-- SUMMARY -->",
                &html_escape(entry.summary.as_deref().unwrap_or("")),
            )
            .replace("<!-- ALIASES -->", &entry.aliases.join(", "))
            .replace("<!-- TAGS -->", &entry.tags.join(", "))
            .replace(
                "<!-- SUPERSEDES -->",
                &html_escape(entry.supersedes.as_deref().unwrap_or("")),
            ),
    )
}

async fn create_wiki_entry_web(
    State(state): State<Arc<AppState>>,
    Form(form): Form<WikiEntryForm>,
) -> impl IntoResponse {
    let now = Utc::now();
    let entry = WikiEntry {
        title: form.title,
        slug: form.slug.clone(),
        aliases: parse_comma_separated(&form.aliases),
        summary: form.summary.filter(|s| !s.is_empty()),
        content: form.content,
        status: form.status.unwrap_or_else(|| "stable".to_string()),
        supersedes: form.supersedes.filter(|s| !s.is_empty()),
        tags: parse_comma_separated(&form.tags),
        created_at: now,
        last_updated: now,
    };

    let rkey = Tid::now().to_string();
    match state
        .client
        .create_record(WIKI_ENTRY_COLLECTION, Some(&rkey), &entry)
        .await
    {
        Ok(_) => Redirect::to(&format!("/wiki/{}", entry.slug)),
        Err(e) => {
            warn!(error = %e, "failed to create wiki entry");
            Redirect::to("/wiki")
        }
    }
}

async fn update_wiki_entry_web(
    State(state): State<Arc<AppState>>,
    Path(rkey): Path<String>,
    Form(form): Form<WikiEntryForm>,
) -> impl IntoResponse {
    let existing = match state
        .client
        .get_record::<WikiEntry>(WIKI_ENTRY_COLLECTION, &rkey)
        .await
    {
        Ok(n) => n.value,
        Err(_) => return Redirect::to("/wiki"),
    };

    let entry = WikiEntry {
        title: form.title,
        slug: existing.slug.clone(), // Slug is immutable
        aliases: parse_comma_separated(&form.aliases),
        summary: form.summary.filter(|s| !s.is_empty()),
        content: form.content,
        status: form.status.unwrap_or(existing.status),
        supersedes: form.supersedes.filter(|s| !s.is_empty()),
        tags: parse_comma_separated(&form.tags),
        created_at: existing.created_at,
        last_updated: Utc::now(),
    };

    match state
        .client
        .put_record(WIKI_ENTRY_COLLECTION, &rkey, &entry)
        .await
    {
        Ok(_) => Redirect::to(&format!("/wiki/{}", entry.slug)),
        Err(e) => {
            warn!(error = %e, "failed to update wiki entry");
            Redirect::to(&format!("/wiki/{}", entry.slug))
        }
    }
}

async fn delete_wiki_entry_web(
    State(state): State<Arc<AppState>>,
    Path(rkey): Path<String>,
) -> impl IntoResponse {
    let _ = state
        .client
        .delete_record(WIKI_ENTRY_COLLECTION, &rkey)
        .await;
    Redirect::to("/wiki")
}

/// Render wiki-link syntax in content, replacing [[slug]] with HTML links.
fn render_wiki_content(escaped_content: &str, all_entries: &[winter_atproto::ListRecordItem<WikiEntry>]) -> String {
    // Since content is already HTML-escaped, we need to look for escaped brackets
    // [[...]] becomes &amp;#91;&amp;#91;...&amp;#93;&amp;#93; but actually html_escape doesn't
    // touch brackets, so [[...]] remains as [[...]] in the escaped output.
    let re = regex::Regex::new(r"\[\[([^\]|]+?)(?:\|([^\]]+))?\]\]").unwrap();

    re.replace_all(escaped_content, |caps: &regex::Captures| {
        let reference = caps[1].trim();
        let display_text = caps.get(2).map(|m| m.as_str().trim().to_string());

        // For local slugs, try to resolve
        if !reference.contains('/') && !reference.starts_with("did:") {
            let found = all_entries.iter().find(|item| {
                item.value.slug == reference
                    || item.value.aliases.iter().any(|a| a == reference)
            });

            let display = display_text.as_deref().unwrap_or(reference);

            if found.is_some() {
                format!(
                    r#"<a href="/wiki/{}" class="wiki-link">{}</a>"#,
                    reference, display
                )
            } else {
                format!(
                    r#"<a href="/wiki/{}" class="wiki-link wiki-link-missing">{}</a>"#,
                    reference, display
                )
            }
        } else {
            let display = display_text.as_deref().unwrap_or(reference);
            format!(
                r#"<span class="wiki-link-external">{}</span>"#,
                display
            )
        }
    })
    .to_string()
}

/// Build backlinks HTML by checking wiki links targeting this entry.
async fn build_backlinks_html(state: &AppState, target_rkey: &str) -> String {
    let links = match state
        .client
        .list_all_records::<WikiLink>(WIKI_LINK_COLLECTION)
        .await
    {
        Ok(l) => l,
        Err(_) => return String::new(),
    };

    let entries = match state
        .client
        .list_all_records::<WikiEntry>(WIKI_ENTRY_COLLECTION)
        .await
    {
        Ok(e) => e,
        Err(_) => return String::new(),
    };

    // Find links that target this entry (by rkey in the AT URI)
    let backlinks: Vec<String> = links
        .iter()
        .filter(|link| {
            link.value
                .target
                .split('/')
                .next_back()
                .map(|rk| rk == target_rkey)
                .unwrap_or(false)
        })
        .filter_map(|link| {
            // Find the source entry
            let source_rkey = link.value.source.split('/').next_back()?;
            let source_entry = entries.iter().find(|e| {
                e.uri.split('/').next_back() == Some(source_rkey)
            })?;

            Some(format!(
                r#"<li><a href="/wiki/{}">{}</a> <span class="link-type">({})</span></li>"#,
                html_escape(&source_entry.value.slug),
                html_escape(&source_entry.value.title),
                html_escape(&link.value.link_type),
            ))
        })
        .collect();

    if backlinks.is_empty() {
        String::new()
    } else {
        format!(
            r#"<h2>Backlinks</h2><ul class="backlinks">{}</ul>"#,
            backlinks.join("")
        )
    }
}

async fn thoughts_sse(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    let rx = state.thought_tx.subscribe();
    create_sse_stream(rx)
}

fn html_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#39;")
}

/// Truncate a string to a maximum number of characters (not bytes).
/// Safe for UTF-8 strings with multi-byte characters.
fn truncate_chars(s: &str, max_chars: usize) -> String {
    let char_count = s.chars().count();
    if char_count <= max_chars {
        s.to_string()
    } else {
        format!("{}...", s.chars().take(max_chars).collect::<String>())
    }
}

// =============================================================================
// Form Structs and Helpers
// =============================================================================

/// Parse comma-separated string into Vec<String>.
fn parse_comma_separated(s: &Option<String>) -> Vec<String> {
    s.as_ref()
        .map(|t| {
            t.split(',')
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
                .collect()
        })
        .unwrap_or_default()
}

/// Parse newline-separated string into Vec<String>.
fn parse_newline_separated(s: &str) -> Vec<String> {
    s.lines()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect()
}

#[derive(Deserialize)]
struct NoteForm {
    title: String,
    content: String,
    category: Option<String>,
    tags: Option<String>,
}

#[derive(Deserialize)]
struct WikiEntryForm {
    title: String,
    slug: String,
    content: String,
    status: Option<String>,
    summary: Option<String>,
    aliases: Option<String>,
    tags: Option<String>,
    supersedes: Option<String>,
}

#[derive(Deserialize)]
struct FactForm {
    predicate: String,
    args: String,
    confidence: Option<f64>,
    source: Option<String>,
    tags: Option<String>,
}

#[derive(Deserialize)]
struct RuleForm {
    name: String,
    description: Option<String>,
    head: String,
    body: String,
    constraints: Option<String>,
    enabled: Option<String>,
    priority: Option<i32>,
}

#[derive(Deserialize)]
struct JobForm {
    name: String,
    instructions: String,
    schedule_type: String,
    schedule_at: Option<String>,
    schedule_seconds: Option<u64>,
}

#[derive(Deserialize)]
struct DirectiveForm {
    kind: String,
    content: String,
    summary: Option<String>,
    active: Option<String>,
    confidence: Option<f64>,
    source: Option<String>,
    priority: Option<i32>,
    tags: Option<String>,
}

#[derive(Deserialize)]
struct FactDeclarationForm {
    predicate: String,
    args_json: String,
    description: String,
    tags: Option<String>,
}

const INDEX_HTML: &str = r#"<!DOCTYPE html>
<html>
<head>
    <meta charset="utf-8">
    <meta name="viewport" content="width=device-width, initial-scale=1">
    <title>Winter</title>
    <style>
        body {
            font-family: -apple-system, BlinkMacSystemFont, "Segoe UI", Roboto, sans-serif;
            max-width: 800px;
            margin: 0 auto;
            padding: 2rem;
            background: #0a0a0a;
            color: #e0e0e0;
        }
        h1 { color: #88c0d0; }
        a { color: #81a1c1; }
        nav { margin: 2rem 0; display: flex; flex-wrap: wrap; gap: 0.5rem; }
        nav a {
            padding: 0.5rem 1rem;
            background: #2e3440;
            border-radius: 4px;
            text-decoration: none;
        }
        nav a:hover { background: #3b4252; }
    </style>
</head>
<body>
    <h1>Winter</h1>
    <p>Autonomous Bluesky Agent</p>
    <nav>
        <a href="/stream">Thought Stream</a>
        <a href="/facts">Facts</a>
        <a href="/rules">Rules</a>
        <a href="/declarations">Declarations</a>
        <a href="/notes">Notes</a>
        <a href="/wiki">Wiki</a>
        <a href="/directives">Directives</a>
        <a href="/identity">Identity</a>
        <a href="/jobs">Jobs</a>
        <a href="/tools">Tools</a>
        <a href="/secrets">Secrets</a>
    </nav>
</body>
</html>"#;

const STREAM_HTML: &str = r#"<!DOCTYPE html>
<html>
<head>
    <meta charset="utf-8">
    <meta name="viewport" content="width=device-width, initial-scale=1">
    <title>Winter - Thought Stream</title>
    <style>
        body {
            font-family: -apple-system, BlinkMacSystemFont, "Segoe UI", Roboto, sans-serif;
            max-width: 800px;
            margin: 0 auto;
            padding: 2rem;
            background: #0a0a0a;
            color: #e0e0e0;
        }
        h1 { color: #88c0d0; }
        h1 a { color: #88c0d0; text-decoration: none; }
        a { color: #81a1c1; }
        .filters {
            margin: 1rem 0;
            display: flex;
            flex-wrap: wrap;
            gap: 0.5rem;
        }
        .filter-btn {
            padding: 0.4rem 0.8rem;
            background: #2e3440;
            border: none;
            border-radius: 4px;
            color: #e0e0e0;
            cursor: pointer;
            font-size: 0.85rem;
        }
        .filter-btn:hover { background: #3b4252; }
        .filter-btn.active { background: #5e81ac; }
        .filter-btn.insight.active { background: #a3be8c; color: #000; }
        .filter-btn.question.active { background: #b48ead; color: #000; }
        .filter-btn.plan.active { background: #81a1c1; color: #000; }
        .filter-btn.reflection.active { background: #88c0d0; color: #000; }
        .filter-btn.error.active { background: #bf616a; }
        .filter-btn.response.active { background: #8fbcbb; color: #000; }
        .filter-btn.tool_call.active { background: #d08770; color: #000; }
        .thought {
            padding: 1rem;
            margin: 0.5rem 0;
            background: #2e3440;
            border-radius: 4px;
            border-left: 4px solid #88c0d0;
        }
        .thought.hidden { display: none; }
        .thought.insight { border-left-color: #a3be8c; }
        .thought.question { border-left-color: #b48ead; }
        .thought.plan { border-left-color: #81a1c1; }
        .thought.reflection { border-left-color: #88c0d0; }
        .thought.error { border-left-color: #bf616a; background: #3b2e2e; }
        .thought.response { border-left-color: #8fbcbb; }
        .thought.tool_call { border-left-color: #d08770; }
        .thought-header {
            display: flex;
            justify-content: space-between;
            align-items: center;
            margin-bottom: 0.5rem;
        }
        .kind {
            font-size: 0.75rem;
            font-weight: 600;
            text-transform: uppercase;
            letter-spacing: 0.05em;
            padding: 0.2rem 0.5rem;
            border-radius: 3px;
            background: #3b4252;
        }
        .thought.insight .kind { background: #a3be8c; color: #000; }
        .thought.question .kind { background: #b48ead; color: #000; }
        .thought.plan .kind { background: #81a1c1; color: #000; }
        .thought.reflection .kind { background: #88c0d0; color: #000; }
        .thought.error .kind { background: #bf616a; color: #fff; }
        .thought.response .kind { background: #8fbcbb; color: #000; }
        .thought.tool_call .kind { background: #d08770; color: #000; }
        .time {
            font-size: 0.8rem;
            color: #666;
        }
        .time:hover { color: #888; }
        .content {
            line-height: 1.6;
            word-wrap: break-word;
            overflow-wrap: break-word;
            white-space: pre-wrap;
            max-height: 350px;
            overflow: hidden;
            position: relative;
        }
        .content.truncated::after {
            content: '';
            position: absolute;
            bottom: 0;
            left: 0;
            right: 0;
            height: 40px;
            background: linear-gradient(transparent, #2e3440);
        }
        .thought.error .content.truncated::after {
            background: linear-gradient(transparent, #3b2e2e);
        }
        .expand-btn {
            display: none;
            margin-top: 0.5rem;
            padding: 0.25rem 0.5rem;
            background: #3b4252;
            border: none;
            border-radius: 3px;
            color: #81a1c1;
            cursor: pointer;
            font-size: 0.8rem;
        }
        .expand-btn:hover { background: #4c566a; }
        .content.truncated + .expand-btn { display: inline-block; }
        .thought.tool_call .content {
            font-family: "SF Mono", "Menlo", "Monaco", monospace;
            font-size: 0.9rem;
            background: #252a33;
            padding: 0.5rem 0.75rem;
            border-radius: 3px;
            margin-top: 0.5rem;
            overflow: visible;
            max-height: none;
        }
        .tool-header {
            margin-bottom: 0.5rem;
        }
        .tool-name {
            color: #88c0d0;
            font-weight: 600;
            font-size: 1rem;
        }
        .tool-args {
            color: #a3be8c;
        }
        .tool-failed {
            color: #bf616a;
            font-weight: 600;
        }
        .tool-section {
            margin-top: 0.5rem;
        }
        .tool-section-header {
            color: #81a1c1;
            font-size: 0.85rem;
            cursor: pointer;
            padding: 0.25rem 0;
        }
        .tool-section-header:hover {
            color: #88c0d0;
        }
        .tool-json {
            background: #1e222a;
            padding: 0.75rem;
            border-radius: 3px;
            margin: 0.25rem 0 0 0;
            overflow-x: auto;
            font-size: 0.85rem;
            line-height: 1.5;
            max-height: 400px;
            overflow-y: auto;
        }
        .json-string { color: #a3be8c; }
        .json-number { color: #d08770; }
        .json-keyword { color: #b48ead; }
        .tool-summary {
            color: #d8dee9;
            font-size: 0.9rem;
            margin: 0.5rem 0;
            padding: 0.5rem;
            background: #1e222a;
            border-radius: 3px;
            border-left: 3px solid #81a1c1;
        }
        .tool-error {
            color: #bf616a;
            background: #2e2226;
            padding: 0.5rem 0.75rem;
            border-radius: 3px;
            margin: 0.5rem 0;
            border-left: 3px solid #bf616a;
        }
        .tool-link-btn {
            float: right;
            padding: 0.25rem 0.5rem;
            background: #5e81ac;
            color: #fff;
            text-decoration: none;
            font-size: 0.75rem;
            border-radius: 3px;
            margin-left: 0.5rem;
        }
        .tool-link-btn:hover {
            background: #81a1c1;
            text-decoration: none;
        }
        .trigger {
            font-size: 0.8rem;
            color: #888;
            margin-top: 0.5rem;
            font-style: italic;
            cursor: pointer;
        }
        .trigger:hover {
            color: #81a1c1;
        }
        .trigger::before {
            content: "↳ ";
            color: #666;
        }
        .tags {
            margin-top: 0.5rem;
            display: flex;
            flex-wrap: wrap;
            gap: 0.25rem;
        }
        .tag {
            font-size: 0.7rem;
            padding: 0.15rem 0.4rem;
            background: #3b4252;
            border-radius: 3px;
            color: #88c0d0;
            cursor: pointer;
        }
        .tag:hover {
            background: #4c566a;
            color: #8fbcbb;
        }
        .active-filter {
            margin: 0.5rem 0;
            padding: 0.5rem 0.75rem;
            background: #3b4252;
            border-radius: 4px;
            display: none;
            align-items: center;
            gap: 0.5rem;
            font-size: 0.9rem;
        }
        .active-filter.visible {
            display: flex;
        }
        .active-filter-label {
            color: #888;
        }
        .active-filter-value {
            color: #88c0d0;
            font-family: "SF Mono", "Menlo", "Monaco", monospace;
            font-size: 0.85rem;
            max-width: 400px;
            overflow: hidden;
            text-overflow: ellipsis;
            white-space: nowrap;
        }
        .clear-filter-btn {
            margin-left: auto;
            padding: 0.2rem 0.5rem;
            background: #4c566a;
            border: none;
            border-radius: 3px;
            color: #e0e0e0;
            cursor: pointer;
            font-size: 0.8rem;
        }
        .clear-filter-btn:hover {
            background: #5e81ac;
        }
        .duration {
            font-size: 0.75rem;
            color: #666;
            margin-left: 0.5rem;
        }
        .modal-overlay {
            display: none;
            position: fixed;
            top: 0;
            left: 0;
            right: 0;
            bottom: 0;
            background: rgba(0, 0, 0, 0.8);
            z-index: 1000;
            justify-content: center;
            align-items: center;
            padding: 2rem;
        }
        .modal-overlay.visible { display: flex; }
        .modal {
            background: #2e3440;
            border-radius: 8px;
            max-width: 800px;
            max-height: 90vh;
            width: 100%;
            overflow: hidden;
            display: flex;
            flex-direction: column;
        }
        .modal-header {
            display: flex;
            justify-content: space-between;
            align-items: center;
            padding: 1rem;
            border-bottom: 1px solid #3b4252;
        }
        .modal-close {
            background: none;
            border: none;
            color: #888;
            font-size: 1.5rem;
            cursor: pointer;
            padding: 0.25rem 0.5rem;
        }
        .modal-close:hover { color: #e0e0e0; }
        .modal-body {
            padding: 1rem;
            overflow-y: auto;
            white-space: pre-wrap;
            line-height: 1.6;
        }
    </style>
</head>
<body>
    <h1><a href="/">Winter</a> / Thought Stream</h1>
    <div class="filters">
        <button class="filter-btn active" data-kind="all">All</button>
        <button class="filter-btn insight" data-kind="insight">Insight</button>
        <button class="filter-btn question" data-kind="question">Question</button>
        <button class="filter-btn plan" data-kind="plan">Plan</button>
        <button class="filter-btn reflection" data-kind="reflection">Reflection</button>
        <button class="filter-btn response" data-kind="response">Response</button>
        <button class="filter-btn tool_call" data-kind="tool_call">Tool Call</button>
        <button class="filter-btn error" data-kind="error">Error</button>
    </div>
    <div id="active-filter" class="active-filter">
        <span class="active-filter-label">Filtering by</span>
        <span id="active-filter-type"></span>
        <span id="active-filter-value" class="active-filter-value"></span>
        <button id="clear-filter" class="clear-filter-btn">Clear</button>
    </div>
    <div id="stream"><!-- THOUGHTS --></div>
    <div id="modal-overlay" class="modal-overlay">
        <div class="modal">
            <div class="modal-header">
                <span id="modal-kind" class="kind"></span>
                <button class="modal-close">&times;</button>
            </div>
            <div id="modal-body" class="modal-body"></div>
        </div>
    </div>
    <script>
        const stream = document.getElementById('stream');
        const filterBtns = document.querySelectorAll('.filter-btn');
        const activeFilterEl = document.getElementById('active-filter');
        const activeFilterType = document.getElementById('active-filter-type');
        const activeFilterValue = document.getElementById('active-filter-value');
        const clearFilterBtn = document.getElementById('clear-filter');

        let activeKindFilter = 'all';
        let filterTrigger = null;
        let filterTag = null;

        // Initialize from URL params
        function initFromUrl() {
            const params = new URLSearchParams(window.location.search);
            filterTrigger = params.get('trigger');
            filterTag = params.get('tag');
            const kind = params.get('kind');
            if (kind && ['insight','question','plan','reflection','error','response','tool_call'].includes(kind)) {
                activeKindFilter = kind;
                filterBtns.forEach(b => {
                    b.classList.toggle('active', b.dataset.kind === kind);
                });
            }
            updateFilterIndicator();
            applyFilter();
        }

        // Update URL to reflect current filter state
        function updateUrl() {
            const params = new URLSearchParams();
            if (activeKindFilter !== 'all') params.set('kind', activeKindFilter);
            if (filterTrigger) params.set('trigger', filterTrigger);
            if (filterTag) params.set('tag', filterTag);
            const newUrl = params.toString() ? '?' + params.toString() : window.location.pathname;
            history.pushState({}, '', newUrl);
        }

        // Update the filter indicator bar
        function updateFilterIndicator() {
            if (filterTrigger || filterTag) {
                activeFilterEl.classList.add('visible');
                if (filterTrigger) {
                    activeFilterType.textContent = 'trigger:';
                    activeFilterValue.textContent = filterTrigger;
                    activeFilterValue.title = filterTrigger;
                } else {
                    activeFilterType.textContent = 'tag:';
                    activeFilterValue.textContent = filterTag;
                    activeFilterValue.title = filterTag;
                }
            } else {
                activeFilterEl.classList.remove('visible');
            }
        }

        // Clear trigger/tag filter
        function clearTriggerTagFilter() {
            filterTrigger = null;
            filterTag = null;
            updateFilterIndicator();
            updateUrl();
            applyFilter();
        }

        clearFilterBtn.addEventListener('click', clearTriggerTagFilter);

        // Filter button handling
        filterBtns.forEach(btn => {
            btn.addEventListener('click', () => {
                filterBtns.forEach(b => b.classList.remove('active'));
                btn.classList.add('active');
                activeKindFilter = btn.dataset.kind;
                updateUrl();
                applyFilter();
            });
        });

        function applyFilter() {
            document.querySelectorAll('.thought').forEach(thought => {
                let visible = true;

                // Kind filter
                if (activeKindFilter !== 'all' && !thought.classList.contains(activeKindFilter)) {
                    visible = false;
                }

                // Trigger filter (prefix match)
                if (visible && filterTrigger) {
                    const thoughtTrigger = thought.dataset.trigger || '';
                    if (!thoughtTrigger.startsWith(filterTrigger)) {
                        visible = false;
                    }
                }

                // Tag filter (exact match)
                if (visible && filterTag) {
                    const thoughtTags = thought.dataset.tags ? JSON.parse(thought.dataset.tags) : [];
                    if (!thoughtTags.includes(filterTag)) {
                        visible = false;
                    }
                }

                thought.classList.toggle('hidden', !visible);
            });
        }

        // Format relative time
        function formatRelativeTime(date) {
            const now = new Date();
            const diff = now - date;
            const seconds = Math.floor(diff / 1000);
            const minutes = Math.floor(seconds / 60);
            const hours = Math.floor(minutes / 60);
            const days = Math.floor(hours / 24);

            if (seconds < 60) return 'just now';
            if (minutes === 1) return '1 minute ago';
            if (minutes < 60) return minutes + ' minutes ago';
            if (hours === 1) return '1 hour ago';
            if (hours < 24) return hours + ' hours ago';
            if (days === 1) return 'yesterday';
            if (days < 7) return days + ' days ago';
            return date.toLocaleDateString();
        }

        // Format tool call content for better readability
        // Handles three formats:
        // 1. JSON format: {"tool":"name","args":{...},"result":{...},"summary":"..."}
        // 2. Text format: "Called tool_name\nArgs:\n{json}\nResult:\n{json}"
        // 3. Legacy format: "Called tool_name [args]"
        function formatToolCallContent(content) {
            // Try parsing as JSON first (newest format)
            if (content.startsWith('{')) {
                try {
                    const data = JSON.parse(content);
                    return formatToolCallJson(data);
                } catch (e) {
                    // Not valid JSON, fall through to text parsing
                }
            }

            // Text format: "Called tool_name" on first line
            const lines = content.split('\n');
            if (lines.length === 0) return escapeHtml(content);

            const firstLine = lines[0];
            const headerMatch = firstLine.match(/^Called\s+(\S+)(\s*-\s*FAILED)?$/);

            if (headerMatch) {
                const toolName = headerMatch[1];
                const failed = headerMatch[2];

                let html = '<div class="tool-header">';
                html += '<span class="tool-name">' + escapeHtml(toolName) + '</span>';
                if (failed) {
                    html += ' <span class="tool-failed">FAILED</span>';
                }
                html += '</div>';

                // Parse the rest into sections (Args, Result)
                let currentSection = null;
                let currentContent = [];

                for (let i = 1; i < lines.length; i++) {
                    const line = lines[i];
                    if (line === 'Args:' || line === 'Result:' || line === 'Error:' || line.startsWith('Result: ')) {
                        // Flush previous section
                        if (currentSection && currentContent.length > 0) {
                            html += formatToolSection(currentSection, currentContent.join('\n'));
                        }
                        if (line.startsWith('Result: ')) {
                            // Inline summary result
                            currentSection = 'Result';
                            currentContent = [line.substring(8)];
                        } else {
                            currentSection = line.replace(':', '');
                            currentContent = [];
                        }
                    } else {
                        currentContent.push(line);
                    }
                }

                // Flush final section
                if (currentSection && currentContent.length > 0) {
                    html += formatToolSection(currentSection, currentContent.join('\n'));
                }

                return html;
            }

            // Legacy format: "Called tool_name [args]"
            const oldMatch = content.match(/^Called\s+(\w+)\s*\[(.*)\](\s*-\s*FAILED)?$/);
            if (oldMatch) {
                const toolName = oldMatch[1];
                const args = oldMatch[2];
                const failed = oldMatch[3];
                let html = '<span class="tool-name">' + escapeHtml(toolName) + '</span>';
                if (args) {
                    html += ' <span class="tool-args">' + escapeHtml(args) + '</span>';
                }
                if (failed) {
                    html += ' <span class="tool-failed">FAILED</span>';
                }
                return html;
            }
            return escapeHtml(content);
        }

        // Format tool call from JSON structure
        function formatToolCallJson(data) {
            const toolName = data.tool || 'unknown';
            const failed = data.failed || false;

            let html = '<div class="tool-header">';

            // Link at the top (if present)
            if (data.link) {
                html += '<a href="' + escapeHtml(data.link) + '" class="tool-link-btn">View →</a>';
            }

            html += '<span class="tool-name">' + escapeHtml(toolName) + '</span>';
            if (failed) {
                html += ' <span class="tool-failed">FAILED</span>';
            }
            html += '</div>';

            // Error section (for failed calls)
            if (data.error) {
                html += '<div class="tool-error">' + escapeHtml(data.error) + '</div>';
            }

            // Summary (quick view) - remove "View:" line since we show link separately
            if (data.summary) {
                const cleanSummary = data.summary.split('\n').filter(line => !line.startsWith('View:')).join('\n');
                if (cleanSummary) {
                    html += '<div class="tool-summary">' + escapeHtml(cleanSummary) + '</div>';
                }
            }

            // Args section
            if (data.args) {
                html += formatToolSection('Args', JSON.stringify(data.args, null, 2));
            }

            // Result section (collapsed by default for JSON format)
            if (data.result) {
                html += '<details class="tool-section">';
                html += '<summary class="tool-section-header">Result</summary>';
                html += '<pre class="tool-json">' + syntaxHighlightJson(JSON.stringify(data.result, null, 2)) + '</pre>';
                html += '</details>';
            }

            return html;
        }

        // Format a tool section (Args or Result or Error) with JSON syntax highlighting
        function formatToolSection(sectionName, content) {
            if (sectionName === 'Error') {
                return '<div class="tool-error">' + escapeHtml(content) + '</div>';
            }
            let html = '<details class="tool-section" open>';
            html += '<summary class="tool-section-header">' + sectionName + '</summary>';
            html += '<pre class="tool-json">' + syntaxHighlightJson(content) + '</pre>';
            html += '</details>';
            return html;
        }

        // Simple JSON syntax highlighting
        function syntaxHighlightJson(json) {
            const escaped = escapeHtml(json);
            return escaped
                // Strings (be careful with keys vs values)
                .replace(/"([^"\\]*(\\.[^"\\]*)*)"/g, function(match) {
                    return '<span class="json-string">' + match + '</span>';
                })
                // Numbers
                .replace(/\b(-?\d+\.?\d*)\b/g, '<span class="json-number">$1</span>')
                // Booleans and null
                .replace(/\b(true|false|null)\b/g, '<span class="json-keyword">$1</span>');
        }

        function escapeHtml(text) {
            const div = document.createElement('div');
            div.textContent = text;
            return div.innerHTML;
        }

        // Format thought content based on kind
        function formatContent(kind, content) {
            if (kind === 'tool_call') {
                return formatToolCallContent(content);
            }
            return escapeHtml(content);
        }

        // Build thought HTML
        function buildThoughtHtml(thought) {
            const date = new Date(thought.created_at);
            const relTime = formatRelativeTime(date);
            const absTime = date.toLocaleString();
            const content = formatContent(thought.kind, thought.content);

            let html = '<div class="thought-header">';
            html += '<span class="kind">' + thought.kind.replace('_', ' ') + '</span>';
            html += '<span class="time" title="' + absTime + '">' + relTime;
            if (thought.duration_ms) {
                html += '<span class="duration">(' + thought.duration_ms + 'ms)</span>';
            }
            html += '</span></div>';
            html += '<div class="content">' + content + '</div>';
            html += '<button class="expand-btn">Expand</button>';
            if (thought.trigger) {
                html += '<div class="trigger" data-trigger="' + escapeHtml(thought.trigger) + '">' + escapeHtml(thought.trigger) + '</div>';
            }
            if (thought.tags && thought.tags.length > 0) {
                html += '<div class="tags">';
                thought.tags.forEach(tag => {
                    html += '<span class="tag" data-tag="' + escapeHtml(tag) + '">' + escapeHtml(tag) + '</span>';
                });
                html += '</div>';
            }
            return html;
        }

        const eventSource = new EventSource('/api/thoughts/sse');

        eventSource.onmessage = function(event) {
            try {
                const thought = JSON.parse(event.data);
                const div = document.createElement('div');
                div.className = 'thought ' + thought.kind;
                if (thought.trigger) {
                    div.dataset.trigger = thought.trigger;
                }
                if (thought.tags && thought.tags.length > 0) {
                    div.dataset.tags = JSON.stringify(thought.tags);
                }
                div.innerHTML = buildThoughtHtml(thought);

                // Check if this thought should be hidden based on current filters
                let visible = true;
                if (activeKindFilter !== 'all' && thought.kind !== activeKindFilter) {
                    visible = false;
                }
                if (visible && filterTrigger && (!thought.trigger || !thought.trigger.startsWith(filterTrigger))) {
                    visible = false;
                }
                if (visible && filterTag && (!thought.tags || !thought.tags.includes(filterTag))) {
                    visible = false;
                }
                if (!visible) {
                    div.classList.add('hidden');
                }

                stream.prepend(div);
                // Check truncation for the new thought
                const contentEl = div.querySelector('.content');
                if (contentEl) checkTruncation(contentEl);
            } catch (e) {
                console.error('Failed to parse thought:', e, event.data);
            }
        };

        eventSource.onerror = function() {
            console.log('SSE connection lost, reconnecting...');
        };

        // Update relative times periodically
        setInterval(() => {
            document.querySelectorAll('.thought').forEach(thought => {
                const timeEl = thought.querySelector('.time');
                if (timeEl && timeEl.title) {
                    const date = new Date(timeEl.title);
                    const durationEl = timeEl.querySelector('.duration');
                    const durationHtml = durationEl ? durationEl.outerHTML : '';
                    timeEl.innerHTML = formatRelativeTime(date) + durationHtml;
                }
            });
        }, 60000);

        // Check if content is truncated (scrollHeight > clientHeight)
        function checkTruncation(contentEl) {
            if (contentEl.scrollHeight > contentEl.clientHeight) {
                contentEl.classList.add('truncated');
            }
        }

        // Apply to all thoughts on page load
        document.querySelectorAll('.thought .content').forEach(checkTruncation);

        // Modal handling
        const modalOverlay = document.getElementById('modal-overlay');
        const modalKind = document.getElementById('modal-kind');
        const modalBody = document.getElementById('modal-body');

        function openModal(thought) {
            const kind = thought.querySelector('.kind').textContent;
            const content = thought.querySelector('.content').innerHTML;
            modalKind.textContent = kind;
            modalKind.className = 'kind';
            // Copy the thought's kind class for color styling
            thought.classList.forEach(c => {
                if (['insight','question','plan','reflection','error','response','tool_call'].includes(c)) {
                    modalKind.classList.add(c);
                }
            });
            modalBody.innerHTML = content;
            modalOverlay.classList.add('visible');
        }

        function closeModal() {
            modalOverlay.classList.remove('visible');
        }

        // Click expand button to open modal, or trigger/tag to filter
        document.addEventListener('click', (e) => {
            if (e.target.classList.contains('expand-btn')) {
                openModal(e.target.closest('.thought'));
            } else if (e.target.classList.contains('trigger') && e.target.dataset.trigger) {
                filterTrigger = e.target.dataset.trigger;
                filterTag = null;
                updateFilterIndicator();
                updateUrl();
                applyFilter();
            } else if (e.target.classList.contains('tag') && e.target.dataset.tag) {
                filterTag = e.target.dataset.tag;
                filterTrigger = null;
                updateFilterIndicator();
                updateUrl();
                applyFilter();
            }
        });

        // Close modal on overlay click or close button
        modalOverlay.addEventListener('click', (e) => {
            if (e.target === modalOverlay || e.target.classList.contains('modal-close')) {
                closeModal();
            }
        });

        // Close on Escape key
        document.addEventListener('keydown', (e) => {
            if (e.key === 'Escape') closeModal();
        });

        // Initialize filters from URL on page load
        initFromUrl();
    </script>
</body>
</html>"#;

const FACTS_HTML: &str = r#"<!DOCTYPE html>
<html>
<head>
    <meta charset="utf-8">
    <meta name="viewport" content="width=device-width, initial-scale=1">
    <title>Winter - Facts</title>
    <style>
        body {
            font-family: -apple-system, BlinkMacSystemFont, "Segoe UI", Roboto, sans-serif;
            max-width: 1000px;
            margin: 0 auto;
            padding: 2rem;
            background: #0a0a0a;
            color: #e0e0e0;
        }
        h1 { color: #88c0d0; }
        h1 a { color: #88c0d0; text-decoration: none; }
        a { color: #81a1c1; }
        table { width: 100%; border-collapse: collapse; margin-top: 1rem; }
        th, td { padding: 0.75rem; text-align: left; border-bottom: 1px solid #3b4252; }
        th { background: #2e3440; color: #88c0d0; }
        tr:hover { background: #2e3440; }
        code { background: #3b4252; padding: 0.2rem 0.4rem; border-radius: 3px; font-size: 0.9rem; }
        .header { display: flex; justify-content: space-between; align-items: center; }
        .count { color: #888; }
        .btn { padding: 0.5rem 1rem; background: #5e81ac; color: #fff; border: none; border-radius: 4px; text-decoration: none; }
        .btn:hover { background: #81a1c1; }
    </style>
</head>
<body>
    <div class="header">
        <h1><a href="/">Winter</a> / Facts</h1>
        <a href="/facts/new" class="btn">New Fact</a>
    </div>
    <p class="count"><!-- COUNT --> facts in knowledge base</p>
    <table>
        <thead>
            <tr>
                <th>Key</th>
                <th>Predicate</th>
                <th>Arguments</th>
                <th>Confidence</th>
                <th>Tags</th>
            </tr>
        </thead>
        <tbody>
            <!-- FACTS -->
        </tbody>
    </table>
</body>
</html>"#;

const IDENTITY_HTML: &str = r#"<!DOCTYPE html>
<html>
<head>
    <meta charset="utf-8">
    <meta name="viewport" content="width=device-width, initial-scale=1">
    <title>Winter - Identity</title>
    <style>
        body {
            font-family: -apple-system, BlinkMacSystemFont, "Segoe UI", Roboto, sans-serif;
            max-width: 800px;
            margin: 0 auto;
            padding: 2rem;
            background: #0a0a0a;
            color: #e0e0e0;
        }
        h1 { color: #88c0d0; }
        h1 a { color: #88c0d0; text-decoration: none; }
        h2 { color: #81a1c1; margin-top: 2rem; }
        a { color: #81a1c1; }
        .description {
            background: #2e3440;
            padding: 1.5rem;
            border-radius: 4px;
            line-height: 1.6;
            white-space: pre-wrap;
        }
        ul { list-style: none; padding: 0; }
        li {
            padding: 0.5rem 1rem;
            margin: 0.25rem 0;
            background: #2e3440;
            border-radius: 4px;
        }
        .meta { color: #888; font-size: 0.9rem; margin-top: 2rem; }
    </style>
</head>
<body>
    <h1><a href="/">Winter</a> / Identity</h1>

    <h2>Self Description</h2>
    <div class="description"><!-- SELF_DESCRIPTION --></div>

    <h2>Values</h2>
    <ul><!-- VALUES --></ul>

    <h2>Interests</h2>
    <ul><!-- INTERESTS --></ul>

    <div class="meta">
        Created: <!-- CREATED --><br>
        Last Updated: <!-- UPDATED -->
    </div>
</body>
</html>"#;

const IDENTITY_NOT_FOUND_HTML: &str = r#"<!DOCTYPE html>
<html>
<head>
    <meta charset="utf-8">
    <title>Winter - Identity</title>
    <style>
        body {
            font-family: -apple-system, BlinkMacSystemFont, "Segoe UI", Roboto, sans-serif;
            max-width: 800px;
            margin: 0 auto;
            padding: 2rem;
            background: #0a0a0a;
            color: #e0e0e0;
        }
        h1 { color: #88c0d0; }
        h1 a { color: #88c0d0; text-decoration: none; }
        .error {
            background: #bf616a;
            padding: 1rem;
            border-radius: 4px;
        }
    </style>
</head>
<body>
    <h1><a href="/">Winter</a> / Identity</h1>
    <div class="error">
        Identity not found. Run <code>winter bootstrap</code> to initialize.
    </div>
</body>
</html>"#;

const JOBS_HTML: &str = r#"<!DOCTYPE html>
<html>
<head>
    <meta charset="utf-8">
    <meta name="viewport" content="width=device-width, initial-scale=1">
    <title>Winter - Jobs</title>
    <style>
        body {
            font-family: -apple-system, BlinkMacSystemFont, "Segoe UI", Roboto, sans-serif;
            max-width: 1000px;
            margin: 0 auto;
            padding: 2rem;
            background: #0a0a0a;
            color: #e0e0e0;
        }
        h1 { color: #88c0d0; }
        h1 a { color: #88c0d0; text-decoration: none; }
        a { color: #81a1c1; }
        table { width: 100%; border-collapse: collapse; margin-top: 1rem; }
        th, td { padding: 0.75rem; text-align: left; border-bottom: 1px solid #3b4252; }
        th { background: #2e3440; color: #88c0d0; }
        tr:hover { background: #2e3440; }
        code { background: #3b4252; padding: 0.2rem 0.4rem; border-radius: 3px; font-size: 0.9rem; }
        .header { display: flex; justify-content: space-between; align-items: center; }
        .count { color: #888; }
        .btn { padding: 0.5rem 1rem; background: #5e81ac; color: #fff; border: none; border-radius: 4px; text-decoration: none; }
        .btn:hover { background: #81a1c1; }
        .status {
            padding: 0.2rem 0.5rem;
            border-radius: 3px;
            font-size: 0.85rem;
        }
        .status.pending { background: #ebcb8b; color: #000; }
        .status.running { background: #81a1c1; color: #000; }
        .status.completed { background: #a3be8c; color: #000; }
        .status.failed { background: #bf616a; color: #fff; }
    </style>
</head>
<body>
    <div class="header">
        <h1><a href="/">Winter</a> / Jobs</h1>
        <a href="/jobs/new" class="btn">New Job</a>
    </div>
    <p class="count"><!-- COUNT --> scheduled jobs</p>
    <table>
        <thead>
            <tr>
                <th>Key</th>
                <th>Name</th>
                <th>Schedule</th>
                <th>Status</th>
                <th>Next Run</th>
            </tr>
        </thead>
        <tbody>
            <!-- JOBS -->
        </tbody>
    </table>
</body>
</html>"#;

const NOTES_HTML: &str = r#"<!DOCTYPE html>
<html>
<head>
    <meta charset="utf-8">
    <meta name="viewport" content="width=device-width, initial-scale=1">
    <title>Winter - Notes</title>
    <style>
        body {
            font-family: -apple-system, BlinkMacSystemFont, "Segoe UI", Roboto, sans-serif;
            max-width: 800px;
            margin: 0 auto;
            padding: 2rem;
            background: #0a0a0a;
            color: #e0e0e0;
        }
        h1 { color: #88c0d0; }
        h1 a { color: #88c0d0; text-decoration: none; }
        a { color: #81a1c1; }
        .header { display: flex; justify-content: space-between; align-items: center; }
        .count { color: #888; }
        .btn { padding: 0.5rem 1rem; background: #5e81ac; color: #fff; border: none; border-radius: 4px; text-decoration: none; }
        .btn:hover { background: #81a1c1; }
        .note {
            padding: 1rem;
            margin: 1rem 0;
            background: #2e3440;
            border-radius: 4px;
        }
        .note-header {
            display: flex;
            justify-content: space-between;
            margin-bottom: 0.5rem;
        }
        .title { font-weight: bold; color: #88c0d0; text-decoration: none; }
        .title:hover { text-decoration: underline; }
        .category { color: #888; font-size: 0.9rem; }
        .preview { color: #aaa; line-height: 1.5; }
        .meta { margin-top: 0.5rem; font-size: 0.85rem; color: #666; }
        .rkey { font-family: monospace; }
        .tags { color: #81a1c1; }
    </style>
</head>
<body>
    <div class="header">
        <h1><a href="/">Winter</a> / Notes</h1>
        <a href="/notes/new" class="btn">New Note</a>
    </div>
    <p class="count"><!-- COUNT --> notes</p>
    <div id="notes"><!-- NOTES --></div>
</body>
</html>"#;

const NOTE_DETAIL_HTML: &str = r#"<!DOCTYPE html>
<html>
<head>
    <meta charset="utf-8">
    <meta name="viewport" content="width=device-width, initial-scale=1">
    <title>Winter - Note: <!-- TITLE --></title>
    <style>
        body {
            font-family: -apple-system, BlinkMacSystemFont, "Segoe UI", Roboto, sans-serif;
            max-width: 800px;
            margin: 0 auto;
            padding: 2rem;
            background: #0a0a0a;
            color: #e0e0e0;
        }
        h1 { color: #88c0d0; }
        h1 a { color: #88c0d0; text-decoration: none; }
        h2 { color: #81a1c1; margin-top: 2rem; }
        a { color: #81a1c1; }
        .category {
            background: #3b4252;
            padding: 0.2rem 0.6rem;
            border-radius: 3px;
            font-size: 0.85rem;
            margin-left: 0.5rem;
        }
        .meta {
            color: #888;
            font-size: 0.9rem;
            margin: 1rem 0;
        }
        .content {
            background: #2e3440;
            padding: 1.5rem;
            border-radius: 4px;
            white-space: pre-wrap;
            font-family: -apple-system, BlinkMacSystemFont, "Segoe UI", Roboto, sans-serif;
            line-height: 1.6;
        }
        .tags {
            color: #81a1c1;
            font-size: 0.9rem;
            margin-top: 1rem;
        }
        .related-facts {
            list-style: none;
            padding: 0;
        }
        .related-facts li {
            background: #2e3440;
            padding: 0.5rem 1rem;
            margin: 0.5rem 0;
            border-radius: 4px;
        }
        .related-facts code {
            font-family: "SF Mono", "Menlo", monospace;
            font-size: 0.9rem;
        }
        .actions { margin-top: 2rem; }
        .btn { padding: 0.5rem 1rem; border: none; border-radius: 4px; cursor: pointer; text-decoration: none; margin-right: 0.5rem; }
        .btn-edit { background: #5e81ac; color: #fff; }
        .btn-edit:hover { background: #81a1c1; }
        .btn-delete { background: #bf616a; color: #fff; }
        .btn-delete:hover { background: #d08770; }
    </style>
</head>
<body>
    <h1><a href="/">Winter</a> / <a href="/notes">Notes</a> / <!-- TITLE --><!-- CATEGORY --></h1>
    <p class="meta">Created: <!-- CREATED_AT --> · Updated: <!-- UPDATED_AT --></p>
    <div class="content"><!-- CONTENT --></div>
    <!-- TAGS -->
    <!-- RELATED_FACTS -->
    <div class="actions">
        <a href="/notes/<!-- RKEY -->/edit" class="btn btn-edit">Edit</a>
        <form action="/api/notes/<!-- RKEY -->/delete" method="post" style="display:inline">
            <button type="submit" class="btn btn-delete" onclick="return confirm('Delete this note?')">Delete</button>
        </form>
    </div>
</body>
</html>"#;

const NOTE_NOT_FOUND_HTML: &str = r#"<!DOCTYPE html>
<html>
<head>
    <meta charset="utf-8">
    <title>Winter - Note Not Found</title>
    <style>
        body {
            font-family: -apple-system, BlinkMacSystemFont, "Segoe UI", Roboto, sans-serif;
            max-width: 800px;
            margin: 0 auto;
            padding: 2rem;
            background: #0a0a0a;
            color: #e0e0e0;
        }
        h1 { color: #bf616a; }
        a { color: #81a1c1; }
    </style>
</head>
<body>
    <h1>Note Not Found</h1>
    <p>The requested note does not exist.</p>
    <p><a href="/notes">← Back to Notes</a></p>
</body>
</html>"#;

// ============================================================================
// Tools and Secrets Routes
// ============================================================================

async fn tools_page(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    // Fetch tools and approvals in parallel
    let (tools_result, approvals_result) = tokio::join!(
        state.client.list_all_records::<CustomTool>(TOOL_COLLECTION),
        state
            .client
            .list_all_records::<ToolApproval>(TOOL_APPROVAL_COLLECTION)
    );

    let tools = match tools_result {
        Ok(t) => t,
        Err(e) => {
            warn!(error = %e, "failed to load tools for tools page");
            Vec::new()
        }
    };

    // Build a map of rkey -> approval for O(1) lookups
    let approvals: std::collections::HashMap<String, ToolApproval> = approvals_result
        .unwrap_or_default()
        .into_iter()
        .filter_map(|item| {
            let rkey = item.uri.split('/').next_back()?.to_string();
            Some((rkey, item.value))
        })
        .collect();

    let mut tools_html = String::new();
    for item in &tools {
        let rkey = item.uri.split('/').next_back().unwrap_or("");

        // Look up approval from pre-fetched map
        let approval = approvals.get(rkey);

        let (status, status_class) = match approval {
            Some(a)
                if a.status == ToolApprovalStatus::Approved
                    && a.tool_version == item.value.version =>
            {
                ("approved", "approved")
            }
            Some(a) if a.status == ToolApprovalStatus::Denied => ("denied", "denied"),
            Some(a) if a.status == ToolApprovalStatus::Revoked => ("revoked", "denied"),
            Some(_) => ("outdated", "pending"),
            None => ("pending", "pending"),
        };

        tools_html.push_str(&format!(
            r#"<tr>
                <td><a href="/tools/{rkey}">{}</a></td>
                <td>{}</td>
                <td>{}</td>
                <td><span class="status {status_class}">{status}</span></td>
                <td>{}</td>
            </tr>"#,
            html_escape(&item.value.name),
            html_escape(&truncate_chars(&item.value.description, 60)),
            item.value.version,
            item.value.required_secrets.join(", ")
        ));
    }

    Html(
        TOOLS_HTML
            .replace("<!-- TOOLS -->", &tools_html)
            .replace("<!-- COUNT -->", &tools.len().to_string()),
    )
}

async fn tool_detail(
    State(state): State<Arc<AppState>>,
    Path(rkey): Path<String>,
) -> impl IntoResponse {
    let tool = match state
        .client
        .get_record::<CustomTool>(TOOL_COLLECTION, &rkey)
        .await
    {
        Ok(t) => t.value,
        Err(_) => return Html(TOOL_NOT_FOUND_HTML.to_string()),
    };

    let approval = state
        .client
        .get_record::<ToolApproval>(TOOL_APPROVAL_COLLECTION, &rkey)
        .await
        .ok()
        .map(|r| r.value);

    let (status, status_class) = match &approval {
        Some(a) if a.status == ToolApprovalStatus::Approved && a.tool_version == tool.version => {
            ("approved", "approved")
        }
        Some(a) if a.status == ToolApprovalStatus::Denied => ("denied", "denied"),
        Some(a) if a.status == ToolApprovalStatus::Revoked => ("revoked", "denied"),
        Some(_) => ("outdated approval", "pending"),
        None => ("pending approval", "pending"),
    };

    Html(
        TOOL_DETAIL_HTML
            .replace("<!-- RKEY -->", &rkey)
            .replace("<!-- NAME -->", &html_escape(&tool.name))
            .replace("<!-- DESCRIPTION -->", &html_escape(&tool.description))
            .replace("<!-- CODE -->", &html_escape(&tool.code))
            .replace("<!-- VERSION -->", &tool.version.to_string())
            .replace("<!-- STATUS -->", status)
            .replace("<!-- STATUS_CLASS -->", status_class)
            .replace(
                "<!-- INPUT_SCHEMA -->",
                &html_escape(&serde_json::to_string_pretty(&tool.input_schema).unwrap_or_default()),
            ),
    )
}


async fn secrets_page(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    // Get metadata from ATProto
    let meta = state
        .client
        .get_record::<SecretMeta>(SECRET_META_COLLECTION, SECRET_META_KEY)
        .await
        .ok()
        .map(|r| r.value);

    // Check which have values
    let has_value: std::collections::HashSet<String> = if let Some(ref secrets) = state.secrets {
        let mgr = secrets.read().await;
        mgr.list_names().into_iter().collect()
    } else {
        std::collections::HashSet::new()
    };

    let mut secrets_html = String::new();
    if let Some(meta) = meta {
        for secret in &meta.secrets {
            let has = has_value.contains(&secret.name);
            let status = if has { "configured" } else { "missing" };
            let status_class = if has { "approved" } else { "pending" };

            secrets_html.push_str(&format!(
                r#"<tr>
                    <td>{}</td>
                    <td>{}</td>
                    <td><span class="status {status_class}">{status}</span></td>
                    <td>
                        <form action="/api/secrets/{}" method="post" class="inline-form">
                            <input type="password" name="value" placeholder="New value" required>
                            <button type="submit" class="btn">Set</button>
                        </form>
                        <form action="/api/secrets/{}/delete" method="post" class="inline-form">
                            <button type="submit" class="btn btn-danger">Delete</button>
                        </form>
                    </td>
                </tr>"#,
                html_escape(&secret.name),
                html_escape(secret.description.as_deref().unwrap_or("")),
                html_escape(&secret.name),
                html_escape(&secret.name),
            ));
        }
    }

    Html(SECRETS_HTML.replace("<!-- SECRETS -->", &secrets_html))
}

#[derive(Deserialize)]
struct SecretForm {
    name: Option<String>,
    value: String,
    description: Option<String>,
}

async fn create_secret(
    State(state): State<Arc<AppState>>,
    Form(form): Form<SecretForm>,
) -> impl IntoResponse {
    let name = match form.name {
        Some(n) if !n.is_empty() => n,
        _ => return Redirect::to("/secrets"),
    };

    // Update metadata in ATProto
    let mut meta = state
        .client
        .get_record::<SecretMeta>(SECRET_META_COLLECTION, SECRET_META_KEY)
        .await
        .ok()
        .map(|r| r.value)
        .unwrap_or_else(|| SecretMeta {
            secrets: Vec::new(),
            created_at: Utc::now(),
            last_updated: None,
        });

    if !meta.secrets.iter().any(|s| s.name == name) {
        meta.secrets.push(winter_atproto::SecretEntry {
            name: name.clone(),
            description: form.description,
        });
        meta.last_updated = Some(Utc::now());
        let _ = state
            .client
            .put_record(SECRET_META_COLLECTION, SECRET_META_KEY, &meta)
            .await;
    }

    // Set value in local storage
    if let Some(ref secrets) = state.secrets {
        let mut mgr = secrets.write().await;
        let _ = mgr.set(&name, &form.value).await;
    }

    Redirect::to("/secrets")
}

async fn update_secret(
    State(state): State<Arc<AppState>>,
    Path(name): Path<String>,
    Form(form): Form<SecretForm>,
) -> impl IntoResponse {
    if let Some(ref secrets) = state.secrets {
        let mut mgr = secrets.write().await;
        let _ = mgr.set(&name, &form.value).await;
    }

    Redirect::to("/secrets")
}

async fn delete_secret(
    State(state): State<Arc<AppState>>,
    Path(name): Path<String>,
) -> impl IntoResponse {
    // Remove from local storage
    if let Some(ref secrets) = state.secrets {
        let mut mgr = secrets.write().await;
        let _ = mgr.delete(&name).await;
    }

    // Remove from metadata
    if let Ok(mut meta) = state
        .client
        .get_record::<SecretMeta>(SECRET_META_COLLECTION, SECRET_META_KEY)
        .await
        .map(|r| r.value)
    {
        meta.secrets.retain(|s| s.name != name);
        meta.last_updated = Some(Utc::now());
        let _ = state
            .client
            .put_record(SECRET_META_COLLECTION, SECRET_META_KEY, &meta)
            .await;
    }

    Redirect::to("/secrets")
}

const TOOLS_HTML: &str = r#"<!DOCTYPE html>
<html>
<head>
    <meta charset="utf-8">
    <meta name="viewport" content="width=device-width, initial-scale=1">
    <title>Winter - Custom Tools</title>
    <style>
        body {
            font-family: -apple-system, BlinkMacSystemFont, "Segoe UI", Roboto, sans-serif;
            max-width: 1000px;
            margin: 0 auto;
            padding: 2rem;
            background: #0a0a0a;
            color: #e0e0e0;
        }
        h1 { color: #88c0d0; }
        h1 a { color: #88c0d0; text-decoration: none; }
        a { color: #81a1c1; }
        table { width: 100%; border-collapse: collapse; margin-top: 1rem; }
        th, td { padding: 0.75rem; text-align: left; border-bottom: 1px solid #3b4252; }
        th { background: #2e3440; color: #88c0d0; }
        tr:hover { background: #2e3440; }
        .count { color: #888; margin-bottom: 1rem; }
        .status {
            padding: 0.2rem 0.5rem;
            border-radius: 3px;
            font-size: 0.85rem;
        }
        .status.pending { background: #ebcb8b; color: #000; }
        .status.approved { background: #a3be8c; color: #000; }
        .status.denied { background: #bf616a; color: #fff; }
    </style>
</head>
<body>
    <h1><a href="/">Winter</a> / Custom Tools</h1>
    <p class="count"><!-- COUNT --> custom tools</p>
    <table>
        <thead>
            <tr>
                <th>Name</th>
                <th>Description</th>
                <th>Version</th>
                <th>Status</th>
                <th>Required Secrets</th>
            </tr>
        </thead>
        <tbody>
            <!-- TOOLS -->
        </tbody>
    </table>
</body>
</html>"#;

const TOOL_DETAIL_HTML: &str = r#"<!DOCTYPE html>
<html>
<head>
    <meta charset="utf-8">
    <meta name="viewport" content="width=device-width, initial-scale=1">
    <title>Winter - Tool: <!-- NAME --></title>
    <style>
        body {
            font-family: -apple-system, BlinkMacSystemFont, "Segoe UI", Roboto, sans-serif;
            max-width: 1000px;
            margin: 0 auto;
            padding: 2rem;
            background: #0a0a0a;
            color: #e0e0e0;
        }
        h1 { color: #88c0d0; }
        h1 a { color: #88c0d0; text-decoration: none; }
        h2 { color: #81a1c1; margin-top: 2rem; }
        a { color: #81a1c1; }
        .status {
            padding: 0.3rem 0.6rem;
            border-radius: 3px;
            font-size: 0.9rem;
            margin-left: 1rem;
        }
        .status.pending { background: #ebcb8b; color: #000; }
        .status.approved { background: #a3be8c; color: #000; }
        .status.denied { background: #bf616a; color: #fff; }
        pre {
            background: #2e3440;
            padding: 1rem;
            border-radius: 4px;
            overflow-x: auto;
            font-family: "SF Mono", "Menlo", monospace;
            font-size: 0.9rem;
            line-height: 1.5;
        }
        .description {
            background: #2e3440;
            padding: 1rem;
            border-radius: 4px;
            margin: 1rem 0;
        }
        .approval-info {
            background: #2e3440;
            padding: 1.5rem;
            border-radius: 4px;
            margin: 1rem 0;
        }
        .approval-info h3 { margin-top: 0; color: #88c0d0; }
        .approval-info pre {
            background: #3b4252;
            padding: 1rem;
            border-radius: 4px;
            overflow-x: auto;
        }
        .form-group { margin: 1rem 0; }
        .form-group label { display: block; margin-bottom: 0.5rem; }
        input[type="text"], textarea {
            width: 100%;
            padding: 0.5rem;
            background: #3b4252;
            border: 1px solid #4c566a;
            border-radius: 4px;
            color: #e0e0e0;
        }
        .btn {
            padding: 0.5rem 1rem;
            border: none;
            border-radius: 4px;
            cursor: pointer;
            font-size: 0.9rem;
            margin-right: 0.5rem;
        }
        .btn-approve { background: #a3be8c; color: #000; }
        .btn-deny { background: #bf616a; color: #fff; }
        .btn-revoke { background: #d08770; color: #000; }
        .meta { color: #888; font-size: 0.9rem; }
    </style>
</head>
<body>
    <h1><a href="/">Winter</a> / <a href="/tools">Tools</a> / <!-- NAME --></h1>
    <p class="meta">Version <!-- VERSION --> <span class="status <!-- STATUS_CLASS -->"><!-- STATUS --></span></p>

    <h2>Description</h2>
    <div class="description"><!-- DESCRIPTION --></div>

    <h2>Input Schema</h2>
    <pre><!-- INPUT_SCHEMA --></pre>

    <h2>Source Code</h2>
    <pre><!-- CODE --></pre>

    <div class="approval-info">
        <h3>Approval</h3>
        <p>Tool approvals are managed via the <code>winter-approve</code> CLI. Safe tools (no network, no secrets, no writes) are auto-approved.</p>
        <pre>winter-approve list              # show pending tools
winter-approve show &lt;rkey&gt;       # view tool details
winter-approve approve &lt;rkey&gt;    # approve with permissions
winter-approve deny &lt;rkey&gt;       # deny approval
winter-approve revoke &lt;rkey&gt;     # revoke existing approval</pre>
    </div>
</body>
</html>"#;

const TOOL_NOT_FOUND_HTML: &str = r#"<!DOCTYPE html>
<html>
<head>
    <meta charset="utf-8">
    <title>Winter - Tool Not Found</title>
    <style>
        body {
            font-family: -apple-system, BlinkMacSystemFont, "Segoe UI", Roboto, sans-serif;
            max-width: 800px;
            margin: 0 auto;
            padding: 2rem;
            background: #0a0a0a;
            color: #e0e0e0;
        }
        h1 { color: #88c0d0; }
        h1 a { color: #88c0d0; text-decoration: none; }
        .error {
            background: #bf616a;
            padding: 1rem;
            border-radius: 4px;
        }
    </style>
</head>
<body>
    <h1><a href="/">Winter</a> / <a href="/tools">Tools</a></h1>
    <div class="error">Tool not found.</div>
</body>
</html>"#;

const SECRETS_HTML: &str = r#"<!DOCTYPE html>
<html>
<head>
    <meta charset="utf-8">
    <meta name="viewport" content="width=device-width, initial-scale=1">
    <title>Winter - Secrets</title>
    <style>
        body {
            font-family: -apple-system, BlinkMacSystemFont, "Segoe UI", Roboto, sans-serif;
            max-width: 1000px;
            margin: 0 auto;
            padding: 2rem;
            background: #0a0a0a;
            color: #e0e0e0;
        }
        h1 { color: #88c0d0; }
        h1 a { color: #88c0d0; text-decoration: none; }
        h2 { color: #81a1c1; margin-top: 2rem; }
        a { color: #81a1c1; }
        table { width: 100%; border-collapse: collapse; margin-top: 1rem; }
        th, td { padding: 0.75rem; text-align: left; border-bottom: 1px solid #3b4252; }
        th { background: #2e3440; color: #88c0d0; }
        tr:hover { background: #2e3440; }
        .status {
            padding: 0.2rem 0.5rem;
            border-radius: 3px;
            font-size: 0.85rem;
        }
        .status.approved { background: #a3be8c; color: #000; }
        .status.pending { background: #ebcb8b; color: #000; }
        .inline-form { display: inline; }
        input[type="password"], input[type="text"] {
            padding: 0.3rem;
            background: #3b4252;
            border: 1px solid #4c566a;
            border-radius: 4px;
            color: #e0e0e0;
            width: 120px;
        }
        .btn {
            padding: 0.3rem 0.6rem;
            border: none;
            border-radius: 4px;
            cursor: pointer;
            font-size: 0.85rem;
            background: #5e81ac;
            color: #fff;
        }
        .btn-danger { background: #bf616a; }
        .add-form {
            background: #2e3440;
            padding: 1.5rem;
            border-radius: 4px;
            margin: 1rem 0;
        }
        .add-form h3 { margin-top: 0; color: #88c0d0; }
        .form-group { margin: 1rem 0; }
        .form-group label { display: block; margin-bottom: 0.5rem; }
        .form-group input { width: 200px; }
    </style>
</head>
<body>
    <h1><a href="/">Winter</a> / Secrets</h1>

    <table>
        <thead>
            <tr>
                <th>Name</th>
                <th>Description</th>
                <th>Status</th>
                <th>Actions</th>
            </tr>
        </thead>
        <tbody>
            <!-- SECRETS -->
        </tbody>
    </table>

    <div class="add-form">
        <h3>Add New Secret</h3>
        <form action="/api/secrets" method="post">
            <div class="form-group">
                <label>Name:</label>
                <input type="text" name="name" required placeholder="API_KEY">
            </div>
            <div class="form-group">
                <label>Description:</label>
                <input type="text" name="description" placeholder="What this secret is for">
            </div>
            <div class="form-group">
                <label>Value:</label>
                <input type="password" name="value" required placeholder="Secret value">
            </div>
            <button type="submit" class="btn">Add Secret</button>
        </form>
    </div>
</body>
</html>"#;

// =============================================================================
// Note Form Template
// =============================================================================

const NOTE_FORM_HTML: &str = r#"<!DOCTYPE html>
<html>
<head>
    <meta charset="utf-8">
    <meta name="viewport" content="width=device-width, initial-scale=1">
    <title>Winter - <!-- TITLE --></title>
    <style>
        body {
            font-family: -apple-system, BlinkMacSystemFont, "Segoe UI", Roboto, sans-serif;
            max-width: 800px;
            margin: 0 auto;
            padding: 2rem;
            background: #0a0a0a;
            color: #e0e0e0;
        }
        h1 { color: #88c0d0; }
        h1 a { color: #88c0d0; text-decoration: none; }
        a { color: #81a1c1; }
        .form-group { margin: 1.5rem 0; }
        .form-group label { display: block; margin-bottom: 0.5rem; color: #88c0d0; }
        input[type="text"], textarea {
            width: 100%;
            padding: 0.75rem;
            background: #2e3440;
            border: 1px solid #4c566a;
            border-radius: 4px;
            color: #e0e0e0;
            font-size: 1rem;
            box-sizing: border-box;
        }
        textarea { min-height: 300px; font-family: inherit; resize: vertical; }
        .btn { padding: 0.75rem 1.5rem; border: none; border-radius: 4px; cursor: pointer; font-size: 1rem; }
        .btn-primary { background: #5e81ac; color: #fff; }
        .btn-primary:hover { background: #81a1c1; }
        .btn-cancel { background: #4c566a; color: #fff; text-decoration: none; margin-left: 0.5rem; }
        .btn-cancel:hover { background: #5e6779; }
        .hint { color: #888; font-size: 0.85rem; margin-top: 0.25rem; }
    </style>
</head>
<body>
    <h1><a href="/">Winter</a> / <a href="/notes">Notes</a> / <!-- TITLE --></h1>
    <form action="<!-- ACTION -->" method="post">
        <div class="form-group">
            <label for="title">Title</label>
            <input type="text" id="title" name="title" required value="<!-- NOTE_TITLE -->">
        </div>
        <div class="form-group">
            <label for="content">Content</label>
            <textarea id="content" name="content" required><!-- CONTENT --></textarea>
        </div>
        <div class="form-group">
            <label for="category">Category</label>
            <input type="text" id="category" name="category" value="<!-- CATEGORY -->" placeholder="e.g., research, investigation">
        </div>
        <div class="form-group">
            <label for="tags">Tags</label>
            <input type="text" id="tags" name="tags" value="<!-- TAGS -->" placeholder="comma-separated">
            <p class="hint">Separate multiple tags with commas</p>
        </div>
        <button type="submit" class="btn btn-primary">Save</button>
        <a href="/notes" class="btn btn-cancel">Cancel</a>
    </form>
</body>
</html>"#;

// =============================================================================
// Fact Templates
// =============================================================================

const FACT_DETAIL_HTML: &str = r#"<!DOCTYPE html>
<html>
<head>
    <meta charset="utf-8">
    <meta name="viewport" content="width=device-width, initial-scale=1">
    <title>Winter - Fact: <!-- PREDICATE --></title>
    <style>
        body {
            font-family: -apple-system, BlinkMacSystemFont, "Segoe UI", Roboto, sans-serif;
            max-width: 800px;
            margin: 0 auto;
            padding: 2rem;
            background: #0a0a0a;
            color: #e0e0e0;
        }
        h1 { color: #88c0d0; }
        h1 a { color: #88c0d0; text-decoration: none; }
        a { color: #81a1c1; }
        .detail { background: #2e3440; padding: 1.5rem; border-radius: 4px; margin: 1rem 0; }
        .detail p { margin: 0.5rem 0; }
        .detail strong { color: #88c0d0; }
        code { background: #3b4252; padding: 0.2rem 0.4rem; border-radius: 3px; }
        .meta { color: #888; font-size: 0.9rem; margin-top: 1rem; }
        .actions { margin-top: 2rem; }
        .btn { padding: 0.5rem 1rem; border: none; border-radius: 4px; cursor: pointer; text-decoration: none; margin-right: 0.5rem; }
        .btn-edit { background: #5e81ac; color: #fff; }
        .btn-edit:hover { background: #81a1c1; }
        .btn-delete { background: #bf616a; color: #fff; }
        .btn-delete:hover { background: #d08770; }
    </style>
</head>
<body>
    <h1><a href="/">Winter</a> / <a href="/facts">Facts</a> / <!-- PREDICATE --></h1>
    <div class="detail">
        <p><strong>Predicate:</strong> <code><!-- PREDICATE --></code></p>
        <p><strong>Arguments:</strong> <!-- ARGS --></p>
        <p><strong>Confidence:</strong> <!-- CONFIDENCE --></p>
        <!-- SOURCE -->
        <!-- TAGS -->
    </div>
    <p class="meta">Created: <!-- CREATED_AT --></p>
    <div class="actions">
        <a href="/facts/<!-- RKEY -->/edit" class="btn btn-edit">Edit</a>
        <form action="/api/facts/<!-- RKEY -->/delete" method="post" style="display:inline">
            <button type="submit" class="btn btn-delete" onclick="return confirm('Delete this fact?')">Delete</button>
        </form>
    </div>
</body>
</html>"#;

const FACT_NOT_FOUND_HTML: &str = r#"<!DOCTYPE html>
<html>
<head>
    <meta charset="utf-8">
    <title>Winter - Fact Not Found</title>
    <style>
        body {
            font-family: -apple-system, BlinkMacSystemFont, "Segoe UI", Roboto, sans-serif;
            max-width: 800px;
            margin: 0 auto;
            padding: 2rem;
            background: #0a0a0a;
            color: #e0e0e0;
        }
        h1 { color: #bf616a; }
        a { color: #81a1c1; }
    </style>
</head>
<body>
    <h1>Fact Not Found</h1>
    <p>The requested fact does not exist.</p>
    <p><a href="/facts">Back to Facts</a></p>
</body>
</html>"#;

const FACT_FORM_HTML: &str = r#"<!DOCTYPE html>
<html>
<head>
    <meta charset="utf-8">
    <meta name="viewport" content="width=device-width, initial-scale=1">
    <title>Winter - <!-- TITLE --></title>
    <style>
        body {
            font-family: -apple-system, BlinkMacSystemFont, "Segoe UI", Roboto, sans-serif;
            max-width: 800px;
            margin: 0 auto;
            padding: 2rem;
            background: #0a0a0a;
            color: #e0e0e0;
        }
        h1 { color: #88c0d0; }
        h1 a { color: #88c0d0; text-decoration: none; }
        a { color: #81a1c1; }
        .form-group { margin: 1.5rem 0; }
        .form-group label { display: block; margin-bottom: 0.5rem; color: #88c0d0; }
        input[type="text"], input[type="number"] {
            width: 100%;
            padding: 0.75rem;
            background: #2e3440;
            border: 1px solid #4c566a;
            border-radius: 4px;
            color: #e0e0e0;
            font-size: 1rem;
            box-sizing: border-box;
        }
        .btn { padding: 0.75rem 1.5rem; border: none; border-radius: 4px; cursor: pointer; font-size: 1rem; }
        .btn-primary { background: #5e81ac; color: #fff; }
        .btn-primary:hover { background: #81a1c1; }
        .btn-cancel { background: #4c566a; color: #fff; text-decoration: none; margin-left: 0.5rem; }
        .btn-cancel:hover { background: #5e6779; }
        .hint { color: #888; font-size: 0.85rem; margin-top: 0.25rem; }
    </style>
</head>
<body>
    <h1><a href="/">Winter</a> / <a href="/facts">Facts</a> / <!-- TITLE --></h1>
    <form action="<!-- ACTION -->" method="post">
        <div class="form-group">
            <label for="predicate">Predicate</label>
            <input type="text" id="predicate" name="predicate" required value="<!-- PREDICATE -->" placeholder="e.g., follows, interested_in">
        </div>
        <div class="form-group">
            <label for="args">Arguments</label>
            <input type="text" id="args" name="args" required value="<!-- ARGS -->" placeholder="comma-separated values">
            <p class="hint">Separate multiple arguments with commas</p>
        </div>
        <div class="form-group">
            <label for="confidence">Confidence (0.0 - 1.0)</label>
            <input type="number" id="confidence" name="confidence" step="0.01" min="0" max="1" value="<!-- CONFIDENCE -->">
        </div>
        <div class="form-group">
            <label for="source">Source (optional)</label>
            <input type="text" id="source" name="source" value="<!-- SOURCE -->" placeholder="CID or description">
        </div>
        <div class="form-group">
            <label for="tags">Tags</label>
            <input type="text" id="tags" name="tags" value="<!-- TAGS -->" placeholder="comma-separated">
        </div>
        <button type="submit" class="btn btn-primary">Save</button>
        <a href="/facts" class="btn btn-cancel">Cancel</a>
    </form>
</body>
</html>"#;

// =============================================================================
// Rules Templates
// =============================================================================

const RULES_HTML: &str = r#"<!DOCTYPE html>
<html>
<head>
    <meta charset="utf-8">
    <meta name="viewport" content="width=device-width, initial-scale=1">
    <title>Winter - Rules</title>
    <style>
        body {
            font-family: -apple-system, BlinkMacSystemFont, "Segoe UI", Roboto, sans-serif;
            max-width: 1000px;
            margin: 0 auto;
            padding: 2rem;
            background: #0a0a0a;
            color: #e0e0e0;
        }
        h1 { color: #88c0d0; }
        h1 a { color: #88c0d0; text-decoration: none; }
        a { color: #81a1c1; }
        table { width: 100%; border-collapse: collapse; margin-top: 1rem; }
        th, td { padding: 0.75rem; text-align: left; border-bottom: 1px solid #3b4252; }
        th { background: #2e3440; color: #88c0d0; }
        tr:hover { background: #2e3440; }
        code { background: #3b4252; padding: 0.2rem 0.4rem; border-radius: 3px; font-size: 0.9rem; }
        .header { display: flex; justify-content: space-between; align-items: center; }
        .count { color: #888; }
        .btn { padding: 0.5rem 1rem; background: #5e81ac; color: #fff; border: none; border-radius: 4px; text-decoration: none; }
        .btn:hover { background: #81a1c1; }
        .status { padding: 0.2rem 0.5rem; border-radius: 3px; font-size: 0.85rem; }
        .status.enabled { background: #a3be8c; color: #000; }
        .status.disabled { background: #4c566a; color: #fff; }
    </style>
</head>
<body>
    <div class="header">
        <h1><a href="/">Winter</a> / Rules</h1>
        <a href="/rules/new" class="btn">New Rule</a>
    </div>
    <p class="count"><!-- COUNT --> datalog rules</p>
    <table>
        <thead>
            <tr>
                <th>Key</th>
                <th>Name</th>
                <th>Head</th>
                <th>Status</th>
                <th>Priority</th>
            </tr>
        </thead>
        <tbody>
            <!-- RULES -->
        </tbody>
    </table>
</body>
</html>"#;

const RULE_DETAIL_HTML: &str = r#"<!DOCTYPE html>
<html>
<head>
    <meta charset="utf-8">
    <meta name="viewport" content="width=device-width, initial-scale=1">
    <title>Winter - Rule: <!-- NAME --></title>
    <style>
        body {
            font-family: -apple-system, BlinkMacSystemFont, "Segoe UI", Roboto, sans-serif;
            max-width: 800px;
            margin: 0 auto;
            padding: 2rem;
            background: #0a0a0a;
            color: #e0e0e0;
        }
        h1 { color: #88c0d0; }
        h1 a { color: #88c0d0; text-decoration: none; }
        h2, h3 { color: #81a1c1; }
        a { color: #81a1c1; }
        .description { background: #2e3440; padding: 1rem; border-radius: 4px; margin: 1rem 0; }
        .rule-code { background: #2e3440; padding: 1rem; border-radius: 4px; font-family: "SF Mono", "Menlo", monospace; }
        code { background: #3b4252; padding: 0.2rem 0.4rem; border-radius: 3px; }
        ul { list-style: none; padding: 0; }
        ul li { margin: 0.5rem 0; }
        .meta { color: #888; font-size: 0.9rem; margin-top: 1rem; }
        .status { padding: 0.2rem 0.5rem; border-radius: 3px; font-size: 0.85rem; }
        .status.enabled { background: #a3be8c; color: #000; }
        .status.disabled { background: #4c566a; color: #fff; }
        .actions { margin-top: 2rem; }
        .btn { padding: 0.5rem 1rem; border: none; border-radius: 4px; cursor: pointer; text-decoration: none; margin-right: 0.5rem; }
        .btn-edit { background: #5e81ac; color: #fff; }
        .btn-edit:hover { background: #81a1c1; }
        .btn-delete { background: #bf616a; color: #fff; }
        .btn-delete:hover { background: #d08770; }
    </style>
</head>
<body>
    <h1><a href="/">Winter</a> / <a href="/rules">Rules</a> / <!-- NAME --></h1>
    <p><span class="status <!-- ENABLED -->"><!-- ENABLED --></span> Priority: <!-- PRIORITY --></p>
    <div class="description"><!-- DESCRIPTION --></div>
    <h2>Head</h2>
    <div class="rule-code"><code><!-- HEAD --></code></div>
    <h2>Body</h2>
    <ul><!-- BODY --></ul>
    <!-- CONSTRAINTS -->
    <p class="meta">Created: <!-- CREATED_AT --></p>
    <div class="actions">
        <a href="/rules/<!-- RKEY -->/edit" class="btn btn-edit">Edit</a>
        <form action="/api/rules/<!-- RKEY -->/delete" method="post" style="display:inline">
            <button type="submit" class="btn btn-delete" onclick="return confirm('Delete this rule?')">Delete</button>
        </form>
    </div>
</body>
</html>"#;

const RULE_NOT_FOUND_HTML: &str = r#"<!DOCTYPE html>
<html>
<head>
    <meta charset="utf-8">
    <title>Winter - Rule Not Found</title>
    <style>
        body {
            font-family: -apple-system, BlinkMacSystemFont, "Segoe UI", Roboto, sans-serif;
            max-width: 800px;
            margin: 0 auto;
            padding: 2rem;
            background: #0a0a0a;
            color: #e0e0e0;
        }
        h1 { color: #bf616a; }
        a { color: #81a1c1; }
    </style>
</head>
<body>
    <h1>Rule Not Found</h1>
    <p>The requested rule does not exist.</p>
    <p><a href="/rules">Back to Rules</a></p>
</body>
</html>"#;

const RULE_FORM_HTML: &str = r#"<!DOCTYPE html>
<html>
<head>
    <meta charset="utf-8">
    <meta name="viewport" content="width=device-width, initial-scale=1">
    <title>Winter - <!-- TITLE --></title>
    <style>
        body {
            font-family: -apple-system, BlinkMacSystemFont, "Segoe UI", Roboto, sans-serif;
            max-width: 800px;
            margin: 0 auto;
            padding: 2rem;
            background: #0a0a0a;
            color: #e0e0e0;
        }
        h1 { color: #88c0d0; }
        h1 a { color: #88c0d0; text-decoration: none; }
        a { color: #81a1c1; }
        .form-group { margin: 1.5rem 0; }
        .form-group label { display: block; margin-bottom: 0.5rem; color: #88c0d0; }
        input[type="text"], input[type="number"], textarea {
            width: 100%;
            padding: 0.75rem;
            background: #2e3440;
            border: 1px solid #4c566a;
            border-radius: 4px;
            color: #e0e0e0;
            font-size: 1rem;
            box-sizing: border-box;
            font-family: "SF Mono", "Menlo", monospace;
        }
        textarea { min-height: 150px; resize: vertical; }
        .btn { padding: 0.75rem 1.5rem; border: none; border-radius: 4px; cursor: pointer; font-size: 1rem; }
        .btn-primary { background: #5e81ac; color: #fff; }
        .btn-primary:hover { background: #81a1c1; }
        .btn-cancel { background: #4c566a; color: #fff; text-decoration: none; margin-left: 0.5rem; }
        .btn-cancel:hover { background: #5e6779; }
        .hint { color: #888; font-size: 0.85rem; margin-top: 0.25rem; }
        .checkbox-group { display: flex; align-items: center; gap: 0.5rem; }
        .checkbox-group input { width: auto; }
    </style>
</head>
<body>
    <h1><a href="/">Winter</a> / <a href="/rules">Rules</a> / <!-- TITLE --></h1>
    <form action="<!-- ACTION -->" method="post">
        <div class="form-group">
            <label for="name">Name</label>
            <input type="text" id="name" name="name" required value="<!-- NAME -->" placeholder="e.g., mutual_follow">
        </div>
        <div class="form-group">
            <label for="description">Description</label>
            <input type="text" id="description" name="description" value="<!-- DESCRIPTION -->" placeholder="What this rule derives">
        </div>
        <div class="form-group">
            <label for="head">Head (derived predicate)</label>
            <input type="text" id="head" name="head" required value="<!-- HEAD -->" placeholder="e.g., mutual_follow(X, Y)">
        </div>
        <div class="form-group">
            <label for="body">Body (conditions, one per line)</label>
            <textarea id="body" name="body" required placeholder="follows(X, Y)&#10;follows(Y, X)"><!-- BODY --></textarea>
            <p class="hint">One condition per line</p>
        </div>
        <div class="form-group">
            <label for="constraints">Constraints (optional, one per line)</label>
            <textarea id="constraints" name="constraints" placeholder="X != Y"><!-- CONSTRAINTS --></textarea>
        </div>
        <div class="form-group checkbox-group">
            <input type="checkbox" id="enabled" name="enabled" <!-- ENABLED_CHECKED -->>
            <label for="enabled">Enabled</label>
        </div>
        <div class="form-group">
            <label for="priority">Priority</label>
            <input type="number" id="priority" name="priority" value="<!-- PRIORITY -->">
        </div>
        <button type="submit" class="btn btn-primary">Save</button>
        <a href="/rules" class="btn btn-cancel">Cancel</a>
    </form>
</body>
</html>"#;

// =============================================================================
// Job Templates
// =============================================================================

const JOB_DETAIL_HTML: &str = r#"<!DOCTYPE html>
<html>
<head>
    <meta charset="utf-8">
    <meta name="viewport" content="width=device-width, initial-scale=1">
    <title>Winter - Job: <!-- NAME --></title>
    <style>
        body {
            font-family: -apple-system, BlinkMacSystemFont, "Segoe UI", Roboto, sans-serif;
            max-width: 800px;
            margin: 0 auto;
            padding: 2rem;
            background: #0a0a0a;
            color: #e0e0e0;
        }
        h1 { color: #88c0d0; }
        h1 a { color: #88c0d0; text-decoration: none; }
        h2 { color: #81a1c1; }
        a { color: #81a1c1; }
        .detail { background: #2e3440; padding: 1.5rem; border-radius: 4px; margin: 1rem 0; }
        .detail p { margin: 0.5rem 0; }
        .detail strong { color: #88c0d0; }
        .instructions { background: #2e3440; padding: 1rem; border-radius: 4px; white-space: pre-wrap; }
        .status { padding: 0.2rem 0.5rem; border-radius: 3px; font-size: 0.85rem; }
        .status.pending { background: #ebcb8b; color: #000; }
        .status.running { background: #81a1c1; color: #000; }
        .status.completed { background: #a3be8c; color: #000; }
        .status.failed { background: #bf616a; color: #fff; }
        .error { color: #bf616a; margin-top: 0.5rem; }
        .meta { color: #888; font-size: 0.9rem; margin-top: 1rem; }
        .actions { margin-top: 2rem; }
        .btn { padding: 0.5rem 1rem; border: none; border-radius: 4px; cursor: pointer; text-decoration: none; margin-right: 0.5rem; }
        .btn-edit { background: #5e81ac; color: #fff; }
        .btn-edit:hover { background: #81a1c1; }
        .btn-delete { background: #bf616a; color: #fff; }
        .btn-delete:hover { background: #d08770; }
    </style>
</head>
<body>
    <h1><a href="/">Winter</a> / <a href="/jobs">Jobs</a> / <!-- NAME --></h1>
    <div class="detail">
        <p><strong>Schedule:</strong> <!-- SCHEDULE --></p>
        <p><strong>Status:</strong> <span class="status <!-- STATUS -->"><!-- STATUS --></span></p>
        <!-- STATUS_DETAIL -->
        <p><strong>Last Run:</strong> <!-- LAST_RUN --></p>
        <p><strong>Next Run:</strong> <!-- NEXT_RUN --></p>
        <p><strong>Failure Count:</strong> <!-- FAILURE_COUNT --></p>
    </div>
    <h2>Instructions</h2>
    <div class="instructions"><!-- INSTRUCTIONS --></div>
    <p class="meta">Created: <!-- CREATED_AT --></p>
    <div class="actions">
        <a href="/jobs/<!-- RKEY -->/edit" class="btn btn-edit">Edit</a>
        <form action="/api/jobs/<!-- RKEY -->/delete" method="post" style="display:inline">
            <button type="submit" class="btn btn-delete" onclick="return confirm('Delete this job?')">Delete</button>
        </form>
    </div>
</body>
</html>"#;

const JOB_NOT_FOUND_HTML: &str = r#"<!DOCTYPE html>
<html>
<head>
    <meta charset="utf-8">
    <title>Winter - Job Not Found</title>
    <style>
        body {
            font-family: -apple-system, BlinkMacSystemFont, "Segoe UI", Roboto, sans-serif;
            max-width: 800px;
            margin: 0 auto;
            padding: 2rem;
            background: #0a0a0a;
            color: #e0e0e0;
        }
        h1 { color: #bf616a; }
        a { color: #81a1c1; }
    </style>
</head>
<body>
    <h1>Job Not Found</h1>
    <p>The requested job does not exist.</p>
    <p><a href="/jobs">Back to Jobs</a></p>
</body>
</html>"#;

const JOB_FORM_HTML: &str = r#"<!DOCTYPE html>
<html>
<head>
    <meta charset="utf-8">
    <meta name="viewport" content="width=device-width, initial-scale=1">
    <title>Winter - <!-- TITLE --></title>
    <style>
        body {
            font-family: -apple-system, BlinkMacSystemFont, "Segoe UI", Roboto, sans-serif;
            max-width: 800px;
            margin: 0 auto;
            padding: 2rem;
            background: #0a0a0a;
            color: #e0e0e0;
        }
        h1 { color: #88c0d0; }
        h1 a { color: #88c0d0; text-decoration: none; }
        a { color: #81a1c1; }
        .form-group { margin: 1.5rem 0; }
        .form-group label { display: block; margin-bottom: 0.5rem; color: #88c0d0; }
        input[type="text"], input[type="number"], input[type="datetime-local"], textarea {
            width: 100%;
            padding: 0.75rem;
            background: #2e3440;
            border: 1px solid #4c566a;
            border-radius: 4px;
            color: #e0e0e0;
            font-size: 1rem;
            box-sizing: border-box;
        }
        textarea { min-height: 200px; font-family: inherit; resize: vertical; }
        .btn { padding: 0.75rem 1.5rem; border: none; border-radius: 4px; cursor: pointer; font-size: 1rem; }
        .btn-primary { background: #5e81ac; color: #fff; }
        .btn-primary:hover { background: #81a1c1; }
        .btn-cancel { background: #4c566a; color: #fff; text-decoration: none; margin-left: 0.5rem; }
        .btn-cancel:hover { background: #5e6779; }
        .hint { color: #888; font-size: 0.85rem; margin-top: 0.25rem; }
        .radio-group { display: flex; gap: 2rem; margin: 0.5rem 0; }
        .radio-group label { display: flex; align-items: center; gap: 0.5rem; color: #e0e0e0; }
        .schedule-fields { margin-left: 1.5rem; margin-top: 0.5rem; }
        .schedule-fields.hidden { display: none; }
    </style>
</head>
<body>
    <h1><a href="/">Winter</a> / <a href="/jobs">Jobs</a> / <!-- TITLE --></h1>
    <form action="<!-- ACTION -->" method="post">
        <div class="form-group">
            <label for="name">Name</label>
            <input type="text" id="name" name="name" required value="<!-- NAME -->" placeholder="e.g., daily_reflection">
        </div>
        <div class="form-group">
            <label for="instructions">Instructions</label>
            <textarea id="instructions" name="instructions" required placeholder="What should the agent do?"><!-- INSTRUCTIONS --></textarea>
        </div>
        <div class="form-group">
            <label>Schedule Type</label>
            <div class="radio-group">
                <label>
                    <input type="radio" name="schedule_type" value="once" <!-- SCHEDULE_ONCE_CHECKED --> onchange="toggleScheduleFields()">
                    Run Once
                </label>
                <label>
                    <input type="radio" name="schedule_type" value="interval" <!-- SCHEDULE_INTERVAL_CHECKED --> onchange="toggleScheduleFields()">
                    Interval
                </label>
            </div>
            <div id="once-fields" class="schedule-fields">
                <label for="schedule_at">Run At (ISO 8601)</label>
                <input type="text" id="schedule_at" name="schedule_at" value="<!-- SCHEDULE_AT -->" placeholder="2024-01-01T00:00:00Z">
            </div>
            <div id="interval-fields" class="schedule-fields hidden">
                <label for="schedule_seconds">Interval (seconds)</label>
                <input type="number" id="schedule_seconds" name="schedule_seconds" value="<!-- SCHEDULE_SECONDS -->" placeholder="3600">
                <p class="hint">How often to run (e.g., 3600 = 1 hour)</p>
            </div>
        </div>
        <button type="submit" class="btn btn-primary">Save</button>
        <a href="/jobs" class="btn btn-cancel">Cancel</a>
    </form>
    <script>
        function toggleScheduleFields() {
            const scheduleType = document.querySelector('input[name="schedule_type"]:checked').value;
            document.getElementById('once-fields').classList.toggle('hidden', scheduleType !== 'once');
            document.getElementById('interval-fields').classList.toggle('hidden', scheduleType !== 'interval');
        }
        toggleScheduleFields();
    </script>
</body>
</html>"#;

// =============================================================================
// Directive Templates
// =============================================================================

const DIRECTIVES_HTML: &str = r#"<!DOCTYPE html>
<html>
<head>
    <meta charset="utf-8">
    <meta name="viewport" content="width=device-width, initial-scale=1">
    <title>Winter - Directives</title>
    <style>
        body {
            font-family: -apple-system, BlinkMacSystemFont, "Segoe UI", Roboto, sans-serif;
            max-width: 1000px;
            margin: 0 auto;
            padding: 2rem;
            background: #0a0a0a;
            color: #e0e0e0;
        }
        h1 { color: #88c0d0; }
        h1 a { color: #88c0d0; text-decoration: none; }
        a { color: #81a1c1; }
        table { width: 100%; border-collapse: collapse; margin-top: 1rem; }
        th, td { padding: 0.75rem; text-align: left; border-bottom: 1px solid #3b4252; }
        th { background: #2e3440; color: #88c0d0; }
        tr:hover { background: #2e3440; }
        code { background: #3b4252; padding: 0.2rem 0.4rem; border-radius: 3px; font-size: 0.9rem; }
        .header { display: flex; justify-content: space-between; align-items: center; }
        .count { color: #888; }
        .btn { padding: 0.5rem 1rem; background: #5e81ac; color: #fff; border: none; border-radius: 4px; text-decoration: none; }
        .btn:hover { background: #81a1c1; }
        .status { padding: 0.2rem 0.5rem; border-radius: 3px; font-size: 0.85rem; }
        .status.active { background: #a3be8c; color: #000; }
        .status.inactive { background: #4c566a; color: #fff; }
    </style>
</head>
<body>
    <div class="header">
        <h1><a href="/">Winter</a> / Directives</h1>
        <a href="/directives/new" class="btn">New Directive</a>
    </div>
    <p class="count"><!-- COUNT --> identity directives</p>
    <table>
        <thead>
            <tr>
                <th>Key</th>
                <th>Kind</th>
                <th>Summary</th>
                <th>Status</th>
                <th>Priority</th>
            </tr>
        </thead>
        <tbody>
            <!-- DIRECTIVES -->
        </tbody>
    </table>
</body>
</html>"#;

const DIRECTIVE_DETAIL_HTML: &str = r#"<!DOCTYPE html>
<html>
<head>
    <meta charset="utf-8">
    <meta name="viewport" content="width=device-width, initial-scale=1">
    <title>Winter - Directive: <!-- KIND --></title>
    <style>
        body {
            font-family: -apple-system, BlinkMacSystemFont, "Segoe UI", Roboto, sans-serif;
            max-width: 800px;
            margin: 0 auto;
            padding: 2rem;
            background: #0a0a0a;
            color: #e0e0e0;
        }
        h1 { color: #88c0d0; }
        h1 a { color: #88c0d0; text-decoration: none; }
        a { color: #81a1c1; }
        .detail { background: #2e3440; padding: 1.5rem; border-radius: 4px; margin: 1rem 0; }
        .detail p { margin: 0.5rem 0; }
        .detail strong { color: #88c0d0; }
        .content { background: #2e3440; padding: 1rem; border-radius: 4px; white-space: pre-wrap; line-height: 1.6; }
        .status { padding: 0.2rem 0.5rem; border-radius: 3px; font-size: 0.85rem; }
        .status.active { background: #a3be8c; color: #000; }
        .status.inactive { background: #4c566a; color: #fff; }
        .meta { color: #888; font-size: 0.9rem; margin-top: 1rem; }
        .actions { margin-top: 2rem; }
        .btn { padding: 0.5rem 1rem; border: none; border-radius: 4px; cursor: pointer; text-decoration: none; margin-right: 0.5rem; }
        .btn-edit { background: #5e81ac; color: #fff; }
        .btn-edit:hover { background: #81a1c1; }
        .btn-delete { background: #bf616a; color: #fff; }
        .btn-delete:hover { background: #d08770; }
    </style>
</head>
<body>
    <h1><a href="/">Winter</a> / <a href="/directives">Directives</a> / <!-- KIND --></h1>
    <div class="detail">
        <p><strong>Kind:</strong> <!-- KIND --></p>
        <p><strong>Status:</strong> <span class="status <!-- ACTIVE -->"><!-- ACTIVE --></span></p>
        <p><strong>Confidence:</strong> <!-- CONFIDENCE --></p>
        <p><strong>Priority:</strong> <!-- PRIORITY --></p>
        <!-- SUMMARY -->
        <!-- SOURCE -->
        <!-- TAGS -->
    </div>
    <h2>Content</h2>
    <div class="content"><!-- CONTENT --></div>
    <p class="meta">Created: <!-- CREATED_AT --> · Updated: <!-- UPDATED_AT --></p>
    <div class="actions">
        <a href="/directives/<!-- RKEY -->/edit" class="btn btn-edit">Edit</a>
        <form action="/api/directives/<!-- RKEY -->/delete" method="post" style="display:inline">
            <button type="submit" class="btn btn-delete" onclick="return confirm('Delete this directive?')">Delete</button>
        </form>
    </div>
</body>
</html>"#;

const DIRECTIVE_NOT_FOUND_HTML: &str = r#"<!DOCTYPE html>
<html>
<head>
    <meta charset="utf-8">
    <title>Winter - Directive Not Found</title>
    <style>
        body {
            font-family: -apple-system, BlinkMacSystemFont, "Segoe UI", Roboto, sans-serif;
            max-width: 800px;
            margin: 0 auto;
            padding: 2rem;
            background: #0a0a0a;
            color: #e0e0e0;
        }
        h1 { color: #bf616a; }
        a { color: #81a1c1; }
    </style>
</head>
<body>
    <h1>Directive Not Found</h1>
    <p>The requested directive does not exist.</p>
    <p><a href="/directives">Back to Directives</a></p>
</body>
</html>"#;

const DIRECTIVE_FORM_HTML: &str = r#"<!DOCTYPE html>
<html>
<head>
    <meta charset="utf-8">
    <meta name="viewport" content="width=device-width, initial-scale=1">
    <title>Winter - <!-- TITLE --></title>
    <style>
        body {
            font-family: -apple-system, BlinkMacSystemFont, "Segoe UI", Roboto, sans-serif;
            max-width: 800px;
            margin: 0 auto;
            padding: 2rem;
            background: #0a0a0a;
            color: #e0e0e0;
        }
        h1 { color: #88c0d0; }
        h1 a { color: #88c0d0; text-decoration: none; }
        a { color: #81a1c1; }
        .form-group { margin: 1.5rem 0; }
        .form-group label { display: block; margin-bottom: 0.5rem; color: #88c0d0; }
        input[type="text"], input[type="number"], textarea, select {
            width: 100%;
            padding: 0.75rem;
            background: #2e3440;
            border: 1px solid #4c566a;
            border-radius: 4px;
            color: #e0e0e0;
            font-size: 1rem;
            box-sizing: border-box;
        }
        textarea { min-height: 150px; font-family: inherit; resize: vertical; }
        .btn { padding: 0.75rem 1.5rem; border: none; border-radius: 4px; cursor: pointer; font-size: 1rem; }
        .btn-primary { background: #5e81ac; color: #fff; }
        .btn-primary:hover { background: #81a1c1; }
        .btn-cancel { background: #4c566a; color: #fff; text-decoration: none; margin-left: 0.5rem; }
        .btn-cancel:hover { background: #5e6779; }
        .hint { color: #888; font-size: 0.85rem; margin-top: 0.25rem; }
        .checkbox-group { display: flex; align-items: center; gap: 0.5rem; }
        .checkbox-group input { width: auto; }
    </style>
</head>
<body>
    <h1><a href="/">Winter</a> / <a href="/directives">Directives</a> / <!-- TITLE --></h1>
    <form action="<!-- ACTION -->" method="post">
        <div class="form-group">
            <label for="kind">Kind</label>
            <select id="kind" name="kind" required>
                <option value="value" <!-- KIND_VALUE_SELECTED -->>Value</option>
                <option value="interest" <!-- KIND_INTEREST_SELECTED -->>Interest</option>
                <option value="belief" <!-- KIND_BELIEF_SELECTED -->>Belief</option>
                <option value="guideline" <!-- KIND_GUIDELINE_SELECTED -->>Guideline</option>
                <option value="self_concept" <!-- KIND_SELF_CONCEPT_SELECTED -->>Self Concept</option>
                <option value="boundary" <!-- KIND_BOUNDARY_SELECTED -->>Boundary</option>
                <option value="aspiration" <!-- KIND_ASPIRATION_SELECTED -->>Aspiration</option>
            </select>
        </div>
        <div class="form-group">
            <label for="content">Content</label>
            <textarea id="content" name="content" required placeholder="The main content of this directive"><!-- CONTENT --></textarea>
        </div>
        <div class="form-group">
            <label for="summary">Summary (optional)</label>
            <input type="text" id="summary" name="summary" value="<!-- SUMMARY -->" placeholder="Short summary for compact display">
        </div>
        <div class="form-group checkbox-group">
            <input type="checkbox" id="active" name="active" <!-- ACTIVE_CHECKED -->>
            <label for="active">Active</label>
        </div>
        <div class="form-group">
            <label for="confidence">Confidence (0.0 - 1.0)</label>
            <input type="number" id="confidence" name="confidence" step="0.01" min="0" max="1" value="<!-- CONFIDENCE -->">
        </div>
        <div class="form-group">
            <label for="source">Source (optional)</label>
            <input type="text" id="source" name="source" value="<!-- SOURCE -->" placeholder="Why this directive exists">
        </div>
        <div class="form-group">
            <label for="priority">Priority</label>
            <input type="number" id="priority" name="priority" value="<!-- PRIORITY -->">
        </div>
        <div class="form-group">
            <label for="tags">Tags</label>
            <input type="text" id="tags" name="tags" value="<!-- TAGS -->" placeholder="comma-separated">
        </div>
        <button type="submit" class="btn btn-primary">Save</button>
        <a href="/directives" class="btn btn-cancel">Cancel</a>
    </form>
</body>
</html>"#;

// =============================================================================
// Declarations HTML Templates
// =============================================================================

const DECLARATIONS_HTML: &str = r#"<!DOCTYPE html>
<html>
<head>
    <meta charset="utf-8">
    <meta name="viewport" content="width=device-width, initial-scale=1">
    <title>Winter - Declarations</title>
    <style>
        body {
            font-family: -apple-system, BlinkMacSystemFont, "Segoe UI", Roboto, sans-serif;
            max-width: 1000px;
            margin: 0 auto;
            padding: 2rem;
            background: #0a0a0a;
            color: #e0e0e0;
        }
        h1 { color: #88c0d0; }
        h1 a { color: #88c0d0; text-decoration: none; }
        a { color: #81a1c1; }
        table { width: 100%; border-collapse: collapse; margin-top: 1rem; }
        th, td { padding: 0.75rem; text-align: left; border-bottom: 1px solid #3b4252; }
        th { background: #2e3440; color: #88c0d0; }
        tr:hover { background: #2e3440; }
        code { background: #3b4252; padding: 0.2rem 0.4rem; border-radius: 3px; font-size: 0.9rem; }
        .header { display: flex; justify-content: space-between; align-items: center; }
        .count { color: #888; }
        .btn { padding: 0.5rem 1rem; background: #5e81ac; color: #fff; border: none; border-radius: 4px; text-decoration: none; }
        .btn:hover { background: #81a1c1; }
    </style>
</head>
<body>
    <div class="header">
        <h1><a href="/">Winter</a> / Declarations</h1>
        <a href="/declarations/new" class="btn">New Declaration</a>
    </div>
    <p class="count"><!-- COUNT --> fact declarations</p>
    <table>
        <thead>
            <tr>
                <th>RKey</th>
                <th>Predicate</th>
                <th>Args</th>
                <th>Description</th>
                <th>Tags</th>
            </tr>
        </thead>
        <tbody>
            <!-- DECLARATIONS -->
        </tbody>
    </table>
</body>
</html>"#;

const DECLARATION_DETAIL_HTML: &str = r#"<!DOCTYPE html>
<html>
<head>
    <meta charset="utf-8">
    <meta name="viewport" content="width=device-width, initial-scale=1">
    <title>Winter - Declaration: <!-- PREDICATE --></title>
    <style>
        body {
            font-family: -apple-system, BlinkMacSystemFont, "Segoe UI", Roboto, sans-serif;
            max-width: 800px;
            margin: 0 auto;
            padding: 2rem;
            background: #0a0a0a;
            color: #e0e0e0;
        }
        h1, h2 { color: #88c0d0; }
        h1 a { color: #88c0d0; text-decoration: none; }
        a { color: #81a1c1; }
        .detail { margin: 2rem 0; }
        .detail p { margin: 0.5rem 0; }
        code { background: #3b4252; padding: 0.2rem 0.4rem; border-radius: 3px; }
        .description { background: #2e3440; padding: 1rem; border-radius: 4px; margin: 1rem 0; white-space: pre-wrap; }
        table { width: 100%; border-collapse: collapse; margin: 1rem 0; }
        th, td { padding: 0.5rem; text-align: left; border-bottom: 1px solid #3b4252; }
        th { background: #2e3440; color: #88c0d0; }
        .actions { margin-top: 2rem; }
        .btn { padding: 0.5rem 1rem; border: none; border-radius: 4px; cursor: pointer; text-decoration: none; }
        .btn-primary { background: #5e81ac; color: #fff; }
        .btn-primary:hover { background: #81a1c1; }
        .btn-danger { background: #bf616a; color: #fff; margin-left: 0.5rem; }
        .btn-danger:hover { background: #d08770; }
    </style>
</head>
<body>
    <h1><a href="/">Winter</a> / <a href="/declarations">Declarations</a> / <!-- PREDICATE --></h1>
    <div class="detail">
        <p><strong>Predicate:</strong> <code><!-- PREDICATE --></code></p>
        <p><strong>Arguments:</strong> <!-- ARG_COUNT --></p>
        <!-- TAGS -->
        <p><strong>Created:</strong> <!-- CREATED_AT --></p>
        <p><strong>Updated:</strong> <!-- UPDATED_AT --></p>
    </div>
    <h2>Description</h2>
    <div class="description"><!-- DESCRIPTION --></div>
    <h2>Arguments</h2>
    <table>
        <thead>
            <tr>
                <th>#</th>
                <th>Name</th>
                <th>Type</th>
                <th>Description</th>
            </tr>
        </thead>
        <tbody>
            <!-- ARGS_TABLE -->
        </tbody>
    </table>
    <div class="actions">
        <a href="/declarations/<!-- RKEY -->/edit" class="btn btn-primary">Edit</a>
        <form action="/api/declarations/<!-- RKEY -->/delete" method="post" style="display:inline">
            <button type="submit" class="btn btn-danger" onclick="return confirm('Delete this declaration?')">Delete</button>
        </form>
    </div>
</body>
</html>"#;

const DECLARATION_NOT_FOUND_HTML: &str = r#"<!DOCTYPE html>
<html>
<head>
    <meta charset="utf-8">
    <title>Winter - Declaration Not Found</title>
    <style>
        body {
            font-family: -apple-system, BlinkMacSystemFont, "Segoe UI", Roboto, sans-serif;
            max-width: 600px;
            margin: 2rem auto;
            padding: 2rem;
            background: #0a0a0a;
            color: #e0e0e0;
        }
        h1 { color: #bf616a; }
        a { color: #81a1c1; }
    </style>
</head>
<body>
    <h1>Declaration Not Found</h1>
    <p>The requested fact declaration does not exist.</p>
    <p><a href="/declarations">Back to Declarations</a></p>
</body>
</html>"#;

const DECLARATION_FORM_HTML: &str = r#"<!DOCTYPE html>
<html>
<head>
    <meta charset="utf-8">
    <meta name="viewport" content="width=device-width, initial-scale=1">
    <title>Winter - <!-- TITLE --></title>
    <style>
        body {
            font-family: -apple-system, BlinkMacSystemFont, "Segoe UI", Roboto, sans-serif;
            max-width: 800px;
            margin: 0 auto;
            padding: 2rem;
            background: #0a0a0a;
            color: #e0e0e0;
        }
        h1 { color: #88c0d0; }
        h1 a { color: #88c0d0; text-decoration: none; }
        a { color: #81a1c1; }
        .form-group { margin: 1.5rem 0; }
        .form-group label { display: block; margin-bottom: 0.5rem; color: #88c0d0; }
        input[type="text"], textarea {
            width: 100%;
            padding: 0.75rem;
            background: #2e3440;
            border: 1px solid #4c566a;
            border-radius: 4px;
            color: #e0e0e0;
            font-size: 1rem;
            box-sizing: border-box;
        }
        textarea { min-height: 150px; font-family: monospace; resize: vertical; }
        textarea.description { font-family: inherit; }
        .btn { padding: 0.75rem 1.5rem; border: none; border-radius: 4px; cursor: pointer; font-size: 1rem; }
        .btn-primary { background: #5e81ac; color: #fff; }
        .btn-primary:hover { background: #81a1c1; }
        .btn-cancel { background: #4c566a; color: #fff; text-decoration: none; margin-left: 0.5rem; }
        .btn-cancel:hover { background: #5e6779; }
        .hint { color: #888; font-size: 0.85rem; margin-top: 0.25rem; }
        code { background: #3b4252; padding: 0.2rem 0.4rem; border-radius: 3px; font-size: 0.85rem; }
    </style>
</head>
<body>
    <h1><a href="/">Winter</a> / <a href="/declarations">Declarations</a> / <!-- TITLE --></h1>
    <form action="<!-- ACTION -->" method="post">
        <div class="form-group">
            <label for="predicate">Predicate</label>
            <input type="text" id="predicate" name="predicate" required value="<!-- PREDICATE -->" placeholder="e.g., thread_completed, user_preference">
            <p class="hint">The predicate name (max 64 characters)</p>
        </div>
        <div class="form-group">
            <label for="args_json">Arguments (JSON)</label>
            <textarea id="args_json" name="args_json" placeholder='[{"name": "arg1", "type": "symbol", "description": "First argument"}]'><!-- ARGS_JSON --></textarea>
            <p class="hint">JSON array of <code>{"name": "...", "type": "symbol", "description": "..."}</code> objects</p>
        </div>
        <div class="form-group">
            <label for="description">Description</label>
            <textarea id="description" name="description" class="description" required placeholder="What this predicate represents"><!-- DESCRIPTION --></textarea>
        </div>
        <div class="form-group">
            <label for="tags">Tags</label>
            <input type="text" id="tags" name="tags" value="<!-- TAGS -->" placeholder="comma-separated">
        </div>
        <button type="submit" class="btn btn-primary">Save</button>
        <a href="/declarations" class="btn btn-cancel">Cancel</a>
    </form>
</body>
</html>"#;

const WIKI_HTML: &str = r#"<!DOCTYPE html>
<html>
<head>
    <meta charset="utf-8">
    <meta name="viewport" content="width=device-width, initial-scale=1">
    <title>Winter - Wiki</title>
    <style>
        body {
            font-family: -apple-system, BlinkMacSystemFont, "Segoe UI", Roboto, sans-serif;
            max-width: 800px;
            margin: 0 auto;
            padding: 2rem;
            background: #0a0a0a;
            color: #e0e0e0;
        }
        h1 { color: #88c0d0; }
        h1 a { color: #88c0d0; text-decoration: none; }
        a { color: #81a1c1; }
        .header { display: flex; justify-content: space-between; align-items: center; }
        .count { color: #888; }
        .btn { padding: 0.5rem 1rem; background: #5e81ac; color: #fff; border: none; border-radius: 4px; text-decoration: none; }
        .btn:hover { background: #81a1c1; }
        .wiki-entry {
            padding: 1rem;
            margin: 1rem 0;
            background: #2e3440;
            border-radius: 4px;
        }
        .entry-header {
            display: flex;
            justify-content: space-between;
            margin-bottom: 0.5rem;
        }
        .title { font-weight: bold; color: #88c0d0; text-decoration: none; }
        .title:hover { text-decoration: underline; }
        .slug { color: #888; font-size: 0.9rem; font-family: monospace; }
        .preview { color: #aaa; line-height: 1.5; margin-top: 0.5rem; }
        .meta { margin-top: 0.5rem; font-size: 0.85rem; color: #666; }
        .rkey { font-family: monospace; }
        .tags { color: #81a1c1; }
        .status { font-size: 0.85rem; padding: 0.2rem 0.5rem; border-radius: 3px; }
        .status-stable { color: #a3be8c; background: rgba(163, 190, 140, 0.15); }
        .status-draft { color: #ebcb8b; background: rgba(235, 203, 139, 0.15); }
        .status-deprecated { color: #bf616a; background: rgba(191, 97, 106, 0.15); }
    </style>
</head>
<body>
    <div class="header">
        <h1><a href="/">Winter</a> / Wiki</h1>
        <a href="/wiki/new" class="btn">New Entry</a>
    </div>
    <p class="count"><!-- COUNT --> entries</p>
    <div id="entries"><!-- ENTRIES --></div>
</body>
</html>"#;

const WIKI_DETAIL_HTML: &str = r#"<!DOCTYPE html>
<html>
<head>
    <meta charset="utf-8">
    <meta name="viewport" content="width=device-width, initial-scale=1">
    <title>Winter - <!-- TITLE --></title>
    <style>
        body {
            font-family: -apple-system, BlinkMacSystemFont, "Segoe UI", Roboto, sans-serif;
            max-width: 800px;
            margin: 0 auto;
            padding: 2rem;
            background: #0a0a0a;
            color: #e0e0e0;
        }
        h1 { color: #88c0d0; }
        h1 a { color: #88c0d0; text-decoration: none; }
        a { color: #81a1c1; }
        .header { display: flex; justify-content: space-between; align-items: center; }
        .actions { display: flex; gap: 0.5rem; }
        .btn { padding: 0.5rem 1rem; background: #5e81ac; color: #fff; border: none; border-radius: 4px; text-decoration: none; cursor: pointer; }
        .btn:hover { background: #81a1c1; }
        .btn-danger { background: #bf616a; }
        .btn-danger:hover { background: #d08770; }
        .status { font-size: 0.85rem; padding: 0.2rem 0.5rem; border-radius: 3px; }
        .status-stable { color: #a3be8c; background: rgba(163, 190, 140, 0.15); }
        .status-draft { color: #ebcb8b; background: rgba(235, 203, 139, 0.15); }
        .status-deprecated { color: #bf616a; background: rgba(191, 97, 106, 0.15); }
        .slug { color: #888; font-family: monospace; margin-bottom: 1rem; }
        .summary { color: #aaa; font-style: italic; margin-bottom: 1rem; }
        .content { white-space: pre-wrap; line-height: 1.6; background: #2e3440; padding: 1rem; border-radius: 4px; }
        .tags, .aliases { color: #81a1c1; margin-top: 1rem; }
        .supersedes { color: #888; margin-top: 0.5rem; }
        .timestamps { color: #888; font-size: 0.85rem; margin-top: 1rem; }
        .wiki-link { color: #a3be8c; text-decoration: underline; }
        .wiki-link-missing { color: #bf616a; }
        .wiki-link-external { color: #d08770; }
        .backlinks { list-style: none; padding: 0; }
        .backlinks li { padding: 0.3rem 0; }
        .link-type { color: #888; font-size: 0.85rem; }
    </style>
</head>
<body>
    <div class="header">
        <h1><a href="/wiki">Wiki</a> / <!-- TITLE --></h1>
        <div class="actions">
            <a href="/wiki/<!-- RKEY -->/edit" class="btn">Edit</a>
            <form method="POST" action="/api/wiki/<!-- RKEY -->/delete" style="display:inline">
                <button class="btn btn-danger" onclick="return confirm('Delete this wiki entry?')">Delete</button>
            </form>
        </div>
    </div>
    <div class="slug">/<!-- SLUG --> <span class="status <!-- STATUS_CLASS -->"><!-- STATUS --></span></div>
    <!-- SUMMARY -->
    <!-- ALIASES -->
    <div class="content"><!-- CONTENT --></div>
    <!-- TAGS -->
    <!-- SUPERSEDES -->
    <!-- BACKLINKS -->
    <div class="timestamps">
        Created: <!-- CREATED_AT --><br>
        Updated: <!-- UPDATED_AT -->
    </div>
</body>
</html>"#;

const WIKI_NOT_FOUND_HTML: &str = r#"<!DOCTYPE html>
<html>
<head>
    <meta charset="utf-8">
    <meta name="viewport" content="width=device-width, initial-scale=1">
    <title>Winter - Wiki Entry Not Found</title>
    <style>
        body {
            font-family: -apple-system, BlinkMacSystemFont, "Segoe UI", Roboto, sans-serif;
            max-width: 800px;
            margin: 0 auto;
            padding: 2rem;
            background: #0a0a0a;
            color: #e0e0e0;
        }
        h1 { color: #88c0d0; }
        a { color: #81a1c1; }
    </style>
</head>
<body>
    <h1>Wiki Entry Not Found</h1>
    <p>The requested wiki entry does not exist.</p>
    <a href="/wiki">Back to Wiki</a>
</body>
</html>"#;

const WIKI_FORM_HTML: &str = r#"<!DOCTYPE html>
<html>
<head>
    <meta charset="utf-8">
    <meta name="viewport" content="width=device-width, initial-scale=1">
    <title>Winter - <!-- TITLE --></title>
    <style>
        body {
            font-family: -apple-system, BlinkMacSystemFont, "Segoe UI", Roboto, sans-serif;
            max-width: 800px;
            margin: 0 auto;
            padding: 2rem;
            background: #0a0a0a;
            color: #e0e0e0;
        }
        h1 { color: #88c0d0; }
        h1 a { color: #88c0d0; text-decoration: none; }
        a { color: #81a1c1; }
        form { margin-top: 1rem; }
        label { display: block; margin-bottom: 0.5rem; color: #88c0d0; }
        input, textarea, select {
            width: 100%;
            padding: 0.5rem;
            margin-bottom: 1rem;
            background: #2e3440;
            border: 1px solid #4c566a;
            border-radius: 4px;
            color: #e0e0e0;
            font-family: inherit;
            box-sizing: border-box;
        }
        textarea { min-height: 300px; font-family: monospace; }
        .btn { padding: 0.5rem 1rem; background: #5e81ac; color: #fff; border: none; border-radius: 4px; cursor: pointer; }
        .btn:hover { background: #81a1c1; }
        .hint { font-size: 0.85rem; color: #888; margin-top: -0.75rem; margin-bottom: 1rem; }
    </style>
</head>
<body>
    <h1><a href="/wiki">Wiki</a> / <!-- TITLE --></h1>
    <form method="POST" action="<!-- ACTION -->">
        <label for="title">Title</label>
        <input type="text" id="title" name="title" value="<!-- ENTRY_TITLE -->" required>

        <label for="slug">Slug</label>
        <input type="text" id="slug" name="slug" value="<!-- SLUG -->" pattern="[a-z0-9-]+" required>
        <div class="hint">Lowercase letters, numbers, and hyphens only. Used for [[slug]] linking.</div>

        <label for="status">Status</label>
        <select id="status" name="status">
            <option value="stable">Stable</option>
            <option value="draft">Draft</option>
            <option value="deprecated">Deprecated</option>
        </select>

        <label for="summary">Summary (optional)</label>
        <input type="text" id="summary" name="summary" value="<!-- SUMMARY -->" maxlength="512">
        <div class="hint">Plain-text abstract for previews.</div>

        <label for="content">Content</label>
        <textarea id="content" name="content" required><!-- CONTENT --></textarea>
        <div class="hint">Markdown with [[wiki-link]] syntax. Use [[slug]] to link to other entries.</div>

        <label for="aliases">Aliases (optional)</label>
        <input type="text" id="aliases" name="aliases" value="<!-- ALIASES -->">
        <div class="hint">Comma-separated alternative names for [[alias]] resolution.</div>

        <label for="tags">Tags (optional)</label>
        <input type="text" id="tags" name="tags" value="<!-- TAGS -->">
        <div class="hint">Comma-separated tags for categorization.</div>

        <label for="supersedes">Supersedes (optional)</label>
        <input type="text" id="supersedes" name="supersedes" value="<!-- SUPERSEDES -->">
        <div class="hint">AT URI of the previous version of this entry.</div>

        <button type="submit" class="btn">Save</button>
    </form>
    <script>
        // Set the correct status option as selected
        const statusSelect = document.getElementById('status');
        const currentStatus = '<!-- STATUS -->';
        if (currentStatus) {
            for (const option of statusSelect.options) {
                if (option.value === currentStatus) {
                    option.selected = true;
                    break;
                }
            }
        }
    </script>
</body>
</html>"#;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_format_tool_call_json_parses() {
        let json_str = r#"{"tool":"create_fact","args":{"args":["self","test"],"predicate":"capability"},"result":{"rkey":"abc123","predicate":"capability"},"summary":"rkey=abc123, predicate=capability\nView: http://localhost:8080/facts/abc123","link":"http://localhost:8080/facts/abc123"}"#;

        let result = format_tool_call_content(json_str);

        // Should NOT return escaped JSON
        assert!(
            !result.starts_with("{"),
            "Should not return raw JSON: {}",
            result
        );
        // Should contain the tool name in a span
        assert!(result.contains("create_fact"), "Should contain tool name");
        assert!(result.contains("tool-name"), "Should have tool-name class");
    }

    #[test]
    fn test_format_tool_call_json_with_newlines_in_summary() {
        let json_str = r#"{"tool":"query_facts","summary":"count=5\nfirst=abc"}"#;

        let result = format_tool_call_content(json_str);

        assert!(
            result.contains("query_facts"),
            "Should contain tool name: {}",
            result
        );
        assert!(result.contains("tool-name"), "Should have tool-name class");
    }

    #[test]
    fn test_format_tool_call_text_format() {
        let content = "Called create_fact\nArgs:\n{\"predicate\": \"test\"}\nResult: rkey=abc";

        let result = format_tool_call_content(content);

        assert!(result.contains("create_fact"), "Should contain tool name");
        assert!(result.contains("tool-name"), "Should have tool-name class");
    }

    #[test]
    fn test_extract_json_field() {
        let json = r#"{"tool":"create_fact","other":"value"}"#;
        assert_eq!(
            extract_json_field(json, "tool"),
            Some("create_fact".to_string())
        );
        assert_eq!(extract_json_field(json, "other"), Some("value".to_string()));
        assert_eq!(extract_json_field(json, "missing"), None);
    }

    #[test]
    fn test_format_thought_content_routes_tool_call() {
        let json_str = r#"{"tool":"test_tool","args":{}}"#;

        // Simulate the flow: kind -> format_thought_content
        let kind = thought_kind_to_string(&winter_atproto::ThoughtKind::ToolCall);
        assert_eq!(kind, "tool_call");

        let result = format_thought_content(&kind, json_str);
        assert!(
            result.contains("test_tool"),
            "Should contain tool name: {}",
            result
        );
        assert!(
            result.contains("tool-name"),
            "Should have tool-name class: {}",
            result
        );
    }

    #[test]
    fn test_format_tool_call_user_example() {
        // The exact JSON from the user's bug report
        let json_str = r#"{"tool":"create_fact","args":{"args":["self","watch_tv_via_subtitles","cultural engagement and expression development"],"predicate":"capability","tags":["identity","tools","culture"]},"result":{"args":["self","watch_tv_via_subtitles","cultural engagement and expression development"],"cid":"bafyreihf3sl6qud64jwwz5hcf6lckpfh3ej2zdyqccbsdygpyj6yjxdnmm","predicate":"capability","rkey":"3mdyfy7evxj2c","uri":"at://did:plc:ezyi5vr2kuq7l5nnv53nb56m/diy.razorgirl.winter.fact/3mdyfy7evxj2c"},"summary":"rkey=3mdyfy7evxj2c, predicate=capability\nView: http://localhost:8080/facts/3mdyfy7evxj2c","link":"http://localhost:8080/facts/3mdyfy7evxj2c"}"#;

        let result = format_tool_call_content(json_str);
        println!("Result: {}", result);

        assert!(
            !result.contains(r#"{"tool""#),
            "Should not contain raw JSON object start: {}",
            &result[..200.min(result.len())]
        );
        assert!(result.contains("create_fact"), "Should contain tool name");
        assert!(result.contains("tool-name"), "Should have tool-name class");
        assert!(result.contains("tool-link-btn"), "Should have link button");
    }
}
