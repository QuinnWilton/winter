//! Markdown rendering with wiki-link resolution.

use pulldown_cmark::{Options, Parser, html};
use regex::Regex;

/// Render markdown to HTML, resolving wiki-link syntax.
///
/// `resolve_slug` is called for `[[slug]]` references to check if the entry exists
/// and get the author handle for URL construction.
pub fn render_wiki_markdown(
    content: &str,
    author_handle: &str,
    resolve_slug: impl Fn(&str, Option<&str>) -> Option<(String, String)>,
) -> String {
    // First pass: replace [[wiki-link]] syntax with HTML links
    let with_links = resolve_wiki_links(content, author_handle, resolve_slug);

    // Second pass: render markdown to HTML
    let mut options = Options::empty();
    options.insert(Options::ENABLE_STRIKETHROUGH);
    options.insert(Options::ENABLE_TABLES);

    let parser = Parser::new_ext(&with_links, options);
    let mut html_output = String::new();
    html::push_html(&mut html_output, parser);

    html_output
}

/// Replace [[wiki-link]] syntax with HTML anchor tags.
fn resolve_wiki_links(
    content: &str,
    author_handle: &str,
    resolve_slug: impl Fn(&str, Option<&str>) -> Option<(String, String)>,
) -> String {
    let re = Regex::new(r"\[\[([^\]|]+?)(?:\|([^\]]+))?\]\]").unwrap();

    re.replace_all(content, |caps: &regex::Captures| {
        let reference = caps[1].trim();
        let display_text = caps.get(2).map(|m| m.as_str().trim());

        if reference.starts_with("did:") {
            // [[did:plc:xxx/slug]]
            if let Some(slash_pos) = reference.find('/') {
                let did = &reference[..slash_pos];
                let slug = &reference[slash_pos + 1..];
                let display = display_text.unwrap_or(slug);

                if let Some((handle, _)) = resolve_slug(slug, Some(did)) {
                    format!("[{}](/u/{}/{})", display, handle, slug)
                } else {
                    format!("[{}](/u/{}/{})", display, did, slug)
                }
            } else {
                reference.to_string()
            }
        } else if reference.contains('/') {
            // [[handle/slug]]
            if let Some(slash_pos) = reference.find('/') {
                let handle = &reference[..slash_pos];
                let slug = &reference[slash_pos + 1..];
                let display = display_text.unwrap_or(slug);

                format!("[{}](/u/{}/{})", display, handle, slug)
            } else {
                reference.to_string()
            }
        } else {
            // [[slug]] — local reference
            let display = display_text.unwrap_or(reference);

            if resolve_slug(reference, None).is_some() {
                format!("[{}](/u/{}/{})", display, author_handle, reference)
            } else {
                // Missing link — render as red link
                format!("[{}](/u/{}/{})", display, author_handle, reference)
            }
        }
    })
    .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_render_local_link() {
        let result = resolve_wiki_links("See [[my-page]]", "alice.bsky.social", |_, _| {
            Some(("alice.bsky.social".to_string(), "rkey".to_string()))
        });
        assert!(result.contains("/u/alice.bsky.social/my-page"));
    }

    #[test]
    fn test_render_cross_user_link() {
        let result = resolve_wiki_links("See [[bob.bsky.social/page]]", "alice.bsky.social", |_, _| None);
        assert!(result.contains("/u/bob.bsky.social/page"));
    }

    #[test]
    fn test_render_display_text() {
        let result = resolve_wiki_links("See [[my-page|My Page]]", "alice.bsky.social", |_, _| {
            Some(("alice.bsky.social".to_string(), "rkey".to_string()))
        });
        assert!(result.contains("My Page"));
        assert!(result.contains("/u/alice.bsky.social/my-page"));
    }
}
