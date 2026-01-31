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
    AtprotoClient, CustomTool, FACT_COLLECTION, Fact, IDENTITY_COLLECTION, IDENTITY_KEY, Identity,
    JOB_COLLECTION, Job, SECRET_META_COLLECTION, SECRET_META_KEY, SecretMeta, THOUGHT_COLLECTION,
    TOOL_APPROVAL_COLLECTION, TOOL_COLLECTION, Thought, ToolApproval, ToolApprovalStatus,
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
        .route("/facts", get(facts_page))
        .route("/identity", get(identity_page))
        .route("/jobs", get(jobs_page))
        .route("/notes", get(notes_page))
        .route("/tools", get(tools_page))
        .route("/tools/:rkey", get(tool_detail))
        .route("/api/tools/:rkey/approve", post(approve_tool))
        .route("/api/tools/:rkey/deny", post(deny_tool))
        .route("/api/tools/:rkey/revoke", post(revoke_tool))
        .route("/secrets", get(secrets_page))
        .route("/api/secrets", post(create_secret))
        .route("/api/secrets/:name", post(update_secret))
        .route("/api/secrets/:name/delete", post(delete_secret))
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
            .map(|t| format!(r#"<div class="trigger">{}</div>"#, html_escape(t)))
            .unwrap_or_default();

        thought_html.push_str(&format!(
            r#"<div class="thought {kind}">
                <div class="thought-header">
                    <span class="kind">{kind_display}</span>
                    <span class="time" title="{abs_time}">{rel_time}{duration_html}</span>
                </div>
                <div class="content">{content}</div>
                {trigger_html}
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
fn format_tool_call_content(content: &str) -> String {
    // Pattern: "Called tool_name [args] - FAILED" or "Called tool_name [args]"
    if let Some(rest) = content.strip_prefix("Called ") {
        // Find tool name (first word)
        let parts: Vec<&str> = rest.splitn(2, ' ').collect();
        if let Some(tool_name) = parts.first() {
            let tool_name = tool_name.trim();
            let mut html = format!(
                r#"<span class="tool-name">{}</span>"#,
                html_escape(tool_name)
            );

            if let Some(remainder) = parts.get(1) {
                // Extract args between [ ]
                if let (Some(args_start), Some(args_end)) =
                    (remainder.find('['), remainder.rfind(']'))
                {
                    let args = &remainder[args_start + 1..args_end];
                    if !args.is_empty() {
                        html.push_str(&format!(
                            r#" <span class="tool-args">{}</span>"#,
                            html_escape(args)
                        ));
                    }

                    // Check for FAILED suffix
                    let after_args = &remainder[args_end + 1..];
                    if after_args.contains("FAILED") {
                        html.push_str(r#" <span class="tool-failed">FAILED</span>"#);
                    }
                }
            }
            return html;
        }
    }

    html_escape(content)
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
            r#"<tr>
                <td><code>{}</code></td>
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

async fn identity_page(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    let identity = state
        .client
        .get_record::<Identity>(IDENTITY_COLLECTION, IDENTITY_KEY)
        .await;

    match identity {
        Ok(record) => {
            let id = &record.value;
            let values_html: String = id
                .values
                .iter()
                .map(|v| format!("<li>{}</li>", html_escape(v)))
                .collect();
            let interests_html: String = id
                .interests
                .iter()
                .map(|i| format!("<li>{}</li>", html_escape(i)))
                .collect();

            Html(
                IDENTITY_HTML
                    .replace(
                        "<!-- SELF_DESCRIPTION -->",
                        &html_escape(&id.self_description),
                    )
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
            winter_atproto::JobSchedule::Once { at } => format!("once at {}", at.to_rfc3339()),
            winter_atproto::JobSchedule::Interval { seconds } => format!("every {}s", seconds),
        };
        let status = format!("{:?}", item.value.status).to_lowercase();

        jobs_html.push_str(&format!(
            r#"<tr>
                <td><code>{}</code></td>
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
                .map(|dt| dt.to_rfc3339())
                .unwrap_or_else(|| "-".to_string())
        ));
    }

    Html(
        JOBS_HTML
            .replace("<!-- JOBS -->", &jobs_html)
            .replace("<!-- COUNT -->", &jobs.len().to_string()),
    )
}

async fn notes_page(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    let notes = match state
        .client
        .list_all_records::<winter_atproto::Note>(winter_atproto::NOTE_COLLECTION)
        .await
    {
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
                    <span class="title">{}</span>
                    <span class="category">{}</span>
                </div>
                <div class="preview">{}</div>
                <div class="meta">
                    <span class="rkey">{}</span>
                    <span class="tags">{}</span>
                </div>
            </div>"#,
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
        nav { margin: 2rem 0; }
        nav a {
            margin-right: 1rem;
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
        <a href="/notes">Notes</a>
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
        }
        .thought.tool_call .content {
            font-family: "SF Mono", "Menlo", "Monaco", monospace;
            font-size: 0.9rem;
            background: #252a33;
            padding: 0.5rem 0.75rem;
            border-radius: 3px;
            margin-top: 0.5rem;
        }
        .tool-name {
            color: #88c0d0;
            font-weight: 600;
        }
        .tool-args {
            color: #a3be8c;
        }
        .tool-failed {
            color: #bf616a;
            font-weight: 600;
        }
        .trigger {
            font-size: 0.8rem;
            color: #888;
            margin-top: 0.5rem;
            font-style: italic;
        }
        .trigger::before {
            content: "↳ ";
            color: #666;
        }
        .duration {
            font-size: 0.75rem;
            color: #666;
            margin-left: 0.5rem;
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
    <div id="stream"><!-- THOUGHTS --></div>
    <script>
        const stream = document.getElementById('stream');
        const filterBtns = document.querySelectorAll('.filter-btn');
        let activeFilter = 'all';

        // Filter button handling
        filterBtns.forEach(btn => {
            btn.addEventListener('click', () => {
                filterBtns.forEach(b => b.classList.remove('active'));
                btn.classList.add('active');
                activeFilter = btn.dataset.kind;
                applyFilter();
            });
        });

        function applyFilter() {
            document.querySelectorAll('.thought').forEach(thought => {
                if (activeFilter === 'all' || thought.classList.contains(activeFilter)) {
                    thought.classList.remove('hidden');
                } else {
                    thought.classList.add('hidden');
                }
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
        function formatToolCallContent(content) {
            // Pattern: "Called tool_name [args] - FAILED" or "Called tool_name [args]"
            const match = content.match(/^Called\s+(\w+)\s*\[(.*)\](\s*-\s*FAILED)?$/);
            if (match) {
                const toolName = match[1];
                const args = match[2];
                const failed = match[3];
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
            if (thought.trigger) {
                html += '<div class="trigger">' + escapeHtml(thought.trigger) + '</div>';
            }
            return html;
        }

        const eventSource = new EventSource('/api/thoughts/sse');

        eventSource.onmessage = function(event) {
            const thought = JSON.parse(event.data);
            const div = document.createElement('div');
            const hidden = activeFilter !== 'all' && thought.kind !== activeFilter ? ' hidden' : '';
            div.className = 'thought ' + thought.kind + hidden;
            div.innerHTML = buildThoughtHtml(thought);
            stream.prepend(div);
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
        .count { color: #888; margin-bottom: 1rem; }
    </style>
</head>
<body>
    <h1><a href="/">Winter</a> / Facts</h1>
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
        .count { color: #888; margin-bottom: 1rem; }
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
    <h1><a href="/">Winter</a> / Jobs</h1>
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
        .count { color: #888; margin-bottom: 1rem; }
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
        .title { font-weight: bold; color: #88c0d0; }
        .category { color: #888; font-size: 0.9rem; }
        .preview { color: #aaa; line-height: 1.5; }
        .meta { margin-top: 0.5rem; font-size: 0.85rem; color: #666; }
        .rkey { font-family: monospace; }
        .tags { color: #81a1c1; }
    </style>
</head>
<body>
    <h1><a href="/">Winter</a> / Notes</h1>
    <p class="count"><!-- COUNT --> notes</p>
    <div id="notes"><!-- NOTES --></div>
</body>
</html>"#;

// ============================================================================
// Tools and Secrets Routes
// ============================================================================

async fn tools_page(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    let tools = match state
        .client
        .list_all_records::<CustomTool>(TOOL_COLLECTION)
        .await
    {
        Ok(t) => t,
        Err(e) => {
            warn!(error = %e, "failed to load tools for tools page");
            Vec::new()
        }
    };

    let mut tools_html = String::new();
    for item in &tools {
        let rkey = item.uri.split('/').next_back().unwrap_or("");

        // Get approval status
        let approval = state
            .client
            .get_record::<ToolApproval>(TOOL_APPROVAL_COLLECTION, rkey)
            .await
            .ok();

        let (status, status_class) = match &approval {
            Some(r)
                if r.value.status == ToolApprovalStatus::Approved
                    && r.value.tool_version == item.value.version =>
            {
                ("approved", "approved")
            }
            Some(r) if r.value.status == ToolApprovalStatus::Denied => ("denied", "denied"),
            Some(r) if r.value.status == ToolApprovalStatus::Revoked => ("revoked", "denied"),
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

    let secrets_checkboxes: String = tool
        .required_secrets
        .iter()
        .map(|s| {
            let checked = approval
                .as_ref()
                .map(|a| a.allowed_secrets.contains(s))
                .unwrap_or(false);
            let checked_attr = if checked { " checked" } else { "" };
            format!(
                r#"<label><input type="checkbox" name="secrets" value="{s}"{checked_attr}> {s}</label><br>"#
            )
        })
        .collect();

    let network_checked = approval
        .as_ref()
        .and_then(|a| a.allow_network)
        .unwrap_or(false);
    let network_attr = if network_checked { " checked" } else { "" };

    Html(
        TOOL_DETAIL_HTML
            .replace("<!-- RKEY -->", &rkey)
            .replace("<!-- NAME -->", &html_escape(&tool.name))
            .replace("<!-- DESCRIPTION -->", &html_escape(&tool.description))
            .replace("<!-- CODE -->", &html_escape(&tool.code))
            .replace("<!-- VERSION -->", &tool.version.to_string())
            .replace("<!-- STATUS -->", status)
            .replace("<!-- STATUS_CLASS -->", status_class)
            .replace("<!-- SECRETS_CHECKBOXES -->", &secrets_checkboxes)
            .replace("<!-- NETWORK_CHECKED -->", network_attr)
            .replace(
                "<!-- INPUT_SCHEMA -->",
                &html_escape(&serde_json::to_string_pretty(&tool.input_schema).unwrap_or_default()),
            ),
    )
}

#[derive(Deserialize)]
struct ApprovalForm {
    #[serde(default)]
    secrets: Vec<String>,
    #[serde(default)]
    allow_network: Option<String>,
    reason: Option<String>,
}

async fn approve_tool(
    State(state): State<Arc<AppState>>,
    Path(rkey): Path<String>,
    Form(form): Form<ApprovalForm>,
) -> impl IntoResponse {
    // Get the tool to verify it exists and get version
    let tool = match state
        .client
        .get_record::<CustomTool>(TOOL_COLLECTION, &rkey)
        .await
    {
        Ok(t) => t.value,
        Err(_) => return Redirect::to(&format!("/tools/{}", rkey)),
    };

    let approval = ToolApproval {
        tool_rkey: rkey.clone(),
        tool_version: tool.version,
        status: ToolApprovalStatus::Approved,
        allow_network: Some(form.allow_network.is_some()),
        allowed_secrets: form.secrets,
        approved_by: None, // TODO: get from session
        reason: form.reason,
        created_at: Utc::now(),
    };

    let _ = state
        .client
        .put_record(TOOL_APPROVAL_COLLECTION, &rkey, &approval)
        .await;

    Redirect::to(&format!("/tools/{}", rkey))
}

async fn deny_tool(
    State(state): State<Arc<AppState>>,
    Path(rkey): Path<String>,
    Form(form): Form<ApprovalForm>,
) -> impl IntoResponse {
    let tool = match state
        .client
        .get_record::<CustomTool>(TOOL_COLLECTION, &rkey)
        .await
    {
        Ok(t) => t.value,
        Err(_) => return Redirect::to(&format!("/tools/{}", rkey)),
    };

    let approval = ToolApproval {
        tool_rkey: rkey.clone(),
        tool_version: tool.version,
        status: ToolApprovalStatus::Denied,
        allow_network: None,
        allowed_secrets: Vec::new(),
        approved_by: None,
        reason: form.reason,
        created_at: Utc::now(),
    };

    let _ = state
        .client
        .put_record(TOOL_APPROVAL_COLLECTION, &rkey, &approval)
        .await;

    Redirect::to(&format!("/tools/{}", rkey))
}

async fn revoke_tool(
    State(state): State<Arc<AppState>>,
    Path(rkey): Path<String>,
) -> impl IntoResponse {
    let tool = match state
        .client
        .get_record::<CustomTool>(TOOL_COLLECTION, &rkey)
        .await
    {
        Ok(t) => t.value,
        Err(_) => return Redirect::to(&format!("/tools/{}", rkey)),
    };

    let approval = ToolApproval {
        tool_rkey: rkey.clone(),
        tool_version: tool.version,
        status: ToolApprovalStatus::Revoked,
        allow_network: None,
        allowed_secrets: Vec::new(),
        approved_by: None,
        reason: Some("Revoked by operator".to_string()),
        created_at: Utc::now(),
    };

    let _ = state
        .client
        .put_record(TOOL_APPROVAL_COLLECTION, &rkey, &approval)
        .await;

    Redirect::to(&format!("/tools/{}", rkey))
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
        .approval-form {
            background: #2e3440;
            padding: 1.5rem;
            border-radius: 4px;
            margin: 1rem 0;
        }
        .approval-form h3 { margin-top: 0; color: #88c0d0; }
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

    <div class="approval-form">
        <h3>Approval</h3>
        <form action="/api/tools/<!-- RKEY -->/approve" method="post">
            <div class="form-group">
                <label><input type="checkbox" name="allow_network"<!-- NETWORK_CHECKED -->> Allow network access</label>
            </div>
            <div class="form-group">
                <label>Allowed secrets:</label>
                <!-- SECRETS_CHECKBOXES -->
            </div>
            <div class="form-group">
                <label>Reason (optional):</label>
                <input type="text" name="reason" placeholder="Approval reason">
            </div>
            <button type="submit" class="btn btn-approve">Approve</button>
        </form>
        <form action="/api/tools/<!-- RKEY -->/deny" method="post" style="display: inline;">
            <input type="hidden" name="reason" value="">
            <button type="submit" class="btn btn-deny">Deny</button>
        </form>
        <form action="/api/tools/<!-- RKEY -->/revoke" method="post" style="display: inline;">
            <button type="submit" class="btn btn-revoke">Revoke</button>
        </form>
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
