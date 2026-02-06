//! Web routes for the wiki browser.

use std::sync::Arc;

use axum::{
    Router,
    extract::{Path, Query, State},
    http::StatusCode,
    response::{Html, IntoResponse},
    routing::{get, post},
};
use serde::Deserialize;
use tokio::sync::RwLock;

use crate::backfill;
use crate::db::WikiDb;
use crate::renderer::render_wiki_markdown;
use crate::resolver::HandleResolver;

/// Shared application state.
pub struct AppState {
    pub db: Arc<WikiDb>,
    pub resolver: Arc<RwLock<HandleResolver>>,
}

/// Create the web router.
pub fn create_router(db: Arc<WikiDb>, resolver: Arc<RwLock<HandleResolver>>) -> Router {
    let state = Arc::new(AppState { db, resolver });

    Router::new()
        .route("/", get(index))
        .route("/u/{handle_or_did}", get(user_entries))
        .route("/u/{handle_or_did}/{slug}", get(entry_detail))
        .route("/search", get(search))
        .route("/admin/backfill/{handle_or_did}", post(admin_backfill))
        .with_state(state)
}

#[derive(Deserialize)]
struct SearchQuery {
    q: Option<String>,
}

async fn index(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    let recent = state.db.recent_entries(20).unwrap_or_default();
    let entry_count = state.db.entry_count().unwrap_or(0);
    let author_count = state.db.author_count().unwrap_or(0);

    let mut entries_html = String::new();
    for entry in &recent {
        let handle = state
            .resolver
            .write()
            .await
            .resolve(&entry.did)
            .await;
        let preview = entry
            .summary
            .as_deref()
            .unwrap_or_else(|| truncate(&entry.content, 150));

        entries_html.push_str(&format!(
            r#"<div class="entry">
                <a href="/u/{}/{}" class="title">{}</a>
                <span class="author">by <a href="/u/{}">{}</a></span>
                <div class="preview">{}</div>
            </div>"#,
            html_escape(&handle),
            html_escape(&entry.slug),
            html_escape(&entry.title),
            html_escape(&handle),
            html_escape(&handle),
            html_escape(preview),
        ));
    }

    Html(
        INDEX_HTML
            .replace("<!-- ENTRIES -->", &entries_html)
            .replace("<!-- ENTRY_COUNT -->", &entry_count.to_string())
            .replace("<!-- AUTHOR_COUNT -->", &author_count.to_string()),
    )
}

async fn user_entries(
    State(state): State<Arc<AppState>>,
    Path(handle_or_did): Path<String>,
) -> impl IntoResponse {
    let did = resolve_to_did(&state, &handle_or_did).await;
    let handle = state.resolver.write().await.resolve(&did).await;

    let entries = state.db.list_entries_by_did(&did).unwrap_or_default();

    // If no entries found, try backfill
    if entries.is_empty() {
        let _ = backfill::backfill_did(&state.db, &did).await;
        let entries = state.db.list_entries_by_did(&did).unwrap_or_default();
        return Html(render_user_page(&handle, &entries));
    }

    Html(render_user_page(&handle, &entries))
}

async fn entry_detail(
    State(state): State<Arc<AppState>>,
    Path((handle_or_did, slug)): Path<(String, String)>,
) -> impl IntoResponse {
    let did = resolve_to_did(&state, &handle_or_did).await;
    let handle = state.resolver.write().await.resolve(&did).await;

    let entry = match state.db.get_entry_by_slug(&did, &slug) {
        Ok(Some(e)) => e,
        _ => {
            // Try backfill
            let _ = backfill::backfill_did(&state.db, &did).await;
            match state.db.get_entry_by_slug(&did, &slug) {
                Ok(Some(e)) => e,
                _ => return Html(NOT_FOUND_HTML.to_string()),
            }
        }
    };

    // Build AT URI for backlinks
    let entry_uri = format!(
        "at://{}/{}/{}",
        did, winter_atproto::WIKI_ENTRY_COLLECTION, entry.rkey
    );

    // Get backlinks
    let backlinks = state.db.get_backlinks(&entry_uri).unwrap_or_default();

    // Render content with wiki-link resolution
    let rendered = render_wiki_markdown(&entry.content, &handle, |slug, target_did| {
        let target = target_did.unwrap_or(&did);
        state
            .db
            .get_entry_by_slug(target, slug)
            .ok()
            .flatten()
            .map(|e| {
                let h = handle.clone(); // Simplified; would need resolver for cross-user
                (h, e.rkey)
            })
    });

    let mut backlinks_html = String::new();
    if !backlinks.is_empty() {
        backlinks_html.push_str("<h2>Backlinks</h2><ul class=\"backlinks\">");
        for link in &backlinks {
            // Resolve source entry
            let source_did = link.source_uri.split('/').nth(2).unwrap_or("");
            let source_rkey = link.source_uri.split('/').next_back().unwrap_or("");
            let source_handle = state.resolver.write().await.resolve(source_did).await;

            // Find source entry for title/slug
            let source_entries = state.db.list_entries_by_did(source_did).unwrap_or_default();
            let source_entry = source_entries.iter().find(|e| e.rkey == source_rkey);

            if let Some(src) = source_entry {
                backlinks_html.push_str(&format!(
                    r#"<li><a href="/u/{}/{}">{}</a> <span class="link-type">({})</span> by <a href="/u/{}">{}</a></li>"#,
                    html_escape(&source_handle),
                    html_escape(&src.slug),
                    html_escape(&src.title),
                    html_escape(&link.link_type),
                    html_escape(&source_handle),
                    html_escape(&source_handle),
                ));
            }
        }
        backlinks_html.push_str("</ul>");
    }

    let tags: Vec<String> = serde_json::from_str(&entry.tags).unwrap_or_default();
    let tags_html = if tags.is_empty() {
        String::new()
    } else {
        format!(
            "<p class=\"tags\">Tags: {}</p>",
            tags.iter()
                .map(|t| html_escape(t))
                .collect::<Vec<_>>()
                .join(", ")
        )
    };

    Html(
        DETAIL_HTML
            .replace("<!-- TITLE -->", &html_escape(&entry.title))
            .replace("<!-- HANDLE -->", &html_escape(&handle))
            .replace("<!-- SLUG -->", &html_escape(&entry.slug))
            .replace("<!-- STATUS -->", &html_escape(&entry.status))
            .replace("<!-- CONTENT -->", &rendered)
            .replace("<!-- TAGS -->", &tags_html)
            .replace("<!-- BACKLINKS -->", &backlinks_html)
            .replace("<!-- CREATED_AT -->", &entry.created_at)
            .replace("<!-- UPDATED_AT -->", &entry.last_updated),
    )
}

async fn search(
    State(state): State<Arc<AppState>>,
    Query(params): Query<SearchQuery>,
) -> impl IntoResponse {
    let query = params.q.unwrap_or_default();
    let results = if query.is_empty() {
        Vec::new()
    } else {
        state.db.search_entries(&query, 50).unwrap_or_default()
    };

    let mut results_html = String::new();
    for entry in &results {
        let handle = state.resolver.write().await.resolve(&entry.did).await;
        let preview = entry
            .summary
            .as_deref()
            .unwrap_or_else(|| truncate(&entry.content, 150));

        results_html.push_str(&format!(
            r#"<div class="entry">
                <a href="/u/{}/{}" class="title">{}</a>
                <span class="author">by <a href="/u/{}">{}</a></span>
                <div class="preview">{}</div>
            </div>"#,
            html_escape(&handle),
            html_escape(&entry.slug),
            html_escape(&entry.title),
            html_escape(&handle),
            html_escape(&handle),
            html_escape(preview),
        ));
    }

    Html(
        SEARCH_HTML
            .replace("<!-- QUERY -->", &html_escape(&query))
            .replace("<!-- RESULTS -->", &results_html)
            .replace("<!-- COUNT -->", &results.len().to_string()),
    )
}

// ============================================================================
// Admin
// ============================================================================

async fn admin_backfill(
    State(state): State<Arc<AppState>>,
    Path(handle_or_did): Path<String>,
) -> impl IntoResponse {
    let did = resolve_to_did(&state, &handle_or_did).await;

    // Clear existing data for this DID and re-fetch everything
    let _ = state.db.clear_did(&did);
    match backfill::backfill_did(&state.db, &did).await {
        Ok(()) => {
            let count = state.db.list_entries_by_did(&did).map(|e| e.len()).unwrap_or(0);
            (StatusCode::OK, format!("Backfilled {} entries for {}", count, did))
        }
        Err(e) => {
            (StatusCode::INTERNAL_SERVER_ERROR, format!("Backfill failed: {}", e))
        }
    }
}

// ============================================================================
// Helpers
// ============================================================================

async fn resolve_to_did(state: &AppState, handle_or_did: &str) -> String {
    if handle_or_did.starts_with("did:") {
        handle_or_did.to_string()
    } else {
        state
            .resolver
            .write()
            .await
            .resolve_handle_to_did(handle_or_did)
            .await
            .unwrap_or_else(|| handle_or_did.to_string())
    }
}

fn render_user_page(handle: &str, entries: &[crate::db::WikiEntryRow]) -> String {
    let mut entries_html = String::new();
    for entry in entries {
        let preview = entry
            .summary
            .as_deref()
            .unwrap_or_else(|| truncate(&entry.content, 120));

        entries_html.push_str(&format!(
            r#"<div class="entry">
                <a href="/u/{}/{}" class="title">{}</a>
                <span class="status">{}</span>
                <div class="preview">{}</div>
            </div>"#,
            html_escape(handle),
            html_escape(&entry.slug),
            html_escape(&entry.title),
            html_escape(&entry.status),
            html_escape(preview),
        ));
    }

    USER_HTML
        .replace("<!-- HANDLE -->", &html_escape(handle))
        .replace("<!-- ENTRIES -->", &entries_html)
        .replace("<!-- COUNT -->", &entries.len().to_string())
}

fn html_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

fn truncate(s: &str, max: usize) -> &str {
    if s.len() <= max {
        s
    } else {
        let end = s
            .char_indices()
            .nth(max)
            .map(|(i, _)| i)
            .unwrap_or(s.len());
        &s[..end]
    }
}

// ============================================================================
// HTML Templates
// ============================================================================

const INDEX_HTML: &str = r#"<!DOCTYPE html>
<html>
<head>
    <meta charset="utf-8">
    <meta name="viewport" content="width=device-width, initial-scale=1">
    <title>Winter Wiki</title>
    <style>
        body { font-family: -apple-system, BlinkMacSystemFont, "Segoe UI", Roboto, sans-serif; max-width: 800px; margin: 0 auto; padding: 2rem; background: #0a0a0a; color: #e0e0e0; }
        h1 { color: #88c0d0; }
        a { color: #81a1c1; }
        .stats { color: #888; margin-bottom: 2rem; }
        .search { margin: 1rem 0; }
        .search input { width: 100%; padding: 0.5rem; background: #2e3440; border: 1px solid #4c566a; border-radius: 4px; color: #e0e0e0; font-size: 1rem; box-sizing: border-box; }
        .entry { padding: 1rem; margin: 1rem 0; background: #2e3440; border-radius: 4px; }
        .title { font-weight: bold; color: #88c0d0; text-decoration: none; }
        .title:hover { text-decoration: underline; }
        .author { color: #888; font-size: 0.9rem; margin-left: 0.5rem; }
        .preview { color: #aaa; margin-top: 0.5rem; line-height: 1.5; }
    </style>
</head>
<body>
    <h1>Winter Wiki</h1>
    <p class="stats"><!-- ENTRY_COUNT --> entries from <!-- AUTHOR_COUNT --> authors</p>
    <div class="search">
        <form action="/search" method="get">
            <input type="text" name="q" placeholder="Search wiki entries..." autofocus>
        </form>
    </div>
    <h2>Recent Entries</h2>
    <div><!-- ENTRIES --></div>
</body>
</html>"#;

const USER_HTML: &str = r#"<!DOCTYPE html>
<html>
<head>
    <meta charset="utf-8">
    <meta name="viewport" content="width=device-width, initial-scale=1">
    <title><!-- HANDLE --> - Winter Wiki</title>
    <style>
        body { font-family: -apple-system, BlinkMacSystemFont, "Segoe UI", Roboto, sans-serif; max-width: 800px; margin: 0 auto; padding: 2rem; background: #0a0a0a; color: #e0e0e0; }
        h1 { color: #88c0d0; }
        h1 a { color: #88c0d0; text-decoration: none; }
        a { color: #81a1c1; }
        .count { color: #888; }
        .entry { padding: 1rem; margin: 1rem 0; background: #2e3440; border-radius: 4px; }
        .title { font-weight: bold; color: #88c0d0; text-decoration: none; }
        .title:hover { text-decoration: underline; }
        .status { color: #888; font-size: 0.85rem; margin-left: 0.5rem; }
        .preview { color: #aaa; margin-top: 0.5rem; }
    </style>
</head>
<body>
    <h1><a href="/">Wiki</a> / <!-- HANDLE --></h1>
    <p class="count"><!-- COUNT --> entries</p>
    <div><!-- ENTRIES --></div>
</body>
</html>"#;

const DETAIL_HTML: &str = r#"<!DOCTYPE html>
<html>
<head>
    <meta charset="utf-8">
    <meta name="viewport" content="width=device-width, initial-scale=1">
    <title><!-- TITLE --> - Winter Wiki</title>
    <style>
        body { font-family: -apple-system, BlinkMacSystemFont, "Segoe UI", Roboto, sans-serif; max-width: 800px; margin: 0 auto; padding: 2rem; background: #0a0a0a; color: #e0e0e0; }
        h1 { color: #88c0d0; }
        h1 a { color: #88c0d0; text-decoration: none; }
        a { color: #81a1c1; }
        .meta { color: #888; font-size: 0.9rem; margin-bottom: 1rem; }
        .content { line-height: 1.7; }
        .content a { color: #a3be8c; }
        .tags { color: #81a1c1; margin-top: 1rem; }
        .backlinks { list-style: none; padding: 0; }
        .backlinks li { padding: 0.3rem 0; }
        .link-type { color: #888; font-size: 0.85rem; }
        .timestamps { color: #666; font-size: 0.85rem; margin-top: 2rem; }
    </style>
</head>
<body>
    <h1><a href="/">Wiki</a> / <a href="/u/<!-- HANDLE -->"><!-- HANDLE --></a> / <!-- TITLE --></h1>
    <div class="meta">
        <span class="slug">/<!-- SLUG --></span> &middot;
        <span class="status"><!-- STATUS --></span>
    </div>
    <div class="content"><!-- CONTENT --></div>
    <!-- TAGS -->
    <!-- BACKLINKS -->
    <div class="timestamps">
        Created: <!-- CREATED_AT --><br>
        Updated: <!-- UPDATED_AT -->
    </div>
</body>
</html>"#;

const SEARCH_HTML: &str = r#"<!DOCTYPE html>
<html>
<head>
    <meta charset="utf-8">
    <meta name="viewport" content="width=device-width, initial-scale=1">
    <title>Search - Winter Wiki</title>
    <style>
        body { font-family: -apple-system, BlinkMacSystemFont, "Segoe UI", Roboto, sans-serif; max-width: 800px; margin: 0 auto; padding: 2rem; background: #0a0a0a; color: #e0e0e0; }
        h1 { color: #88c0d0; }
        h1 a { color: #88c0d0; text-decoration: none; }
        a { color: #81a1c1; }
        .search { margin: 1rem 0; }
        .search input { width: 100%; padding: 0.5rem; background: #2e3440; border: 1px solid #4c566a; border-radius: 4px; color: #e0e0e0; font-size: 1rem; box-sizing: border-box; }
        .count { color: #888; }
        .entry { padding: 1rem; margin: 1rem 0; background: #2e3440; border-radius: 4px; }
        .title { font-weight: bold; color: #88c0d0; text-decoration: none; }
        .title:hover { text-decoration: underline; }
        .author { color: #888; font-size: 0.9rem; margin-left: 0.5rem; }
        .preview { color: #aaa; margin-top: 0.5rem; }
    </style>
</head>
<body>
    <h1><a href="/">Winter Wiki</a> / Search</h1>
    <div class="search">
        <form action="/search" method="get">
            <input type="text" name="q" value="<!-- QUERY -->" autofocus>
        </form>
    </div>
    <p class="count"><!-- COUNT --> results</p>
    <div><!-- RESULTS --></div>
</body>
</html>"#;

const NOT_FOUND_HTML: &str = r#"<!DOCTYPE html>
<html>
<head>
    <meta charset="utf-8">
    <meta name="viewport" content="width=device-width, initial-scale=1">
    <title>Not Found - Winter Wiki</title>
    <style>
        body { font-family: -apple-system, BlinkMacSystemFont, "Segoe UI", Roboto, sans-serif; max-width: 800px; margin: 0 auto; padding: 2rem; background: #0a0a0a; color: #e0e0e0; }
        h1 { color: #88c0d0; }
        a { color: #81a1c1; }
    </style>
</head>
<body>
    <h1>Entry Not Found</h1>
    <p>The requested wiki entry does not exist.</p>
    <a href="/">Back to Wiki</a>
</body>
</html>"#;
