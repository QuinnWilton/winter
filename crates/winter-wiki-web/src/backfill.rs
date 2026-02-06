//! On-demand backfill for users not yet indexed.

use std::sync::Arc;

use tracing::{info, warn};

use winter_atproto::{WIKI_ENTRY_COLLECTION, WIKI_LINK_COLLECTION, WikiEntry, WikiLink};

use crate::db::WikiDb;

/// Backfill wiki records for a specific DID by fetching from their PDS.
pub async fn backfill_did(db: &Arc<WikiDb>, did: &str) -> Result<(), Box<dyn std::error::Error>> {
    let pds_url = resolve_pds(did).await.ok_or("could not resolve PDS")?;

    // Fetch wiki entries
    let entries = list_records::<WikiEntry>(&pds_url, did, WIKI_ENTRY_COLLECTION).await?;
    for (rkey, entry) in &entries {
        let _ = db.upsert_entry(did, rkey, entry);
    }

    // Fetch wiki links
    let links = list_records::<WikiLink>(&pds_url, did, WIKI_LINK_COLLECTION).await?;
    for (rkey, link) in &links {
        let _ = db.insert_link(did, rkey, link);
    }

    info!(
        did = %did,
        entries = entries.len(),
        links = links.len(),
        "backfilled wiki records"
    );

    Ok(())
}

/// Resolve a DID to its PDS URL.
async fn resolve_pds(did: &str) -> Option<String> {
    let url = if did.starts_with("did:plc:") {
        format!("https://plc.directory/{}", did)
    } else {
        return None;
    };

    let resp = reqwest::get(&url).await.ok()?;
    let body: serde_json::Value = resp.json().await.ok()?;

    body.get("service")
        .and_then(|v| v.as_array())
        .and_then(|arr| {
            arr.iter().find_map(|s| {
                let stype = s.get("type").and_then(|t| t.as_str())?;
                if stype == "AtprotoPersonalDataServer" {
                    s.get("serviceEndpoint")
                        .and_then(|e| e.as_str())
                        .map(String::from)
                } else {
                    None
                }
            })
        })
}

/// List all records of a collection from a PDS.
async fn list_records<T: serde::de::DeserializeOwned>(
    pds_url: &str,
    did: &str,
    collection: &str,
) -> Result<Vec<(String, T)>, Box<dyn std::error::Error>> {
    let mut records = Vec::new();
    let mut cursor: Option<String> = None;

    loop {
        let mut url = format!(
            "{}/xrpc/com.atproto.repo.listRecords?repo={}&collection={}&limit=100",
            pds_url, did, collection
        );

        if let Some(ref c) = cursor {
            url.push_str(&format!("&cursor={}", c));
        }

        let resp = reqwest::get(&url).await?;
        if !resp.status().is_success() {
            warn!(status = %resp.status(), "listRecords failed");
            break;
        }

        let body: serde_json::Value = resp.json().await?;

        if let Some(items) = body.get("records").and_then(|v| v.as_array()) {
            for item in items {
                let uri = item.get("uri").and_then(|v| v.as_str()).unwrap_or("");
                let rkey = uri.split('/').next_back().unwrap_or("");
                if let Some(value) = item.get("value") {
                    if let Ok(record) = serde_json::from_value::<T>(value.clone()) {
                        records.push((rkey.to_string(), record));
                    }
                }
            }

            cursor = body
                .get("cursor")
                .and_then(|v| v.as_str())
                .map(String::from);

            if cursor.is_none() || items.is_empty() {
                break;
            }
        } else {
            break;
        }
    }

    Ok(records)
}
