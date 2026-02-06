//! Sync coordinator for cache hydration and firehose subscription.
//!
//! Orchestrates the startup sequence:
//! 1. Start firehose (queue events)
//! 2. Fetch CAR file
//! 3. Parse and populate cache
//! 4. Replay queued events
//! 5. Go live

use std::sync::Arc;

use tokio::sync::watch;
use tokio::task::JoinHandle;
use tracing::{debug, error, info, warn};

use crate::cache::{RepoCache, SyncState};
use crate::car::parse_car;
use crate::firehose::{DEFAULT_FIREHOSE_URL, FirehoseClient, apply_commit};
use crate::{AtprotoClient, AtprotoError};

/// Sync coordinator for managing cache synchronization.
pub struct SyncCoordinator {
    /// ATProto client for fetching CAR files.
    client: AtprotoClient,
    /// DID of the repository to sync.
    did: String,
    /// Firehose URL.
    firehose_url: String,
    /// Cache to populate.
    cache: Arc<RepoCache>,
}

impl SyncCoordinator {
    /// Create a new sync coordinator.
    pub fn new(client: AtprotoClient, did: impl Into<String>, cache: Arc<RepoCache>) -> Self {
        Self {
            client,
            did: did.into(),
            firehose_url: DEFAULT_FIREHOSE_URL.to_string(),
            cache,
        }
    }

    /// Set a custom firehose URL.
    pub fn with_firehose_url(mut self, url: impl Into<String>) -> Self {
        self.firehose_url = url.into();
        self
    }

    /// Get the cache.
    pub fn cache(&self) -> Arc<RepoCache> {
        Arc::clone(&self.cache)
    }

    /// Start synchronization.
    ///
    /// This spawns a firehose listener task, fetches the CAR file,
    /// populates the cache, replays queued events, and goes live.
    ///
    /// Returns a handle to the firehose task.
    pub async fn start(
        &self,
        shutdown_rx: watch::Receiver<bool>,
    ) -> Result<JoinHandle<()>, AtprotoError> {
        info!(did = %self.did, firehose = %self.firehose_url, "starting sync coordinator");

        // Clear any stale pending events from a previous sync attempt
        // This prevents unbounded queue growth on reconnection
        self.cache.clear_pending().await;

        // Set state to syncing
        self.cache.set_state(SyncState::Syncing);

        // 1. Spawn firehose listener (starts queuing events immediately)
        let firehose = FirehoseClient::new(
            self.firehose_url.clone(),
            self.did.clone(),
            Arc::clone(&self.cache),
        );

        let firehose_handle = {
            let shutdown_rx = shutdown_rx.clone();
            tokio::spawn(async move {
                if let Err(e) = firehose.run(shutdown_rx).await {
                    error!(error = %e, "firehose task failed");
                }
            })
        };

        // Give the firehose a moment to connect
        tokio::time::sleep(std::time::Duration::from_millis(500)).await;

        // 2. Fetch CAR file
        info!(did = %self.did, "fetching repository CAR");
        let (car_bytes, repo_rev) = self.client.get_repo(&self.did).await?;

        info!(
            size = car_bytes.len(),
            rev = ?repo_rev,
            "fetched CAR file"
        );

        // 3. Parse CAR and populate cache
        let parse_result = parse_car(&car_bytes).await?;

        // Store the repo revision
        if let Some(ref rev) = parse_result.rev {
            self.cache.set_repo_rev(rev.clone()).await;
        } else if let Some(ref rev) = repo_rev {
            self.cache.set_repo_rev(rev.clone()).await;
        }

        // Populate cache from parsed CAR
        let facts = parse_result
            .facts
            .into_iter()
            .map(|(rkey, (fact, cid))| (rkey, fact, cid));
        let rules = parse_result
            .rules
            .into_iter()
            .map(|(rkey, (rule, cid))| (rkey, rule, cid));
        let thoughts = parse_result
            .thoughts
            .into_iter()
            .map(|(rkey, (thought, cid))| (rkey, thought, cid));
        let notes = parse_result
            .notes
            .into_iter()
            .map(|(rkey, (note, cid))| (rkey, note, cid));
        let jobs = parse_result
            .jobs
            .into_iter()
            .map(|(rkey, (job, cid))| (rkey, job, cid));
        let follows = parse_result
            .follows
            .into_iter()
            .map(|(rkey, (follow, cid))| (rkey, follow, cid));
        let likes = parse_result
            .likes
            .into_iter()
            .map(|(rkey, (like, cid))| (rkey, like, cid));
        let reposts = parse_result
            .reposts
            .into_iter()
            .map(|(rkey, (repost, cid))| (rkey, repost, cid));
        let posts = parse_result
            .posts
            .into_iter()
            .map(|(rkey, (post, cid))| (rkey, post, cid));
        let directives = parse_result
            .directives
            .into_iter()
            .map(|(rkey, (directive, cid))| (rkey, directive, cid));
        let declarations = parse_result
            .declarations
            .into_iter()
            .map(|(rkey, (declaration, cid))| (rkey, declaration, cid));
        let tools = parse_result
            .tools
            .into_iter()
            .map(|(rkey, (tool, cid))| (rkey, tool, cid));
        let tool_approvals = parse_result
            .tool_approvals
            .into_iter()
            .map(|(rkey, (approval, cid))| (rkey, approval, cid));
        let blog_entries = parse_result
            .blog_entries
            .into_iter()
            .map(|(rkey, (entry, cid))| (rkey, entry, cid));
        let wiki_entries = parse_result
            .wiki_entries
            .into_iter()
            .map(|(rkey, (entry, cid))| (rkey, entry, cid));
        let wiki_links = parse_result
            .wiki_links
            .into_iter()
            .map(|(rkey, (link, cid))| (rkey, link, cid));

        self.cache.populate_from_car_full(
            facts,
            rules,
            thoughts,
            notes,
            jobs,
            parse_result.identity,
            follows,
            likes,
            reposts,
            posts,
            directives,
            declarations,
            tools,
            tool_approvals,
            blog_entries,
            wiki_entries,
            wiki_links,
        );

        // Populate daemon state if present (contains followers list)
        if let Some((state, cid)) = parse_result.daemon_state {
            self.cache.set_daemon_state(state, cid).await;
        }

        info!(
            facts = self.cache.fact_count(),
            rules = self.cache.rule_count(),
            "cache populated from CAR"
        );

        // 4. Replay queued firehose events
        // Suppress broadcasts during replay to prevent broadcast channel lag,
        // which would trigger expensive full TSV regeneration in DatalogCache.
        // The Synchronized event (sent when going Live) will trigger
        // populate_from_repo_cache() to do a full sync instead.
        let current_rev = self.cache.repo_rev().await;
        let pending = self.cache.drain_pending().await;

        debug!(
            pending = pending.len(),
            "replaying pending firehose events (broadcasts suppressed)"
        );
        self.cache.set_suppress_broadcasts(true);

        for commit in pending {
            // Skip events already included in CAR (based on revision comparison)
            if let Some(ref car_rev) = current_rev
                && commit.rev <= *car_rev
            {
                trace_skip_commit(&commit.rev, car_rev);
                continue;
            }

            // Apply the commit
            if let Err(e) = apply_commit(&self.cache, &commit) {
                warn!(rev = %commit.rev, error = %e, "failed to apply pending commit");
            }
        }

        // Re-enable broadcasts before going live
        self.cache.set_suppress_broadcasts(false);

        // 5. Go live
        self.cache.set_state(SyncState::Live);

        info!(
            facts = self.cache.fact_count(),
            rules = self.cache.rule_count(),
            "sync coordinator is live"
        );

        Ok(firehose_handle)
    }
}

fn trace_skip_commit(commit_rev: &str, car_rev: &str) {
    tracing::trace!(
        commit_rev = %commit_rev,
        car_rev = %car_rev,
        "skipping commit already in CAR"
    );
}

/// Builder for creating a SyncCoordinator with optional configuration.
pub struct SyncCoordinatorBuilder {
    client: AtprotoClient,
    did: String,
    firehose_url: Option<String>,
    cache: Option<Arc<RepoCache>>,
}

impl SyncCoordinatorBuilder {
    /// Create a new builder.
    pub fn new(client: AtprotoClient, did: impl Into<String>) -> Self {
        Self {
            client,
            did: did.into(),
            firehose_url: None,
            cache: None,
        }
    }

    /// Set a custom firehose URL.
    pub fn firehose_url(mut self, url: impl Into<String>) -> Self {
        self.firehose_url = Some(url.into());
        self
    }

    /// Use an existing cache.
    pub fn cache(mut self, cache: Arc<RepoCache>) -> Self {
        self.cache = Some(cache);
        self
    }

    /// Build the sync coordinator.
    #[allow(clippy::unwrap_or_default)]
    pub fn build(self) -> SyncCoordinator {
        let cache = self.cache.unwrap_or_else(RepoCache::new);
        let mut coordinator = SyncCoordinator::new(self.client, self.did, cache);

        if let Some(url) = self.firehose_url {
            coordinator = coordinator.with_firehose_url(url);
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
            .firehose_url("wss://custom.relay")
            .build();

        assert_eq!(coordinator.did, "did:plc:test");
        assert_eq!(coordinator.firehose_url, "wss://custom.relay");
    }
}
