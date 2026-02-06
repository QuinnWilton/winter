//! Sync coordinator for cache hydration and Jetstream subscription.
//!
//! Orchestrates the startup sequence:
//! 1. Download full repo as CAR file (single HTTP request)
//! 2. Parse MST and populate cache
//! 3. Start Jetstream WebSocket for live updates

use std::sync::Arc;

use tokio::sync::watch;
use tokio::task::JoinHandle;
use tracing::{error, info};

use crate::cache::{RepoCache, SyncState};
use crate::car;
use crate::jetstream::{
    DEFAULT_JETSTREAM_URL, JetstreamClient, OperatorEventCallback,
};
use crate::{AtprotoClient, AtprotoError};

/// Sync coordinator for managing cache synchronization.
pub struct SyncCoordinator {
    /// ATProto client for fetching records.
    client: AtprotoClient,
    /// DID of the repository to sync.
    did: String,
    /// Jetstream URL.
    jetstream_url: String,
    /// Cache to populate.
    cache: Arc<RepoCache>,
    /// Operator DID for watching tool approvals.
    operator_did: Option<String>,
    /// Callback for operator events (tool approvals, etc.).
    operator_callback: Option<OperatorEventCallback>,
}

impl SyncCoordinator {
    /// Create a new sync coordinator.
    pub fn new(client: AtprotoClient, did: impl Into<String>, cache: Arc<RepoCache>) -> Self {
        Self {
            client,
            did: did.into(),
            jetstream_url: DEFAULT_JETSTREAM_URL.to_string(),
            cache,
            operator_did: None,
            operator_callback: None,
        }
    }

    /// Set a custom Jetstream URL.
    pub fn with_jetstream_url(mut self, url: impl Into<String>) -> Self {
        self.jetstream_url = url.into();
        self
    }

    /// Set the operator DID for watching tool approvals via Jetstream.
    pub fn with_operator_did(mut self, did: impl Into<String>) -> Self {
        let did = did.into();
        if !did.is_empty() {
            self.operator_did = Some(did);
        }
        self
    }

    /// Set a callback for operator events from Jetstream.
    pub fn with_operator_callback(mut self, callback: OperatorEventCallback) -> Self {
        self.operator_callback = Some(callback);
        self
    }

    /// Get the cache.
    pub fn cache(&self) -> Arc<RepoCache> {
        Arc::clone(&self.cache)
    }

    /// Start synchronization.
    ///
    /// This downloads the full repo as a CAR file, parses the MST,
    /// populates the cache, then starts a Jetstream WebSocket for live updates.
    ///
    /// Returns a handle to the Jetstream task.
    pub async fn start(
        &self,
        shutdown_rx: watch::Receiver<bool>,
    ) -> Result<JoinHandle<()>, AtprotoError> {
        info!(did = %self.did, jetstream = %self.jetstream_url, "starting sync coordinator");

        // Set state to syncing
        self.cache.set_state(SyncState::Syncing);

        // 1. Download and parse CAR file
        info!(did = %self.did, "downloading repo CAR file");
        self.populate_cache().await?;

        info!(
            facts = self.cache.fact_count(),
            rules = self.cache.rule_count(),
            "cache populated from CAR"
        );

        // 2. Go live
        self.cache.set_state(SyncState::Live);

        // 3. Start Jetstream for live updates
        let mut jetstream = JetstreamClient::new(
            self.jetstream_url.clone(),
            self.did.clone(),
            Arc::clone(&self.cache),
        );

        if let Some(ref operator_did) = self.operator_did {
            jetstream = jetstream.with_operator_did(operator_did.clone());
        }

        if let Some(ref callback) = self.operator_callback {
            jetstream = jetstream.with_operator_callback(Arc::clone(callback));
        }

        let jetstream_handle = {
            let shutdown_rx = shutdown_rx.clone();
            tokio::spawn(async move {
                if let Err(e) = jetstream.run(shutdown_rx).await {
                    error!(error = %e, "jetstream task failed");
                }
            })
        };

        info!(
            facts = self.cache.fact_count(),
            rules = self.cache.rule_count(),
            "sync coordinator is live"
        );

        Ok(jetstream_handle)
    }

    /// Populate the cache by downloading the full repo as a CAR file.
    ///
    /// This is much faster than fetching per-collection via list_all_records
    /// because it's a single HTTP request for the entire repo.
    async fn populate_cache(&self) -> Result<(), AtprotoError> {
        // Suppress broadcasts during bulk population
        self.cache.set_suppress_broadcasts(true);

        // Download full repo as CAR
        let (car_bytes, _rev) = self.client.get_repo(&self.did).await?;
        info!(bytes = car_bytes.len(), "downloaded repo CAR file");

        // Parse CAR and extract all records
        let parsed = car::parse_car(&car_bytes).await?;

        // Set repo revision
        if let Some(ref rev) = parsed.rev {
            self.cache.set_repo_rev(rev.clone()).await;
        }

        // Populate cache from parsed CAR result
        self.cache.populate_from_car_full(
            parsed.facts.into_iter().map(|(k, (v, c))| (k, v, c)),
            parsed.rules.into_iter().map(|(k, (v, c))| (k, v, c)),
            parsed.thoughts.into_iter().map(|(k, (v, c))| (k, v, c)),
            parsed.notes.into_iter().map(|(k, (v, c))| (k, v, c)),
            parsed.jobs.into_iter().map(|(k, (v, c))| (k, v, c)),
            parsed.identity,
            parsed.follows.into_iter().map(|(k, (v, c))| (k, v, c)),
            parsed.likes.into_iter().map(|(k, (v, c))| (k, v, c)),
            parsed.reposts.into_iter().map(|(k, (v, c))| (k, v, c)),
            parsed.posts.into_iter().map(|(k, (v, c))| (k, v, c)),
            parsed.directives.into_iter().map(|(k, (v, c))| (k, v, c)),
            parsed.declarations.into_iter().map(|(k, (v, c))| (k, v, c)),
            parsed.tools.into_iter().map(|(k, (v, c))| (k, v, c)),
            parsed.tool_approvals.into_iter().map(|(k, (v, c))| (k, v, c)),
            parsed.blog_entries.into_iter().map(|(k, (v, c))| (k, v, c)),
            parsed.wiki_entries.into_iter().map(|(k, (v, c))| (k, v, c)),
            parsed.wiki_links.into_iter().map(|(k, (v, c))| (k, v, c)),
            parsed.triggers.into_iter().map(|(k, (v, c))| (k, v, c)),
        );

        // Set identity and daemon state from CAR (singletons handled separately)
        if let Some((identity, cid)) = parsed.daemon_state {
            // Note: daemon_state is set here, identity was already set via populate_from_car_full
            self.cache.set_daemon_state(identity, cid).await;
        }

        // Re-enable broadcasts
        self.cache.set_suppress_broadcasts(false);

        Ok(())
    }
}

/// Builder for creating a SyncCoordinator with optional configuration.
pub struct SyncCoordinatorBuilder {
    client: AtprotoClient,
    did: String,
    jetstream_url: Option<String>,
    cache: Option<Arc<RepoCache>>,
    operator_did: Option<String>,
    operator_callback: Option<OperatorEventCallback>,
}

impl SyncCoordinatorBuilder {
    /// Create a new builder.
    pub fn new(client: AtprotoClient, did: impl Into<String>) -> Self {
        Self {
            client,
            did: did.into(),
            jetstream_url: None,
            cache: None,
            operator_did: None,
            operator_callback: None,
        }
    }

    /// Set a custom Jetstream URL.
    pub fn jetstream_url(mut self, url: impl Into<String>) -> Self {
        self.jetstream_url = Some(url.into());
        self
    }

    /// Use an existing cache.
    pub fn cache(mut self, cache: Arc<RepoCache>) -> Self {
        self.cache = Some(cache);
        self
    }

    /// Set the operator DID.
    pub fn operator_did(mut self, did: impl Into<String>) -> Self {
        self.operator_did = Some(did.into());
        self
    }

    /// Set the operator event callback.
    pub fn operator_callback(mut self, callback: OperatorEventCallback) -> Self {
        self.operator_callback = Some(callback);
        self
    }

    /// Build the sync coordinator.
    #[allow(clippy::unwrap_or_default)]
    pub fn build(self) -> SyncCoordinator {
        let cache = self.cache.unwrap_or_else(RepoCache::new);
        let mut coordinator = SyncCoordinator::new(self.client, self.did, cache);

        if let Some(url) = self.jetstream_url {
            coordinator = coordinator.with_jetstream_url(url);
        }

        if let Some(did) = self.operator_did {
            coordinator = coordinator.with_operator_did(did);
        }

        if let Some(callback) = self.operator_callback {
            coordinator = coordinator.with_operator_callback(callback);
        }

        coordinator
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_builder() {
        let client = AtprotoClient::new("https://example.com");
        let coordinator = SyncCoordinatorBuilder::new(client, "did:plc:test")
            .jetstream_url("wss://custom.jetstream")
            .build();

        assert_eq!(coordinator.did, "did:plc:test");
        assert_eq!(coordinator.jetstream_url, "wss://custom.jetstream");
    }

    #[test]
    fn test_builder_with_operator() {
        let client = AtprotoClient::new("https://example.com");
        let coordinator = SyncCoordinatorBuilder::new(client, "did:plc:test")
            .operator_did("did:plc:operator")
            .build();

        assert_eq!(coordinator.operator_did, Some("did:plc:operator".to_string()));
    }
}
