//! DID -> handle resolution.

use std::collections::HashMap;

use tracing::warn;

/// Handle resolver with in-memory cache.
pub struct HandleResolver {
    cache: HashMap<String, String>,
}

impl HandleResolver {
    pub fn new() -> Self {
        Self {
            cache: HashMap::new(),
        }
    }

    /// Resolve a DID to a handle. Returns the DID itself if resolution fails.
    pub async fn resolve(&mut self, did: &str) -> String {
        if let Some(handle) = self.cache.get(did) {
            return handle.clone();
        }

        match resolve_did_to_handle(did).await {
            Some(handle) => {
                self.cache.insert(did.to_string(), handle.clone());
                handle
            }
            None => did.to_string(),
        }
    }

    /// Resolve a handle to a DID.
    pub async fn resolve_handle_to_did(&self, handle: &str) -> Option<String> {
        let url = format!(
            "https://bsky.social/xrpc/com.atproto.identity.resolveHandle?handle={}",
            handle
        );

        let resp = reqwest::get(&url).await.ok()?;
        if !resp.status().is_success() {
            return None;
        }

        let body: serde_json::Value = resp.json().await.ok()?;
        body.get("did").and_then(|d| d.as_str()).map(String::from)
    }
}

/// Resolve a DID to a handle via plc.directory.
async fn resolve_did_to_handle(did: &str) -> Option<String> {
    let url = if did.starts_with("did:plc:") {
        format!("https://plc.directory/{}", did)
    } else if did.starts_with("did:web:") {
        // did:web resolution is different
        return None;
    } else {
        return None;
    };

    let resp = reqwest::get(&url).await.ok()?;
    if !resp.status().is_success() {
        warn!(did = %did, status = %resp.status(), "DID resolution failed");
        return None;
    }

    let body: serde_json::Value = resp.json().await.ok()?;

    // Extract handle from alsoKnownAs
    body.get("alsoKnownAs")
        .and_then(|v| v.as_array())
        .and_then(|arr| {
            arr.iter().find_map(|v| {
                v.as_str()
                    .and_then(|s| s.strip_prefix("at://"))
                    .map(String::from)
            })
        })
}
