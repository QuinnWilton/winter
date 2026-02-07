//! Daemon command for running Winter's main loop.
//!
//! The daemon uses a persistent session architecture:
//! - Single persistent Claude Code session polling an inbox for work
//! - Dedicated DM poller (pushes to inbox at priority 200 for operator, 150 for others)
//! - Notification poller (pushes to inbox at priority 100)
//! - Scheduler (pushes jobs to inbox at priority 50)
//! - Watchdog for detecting stuck sessions

use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use miette::Result;
use tokio::sync::watch;
use tracing::{debug, error, info, warn};

use winter_agent::{
    Agent, AgentContext, ContextTrigger, ConversationHistoryMessage, IdentityManager,
    StateManager,
};
use winter_atproto::{
    AtprotoClient, DIRECTIVE_COLLECTION, Directive, OperatorEvent, RULE_COLLECTION, RepoCache,
    Rule, ScopeFilter, SyncCoordinator, SyncState, THOUGHT_COLLECTION, Thought,
};
use winter_datalog::DatalogCache;
use winter_mcp::InterruptionState;
use winter_mcp::bluesky::NotificationReason;
use winter_mcp::tools::inbox::{
    ConversationHistoryMessage as InboxConversationHistoryMessage, InboxItem,
    PostRef as InboxPostRef,
};
use winter_mcp::{BlueskyClient, BlueskyError};
use winter_scheduler::Scheduler;

/// Default DM poll interval in seconds.
const DEFAULT_DM_POLL_INTERVAL: u64 = 5;

/// Default notification poll interval in seconds.
const DEFAULT_NOTIF_POLL_INTERVAL: u64 = 10;

/// Configuration for the daemon.
pub struct DaemonConfig {
    pub pds_url: String,
    pub handle: String,
    pub app_password: String,
    pub poll_interval: u64,
    pub mcp_config_path: PathBuf,
    /// Interval in seconds for syncing followers from the Bluesky API.
    pub follower_sync_interval: u64,
    /// If true, fast-forward notification and DM cursors to current time on startup.
    /// This skips all existing notifications/DMs and only processes new ones.
    pub fast_forward: bool,
    /// DM poll interval in seconds (default 5).
    pub dm_poll_interval: Option<u64>,
    /// Notification poll interval in seconds (default 10).
    pub notif_poll_interval: Option<u64>,
}

/// Fetch deduplicated rule heads from the PDS or cache.
/// Returns heads like "mutual_follow(X, Y)" for use in queries.
async fn fetch_rule_heads(client: &AtprotoClient, cache: Option<&RepoCache>) -> Vec<String> {
    // Try cache first
    if let Some(cache) = cache
        && cache.state() == SyncState::Live
    {
        return cache.enabled_rule_heads();
    }

    // Fall back to HTTP
    match client.list_all_records::<Rule>(RULE_COLLECTION).await {
        Ok(records) => {
            let mut heads: Vec<String> = records
                .into_iter()
                .filter(|r| r.value.enabled)
                .map(|r| r.value.head)
                .collect();
            heads.sort();
            heads.dedup();
            heads
        }
        Err(e) => {
            warn!(error = %e, "failed to fetch rule heads for context");
            Vec::new()
        }
    }
}

/// Fetch recent thoughts filtered by conversation scope.
///
/// Filters thoughts to only include those relevant to the current conversation,
/// preventing cross-contamination when multiple workers process notifications concurrently.
async fn fetch_recent_thoughts_scoped(
    client: &AtprotoClient,
    cache: Option<&RepoCache>,
    limit: usize,
    scope: &ScopeFilter,
) -> Vec<Thought> {
    // Try cache first
    if let Some(cache) = cache
        && cache.state() == SyncState::Live
    {
        return cache.recent_thoughts_for_scope(limit, scope);
    }

    // Fall back to HTTP - fetch more and post-filter
    // We fetch extra records since we'll filter some out
    let fetch_limit = limit * 3;
    match client
        .list_records::<Thought>(THOUGHT_COLLECTION, Some(fetch_limit as u32), None)
        .await
    {
        Ok(response) => {
            let mut thoughts: Vec<Thought> = response
                .records
                .into_iter()
                .map(|r| r.value)
                .filter(|t| thought_matches_scope_filter(t, scope))
                .collect();
            thoughts.reverse();
            thoughts.truncate(limit);
            thoughts
        }
        Err(e) => {
            warn!(error = %e, "failed to fetch recent thoughts for context");
            Vec::new()
        }
    }
}

/// Check if a thought matches the given scope filter.
/// Mirrors the logic in cache.rs for HTTP fallback path.
fn thought_matches_scope_filter(thought: &Thought, scope: &ScopeFilter) -> bool {
    match &thought.trigger {
        None => true, // Global thoughts always match
        Some(trigger) => match scope {
            ScopeFilter::Thread { root_uri } => {
                trigger.starts_with("notification:")
                    && trigger.ends_with(&format!(":root={}", root_uri))
            }
            ScopeFilter::DirectMessage { convo_id } => {
                trigger.starts_with(&format!("dm:{}:", convo_id))
            }
            ScopeFilter::Job { name } => trigger == &format!("job:{}", name),
            ScopeFilter::Global => false,
        },
    }
}

/// Fetch active directives from the PDS or cache.
/// Returns only active directives, sorted by priority (descending) then created_at.
async fn fetch_directives(client: &AtprotoClient, cache: Option<&RepoCache>) -> Vec<Directive> {
    // Try cache first
    if let Some(cache) = cache
        && cache.state() == SyncState::Live
    {
        return cache.active_directives_sorted();
    }

    // Fall back to HTTP
    match client
        .list_all_records::<Directive>(DIRECTIVE_COLLECTION)
        .await
    {
        Ok(records) => {
            let mut directives: Vec<Directive> = records
                .into_iter()
                .map(|r| r.value)
                .filter(|d| d.active)
                .collect();
            // Sort by priority (descending) then created_at
            directives.sort_by(|a, b| {
                b.priority
                    .cmp(&a.priority)
                    .then_with(|| a.created_at.cmp(&b.created_at))
            });
            directives
        }
        Err(e) => {
            warn!(error = %e, "failed to fetch directives for context");
            Vec::new()
        }
    }
}

/// Run the daemon.
pub async fn run(
    pds_url: &str,
    handle: &str,
    app_password: &str,
    poll_interval: u64,
    follower_sync_interval: u64,
    fast_forward: bool,
) -> Result<()> {
    // Use HTTP MCP config when WINTER_MCP_URL is set (Docker environment),
    // otherwise fall back to stdio config for local development
    let mcp_config_path = if std::env::var("WINTER_MCP_URL").is_ok() {
        // Docker: use HTTP transport config
        dirs::home_dir()
            .map(|h| h.join(".config/winter/mcp-http.json"))
            .unwrap_or_else(|| PathBuf::from("/etc/winter/mcp-http.json"))
    } else {
        // Local: use stdio transport config
        dirs::home_dir()
            .map(|h| h.join(".config/winter/mcp.json"))
            .unwrap_or_else(|| PathBuf::from("/etc/winter/mcp.json"))
    };

    run_with_config(DaemonConfig {
        pds_url: pds_url.to_string(),
        handle: handle.to_string(),
        app_password: app_password.to_string(),
        poll_interval,
        mcp_config_path,
        follower_sync_interval,
        fast_forward,
        dm_poll_interval: None,
        notif_poll_interval: None,
    })
    .await
}

/// Run the daemon with full configuration.
pub async fn run_with_config(config: DaemonConfig) -> Result<()> {
    info!("starting Winter daemon");

    // Read configuration from env vars with defaults
    // Use specific intervals if provided, fall back to poll_interval, then to defaults
    let dm_poll_interval = Duration::from_secs(config.dm_poll_interval.unwrap_or_else(|| {
        std::env::var("WINTER_DM_POLL_INTERVAL")
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or(if config.poll_interval > 0 {
                config.poll_interval
            } else {
                DEFAULT_DM_POLL_INTERVAL
            })
    }));
    let notif_poll_interval =
        Duration::from_secs(config.notif_poll_interval.unwrap_or_else(|| {
            std::env::var("WINTER_NOTIF_POLL_INTERVAL")
                .ok()
                .and_then(|s| s.parse().ok())
                .unwrap_or(if config.poll_interval > 0 {
                    config.poll_interval
                } else {
                    DEFAULT_NOTIF_POLL_INTERVAL
                })
        }));

    info!(
        dm_poll_interval_secs = dm_poll_interval.as_secs(),
        notif_poll_interval_secs = notif_poll_interval.as_secs(),
        "daemon configuration"
    );

    // Create a single shared ATProto client for all operations
    // This enables HTTP connection pooling and reduces authentication overhead
    let client = Arc::new(AtprotoClient::new(&config.pds_url));
    client
        .login(&config.handle, &config.app_password)
        .await
        .map_err(|e| miette::miette!("{}", e))?;

    // Create dedicated ATProto client for DM operations
    // This prevents DM polling from being blocked by heavy tool calls in background sessions
    let dm_client = Arc::new(AtprotoClient::new(&config.pds_url));
    dm_client
        .login(&config.handle, &config.app_password)
        .await
        .map_err(|e| miette::miette!("failed to login DM client: {}", e))?;

    // Create identity manager with shared client
    let identity_manager = Arc::new(IdentityManager::new(Arc::clone(&client)));

    // Load identity
    let identity = match identity_manager.load().await {
        Ok(id) => id,
        Err(e) => {
            error!(error = ?e, "failed to load identity - run 'winter bootstrap' first");
            return Err(miette::miette!(
                "identity not found - run 'winter bootstrap' first"
            ));
        }
    };

    let operator_did = identity.operator_did.clone();
    info!(
        operator_did = %operator_did,
        "identity loaded"
    );

    // Create state manager with shared client
    let state_manager = Arc::new(StateManager::new(Arc::clone(&client)));

    let notification_cursor = match state_manager.get_notification_cursor().await {
        Ok(cursor) => {
            if cursor.is_some() {
                info!(cursor = ?cursor, "loaded notification cursor from state");
            }
            cursor
        }
        Err(e) => {
            warn!(error = %e, "failed to load notification cursor, starting fresh");
            None
        }
    };

    let dm_cursor = match state_manager.get_dm_cursor().await {
        Ok(cursor) => {
            if cursor.is_some() {
                info!(cursor = ?cursor, "loaded DM cursor from state");
            }
            cursor
        }
        Err(e) => {
            warn!(error = %e, "failed to load DM cursor, starting fresh");
            None
        }
    };

    // Get DID for sync coordinator
    let did = client
        .did()
        .await
        .ok_or_else(|| miette::miette!("no DID available after login"))?;

    // Create cache and sync coordinator
    let cache = RepoCache::new();

    let sync_client = AtprotoClient::new(&config.pds_url);
    sync_client
        .login(&config.handle, &config.app_password)
        .await
        .map_err(|e| miette::miette!("{}", e))?;

    // Build operator event callback for tool approvals → inbox
    let operator_http_client = Arc::new(reqwest::Client::new());
    let operator_mcp_base_url = Arc::new(
        std::env::var("WINTER_MCP_URL")
            .ok()
            .and_then(|url| url.strip_suffix("/mcp").map(String::from))
            .unwrap_or_else(|| "http://127.0.0.1:3847".to_string()),
    );
    let operator_callback: winter_atproto::OperatorEventCallback = {
        let http_client = Arc::clone(&operator_http_client);
        let mcp_base_url = Arc::clone(&operator_mcp_base_url);
        Arc::new(move |event: OperatorEvent| {
            match event {
                OperatorEvent::ToolApproval { rkey, approval } => {
                    if approval.status == winter_atproto::ToolApprovalStatus::Approved {
                        let tool_rkey = approval.tool_rkey.clone();
                        let http_client = Arc::clone(&http_client);
                        let mcp_base_url = Arc::clone(&mcp_base_url);
                        let tool_name = format!("tool:{}", tool_rkey);
                        let item = InboxItem::tool_approved(
                            tool_name,
                            tool_rkey,
                            rkey,
                        );
                        // Fire-and-forget push to inbox
                        tokio::spawn(async move {
                            push_inbox_item(&http_client, &mcp_base_url, item).await;
                        });
                    }
                }
            }
        })
    };

    let mut sync_coordinator = SyncCoordinator::new(sync_client, &did, Arc::clone(&cache));
    sync_coordinator = sync_coordinator.with_operator_did(&operator_did);
    sync_coordinator = sync_coordinator.with_operator_callback(operator_callback);

    // Create shutdown channel
    let (shutdown_tx, shutdown_rx) = watch::channel(false);

    // Handle shutdown signals
    let shutdown_tx_clone = shutdown_tx.clone();
    tokio::spawn(async move {
        tokio::signal::ctrl_c().await.ok();
        info!("received shutdown signal");
        let _ = shutdown_tx_clone.send(true);
    });

    // Start sync coordinator (list_all_records + Jetstream)
    let sync_handle = match sync_coordinator.start(shutdown_rx.clone()).await {
        Ok(handle) => {
            info!("sync coordinator started");
            Some(handle)
        }
        Err(e) => {
            warn!(error = %e, "sync coordinator failed to start, continuing without cache");
            None
        }
    };

    // Create DatalogCache for derived facts (followers, etc.)
    let cache_dir = dirs::cache_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("winter");
    let datalog_cache = match DatalogCache::new_with_did(
        &cache_dir,
        Some(did.clone()),
        Some(config.handle.clone()),
    ) {
        Ok(cache) => {
            info!(cache_dir = %cache_dir.display(), "datalog cache initialized");
            Some(cache)
        }
        Err(e) => {
            warn!(error = %e, "failed to create datalog cache, follower sync disabled");
            None
        }
    };

    // Connect DatalogCache to RepoCache for derived facts
    // This starts a background listener that will automatically populate
    // the DatalogCache when the RepoCache becomes synchronized
    if let Some(ref dc) = datalog_cache {
        dc.start_update_listener(Arc::clone(&cache));
        info!("datalog cache connected to repo cache");
    }


    // Create Bluesky client for notification polling
    let mut notif_bluesky =
        BlueskyClient::new(&config.pds_url, &config.handle, &config.app_password)
            .await
            .map_err(|e| miette::miette!("failed to create Bluesky client: {}", e))?;

    // Initialize with persisted cursors
    notif_bluesky.set_last_seen_at(notification_cursor);

    // Create separate Bluesky client for DM polling (dedicated)
    let mut dm_bluesky = BlueskyClient::new(&config.pds_url, &config.handle, &config.app_password)
        .await
        .map_err(|e| miette::miette!("failed to create DM Bluesky client: {}", e))?;
    dm_bluesky.set_last_dm_cursor(dm_cursor);

    // Fast-forward: skip all existing notifications and DMs
    if config.fast_forward {
        info!("fast-forward mode enabled, catching up to current state");

        // Fetch latest notifications to get the current timestamp
        match notif_bluesky.get_notifications(Some(1)).await {
            Ok(_) => {
                // get_notifications updates last_seen_at internally
                if let Some(cursor) = notif_bluesky.last_seen_at() {
                    info!(cursor = %cursor, "fast-forwarded notification cursor");
                    if let Err(e) = state_manager
                        .set_notification_cursor(Some(cursor.to_string()))
                        .await
                    {
                        warn!(error = %e, "failed to persist fast-forwarded notification cursor");
                    }
                }
            }
            Err(e) => {
                warn!(error = %e, "failed to fetch notifications for fast-forward");
            }
        }

        // Fetch latest DMs to get the current timestamp
        match dm_bluesky.get_unread_dms().await {
            Ok(_) => {
                // get_unread_dms updates last_dm_cursor internally
                if let Some(cursor) = dm_bluesky.last_dm_cursor() {
                    info!(cursor = %cursor, "fast-forwarded DM cursor");
                    if let Err(e) = state_manager.set_dm_cursor(Some(cursor.to_string())).await {
                        warn!(error = %e, "failed to persist fast-forwarded DM cursor");
                    }
                }
            }
            Err(e) => {
                warn!(error = %e, "failed to fetch DMs for fast-forward");
            }
        }

        info!("fast-forward complete, daemon will only process new notifications");
    }

    // Create scheduler with shared client
    let scheduler = Arc::new(Scheduler::new(Arc::clone(&client)));

    // Load existing jobs
    if let Err(e) = scheduler.load_jobs().await {
        error!(error = %e, "failed to load jobs, starting with empty job list");
    }

    // Connect scheduler to repo cache for live job updates
    // This enables the scheduler to pick up job changes made via MCP tools
    scheduler.start_update_listener(Arc::clone(&cache));

    // Create agent for Claude invocation
    let agent = Arc::new(Agent::new(&config.mcp_config_path));

    // HTTP client and MCP base URL for pushing inbox items to the MCP server
    let http_client = Arc::new(reqwest::Client::new());
    let mcp_base_url = Arc::new(
        std::env::var("WINTER_MCP_URL")
            .ok()
            .and_then(|url| url.strip_suffix("/mcp").map(String::from))
            .unwrap_or_else(|| "http://127.0.0.1:3847".to_string()),
    );

    // Create shared interruption state for background sessions
    let interruption_state = Arc::new(InterruptionState::new());

    // Create job executor that pushes to inbox via HTTP
    let executor: winter_scheduler::JobExecutor = {
        let http_client = Arc::clone(&http_client);
        let mcp_base_url = Arc::clone(&mcp_base_url);

        Box::new(move |job| {
            let http_client = Arc::clone(&http_client);
            let mcp_base_url = Arc::clone(&mcp_base_url);

            Box::pin(async move {
                info!(name = %job.name, "scheduling job to inbox");

                let item = InboxItem::job(job.name.clone(), job.instructions.clone());
                push_inbox_item(&http_client, &mcp_base_url, item).await;

                Ok(())
            })
        })
    };

    // Spawn dedicated DM poller (pushes to inbox via HTTP)
    let dm_handle = {
        let operator_did = operator_did.clone();
        let state_manager = Arc::clone(&state_manager);
        let mut shutdown_rx = shutdown_rx.clone();
        let http_client = Arc::clone(&http_client);
        let mcp_base_url = Arc::clone(&mcp_base_url);
        let interruption_state = Arc::clone(&interruption_state);

        tokio::spawn(async move {
            info!("DM poller started");
            let mut interval = tokio::time::interval(dm_poll_interval);

            loop {
                tokio::select! {
                    biased;

                    _ = shutdown_rx.changed() => {
                        if *shutdown_rx.borrow() {
                            break;
                        }
                    }

                    _ = interval.tick() => {
                        match dm_bluesky.get_unread_dms().await {
                            Ok(dms) => {
                                if dms.is_empty() {
                                    continue;
                                }

                                for dm in dms {
                                    let is_operator = dm.sender_did == operator_did;
                                    info!(
                                        sender = %dm.sender_did,
                                        convo_id = %dm.convo_id,
                                        text = %dm.text,
                                        is_operator,
                                        "pushing DM to inbox"
                                    );

                                    // Persist DM cursor BEFORE pushing to inbox
                                    if let Some(cursor) = dm_bluesky.last_dm_cursor() {
                                        debug!(cursor = %cursor, "persisting DM cursor");
                                        if let Err(e) = state_manager.set_dm_cursor(Some(cursor.to_string())).await {
                                            warn!(error = %e, "failed to persist DM cursor");
                                        }
                                    }

                                    // Resolve sender handle
                                    let sender_handle = format!("did:{}", &dm.sender_did[4..]);

                                    // Fetch conversation history
                                    let history = fetch_dm_history(&dm_bluesky, &dm.convo_id, &dm.id, dm.sent_at).await;
                                    let inbox_history: Vec<InboxConversationHistoryMessage> = history
                                        .into_iter()
                                        .map(|h| InboxConversationHistoryMessage {
                                            sender_label: h.sender_label,
                                            text: h.text,
                                            sent_at: h.sent_at,
                                        })
                                        .collect();

                                    // Push to inbox via HTTP
                                    let priority = if is_operator { 200 } else { 150 };
                                    let item = InboxItem::direct_message(
                                        dm.sender_did.clone(),
                                        sender_handle,
                                        dm.convo_id.clone(),
                                        dm.id.clone(),
                                        dm.text.clone(),
                                        dm.facets.clone(),
                                        inbox_history,
                                        priority,
                                    );
                                    push_inbox_item(&http_client, &mcp_base_url, item).await;

                                    // Signal interruption only for operator DMs
                                    if is_operator {
                                        interruption_state.set_interrupt("operator_dm").await;
                                    }
                                }
                            }
                            Err(e) => {
                                warn!(error = %e, "DM poll failed");
                            }
                        }
                    }
                }
            }

            info!("DM poller stopped");
        })
    };

    // Spawn notification poller (pushes to inbox via HTTP)
    let notif_handle = {
        let state_manager = Arc::clone(&state_manager);
        let datalog_cache = datalog_cache.clone();
        let mut shutdown_rx = shutdown_rx.clone();
        let http_client = Arc::clone(&http_client);
        let mcp_base_url = Arc::clone(&mcp_base_url);
        let interruption_state = Arc::clone(&interruption_state);

        tokio::spawn(async move {
            info!("notification poller started");
            let mut interval = tokio::time::interval(notif_poll_interval);
            let mut rate_limit_backoff = Duration::from_secs(0);

            loop {
                if rate_limit_backoff > Duration::ZERO {
                    debug!(
                        backoff_secs = rate_limit_backoff.as_secs(),
                        "rate limit backoff"
                    );
                    tokio::time::sleep(rate_limit_backoff).await;
                }

                tokio::select! {
                    biased;

                    _ = shutdown_rx.changed() => {
                        if *shutdown_rx.borrow() {
                            break;
                        }
                    }

                    _ = interval.tick() => {
                        match notif_bluesky.get_notifications(Some(50)).await {
                            Ok(notifications) => {
                                rate_limit_backoff = Duration::ZERO;

                                for notif in &notifications {
                                    // Handle Follow notifications incrementally
                                    if notif.reason == NotificationReason::Follow {
                                        if let Some(ref dc) = datalog_cache
                                            && dc.add_follower(notif.author_did.clone()).await
                                        {
                                            info!(
                                                follower = %notif.author_did,
                                                handle = %notif.author_handle,
                                                "new follower added"
                                            );
                                            if let Err(e) = dc.flush_dirty_predicates().await {
                                                warn!(error = %e, "failed to flush follower update");
                                            }
                                        }
                                        continue;
                                    }

                                    // Skip non-wakeup notifications (likes, reposts)
                                    if !notif.reason.triggers_wakeup() {
                                        debug!(
                                            reason = ?notif.reason,
                                            author = %notif.author_handle,
                                            "skipping non-wakeup notification"
                                        );
                                        continue;
                                    }

                                    // Convert notification reason to kind string
                                    let kind = match notif.reason {
                                        NotificationReason::Mention => "mention",
                                        NotificationReason::Reply => "reply",
                                        NotificationReason::Quote => "quote",
                                        _ => "notification",
                                    };

                                    // Push to inbox via HTTP
                                    let item = InboxItem::notification(
                                        notif.author_did.clone(),
                                        notif.author_handle.clone(),
                                        kind.to_string(),
                                        notif.text.clone(),
                                        notif.uri.clone(),
                                        notif.cid.clone(),
                                        notif.parent.as_ref().map(|p| InboxPostRef {
                                            uri: p.uri.clone(),
                                            cid: p.cid.clone(),
                                        }),
                                        notif.root.as_ref().map(|r| InboxPostRef {
                                            uri: r.uri.clone(),
                                            cid: r.cid.clone(),
                                        }),
                                        notif.facets.clone(),
                                    );
                                    push_inbox_item(&http_client, &mcp_base_url, item).await;

                                    // Signal interruption for pending notifications
                                    interruption_state.set_interrupt("inbox_items").await;
                                }

                                // Persist cursor after all notifications pushed
                                if let Some(cursor) = notif_bluesky.last_seen_at() {
                                    debug!(cursor = %cursor, "persisting notification cursor");
                                    if let Err(e) = state_manager
                                        .set_notification_cursor(Some(cursor.to_string()))
                                        .await
                                    {
                                        warn!(error = %e, "failed to persist notification cursor");
                                    }
                                }
                            }
                            Err(BlueskyError::RateLimited { endpoint }) => {
                                warn!(endpoint = ?endpoint, "notification poll rate limited");
                                rate_limit_backoff = if rate_limit_backoff.is_zero() {
                                    Duration::from_secs(5)
                                } else {
                                    (rate_limit_backoff * 2).min(Duration::from_secs(300))
                                };
                            }
                            Err(e) => {
                                warn!(error = %e, "notification poll failed");
                            }
                        }
                    }
                }
            }

            info!("notification poller stopped");
        })
    };

    // Spawn scheduler task
    let scheduler_handle = {
        let scheduler = Arc::clone(&scheduler);
        let mut shutdown_rx = shutdown_rx.clone();

        tokio::spawn(async move {
            info!("scheduler started");

            loop {
                tokio::select! {
                    biased;

                    _ = shutdown_rx.changed() => {
                        if *shutdown_rx.borrow() {
                            break;
                        }
                    }

                    _ = scheduler.sleep_until_next_job() => {
                        if let Some(job) = scheduler.take_due_job().await {
                            info!(name = %job.name, "processing scheduled job");
                            scheduler.execute_job(job, &executor).await;
                        }
                    }
                }
            }

            info!("scheduler stopped");
        })
    };

    // Spawn follower sync task
    let follower_sync_handle = {
        let datalog_cache = datalog_cache.clone();
        let state_manager = Arc::clone(&state_manager);
        let mut shutdown_rx = shutdown_rx.clone();
        let follower_sync_interval = Duration::from_secs(config.follower_sync_interval);

        let sync_bluesky =
            BlueskyClient::new(&config.pds_url, &config.handle, &config.app_password)
                .await
                .map_err(|e| {
                    miette::miette!("failed to create follower sync Bluesky client: {}", e)
                })?;

        tokio::spawn(async move {
            info!("follower sync started");

            // Do initial sync immediately on startup
            if let Some(ref datalog_cache) = datalog_cache {
                match sync_followers(&sync_bluesky, &state_manager, datalog_cache).await {
                    Ok(count) => info!(count, "initial follower sync complete"),
                    Err(e) => warn!(error = %e, "initial follower sync failed"),
                }
            }

            let mut interval = tokio::time::interval(follower_sync_interval);

            loop {
                tokio::select! {
                    biased;

                    _ = shutdown_rx.changed() => {
                        if *shutdown_rx.borrow() {
                            break;
                        }
                    }

                    _ = interval.tick() => {
                        if let Some(ref datalog_cache) = datalog_cache {
                            match sync_followers(&sync_bluesky, &state_manager, datalog_cache).await {
                                Ok(count) => info!(count, "synced followers"),
                                Err(e) => warn!(error = %e, "follower sync failed"),
                            }
                        }
                    }
                }
            }

            info!("follower sync stopped");
        })
    };

    // Spawn trigger evaluation task
    let trigger_handle = {
        let cache = Arc::clone(&cache);
        let datalog_cache = datalog_cache.clone();
        let client = Arc::clone(&client);
        let mcp_base_url = Arc::clone(&mcp_base_url);
        let mut shutdown_rx = shutdown_rx.clone();

        let trigger_interval = Duration::from_secs(
            std::env::var("WINTER_TRIGGER_INTERVAL")
                .ok()
                .and_then(|s| s.parse().ok())
                .unwrap_or(300),
        );

        tokio::spawn(async move {
            info!(
                interval_secs = trigger_interval.as_secs(),
                "trigger evaluation task started"
            );

            // Wait for cache to be live before evaluating triggers
            loop {
                if *shutdown_rx.borrow() {
                    info!("trigger evaluation task shutting down before cache ready");
                    return;
                }
                if cache.state() == SyncState::Live {
                    break;
                }
                tokio::time::sleep(Duration::from_secs(2)).await;
            }

            let Some(datalog) = datalog_cache else {
                info!("trigger evaluation task disabled: no datalog cache");
                return;
            };

            let engine = crate::trigger_engine::TriggerEngine::new(
                cache,
                Arc::clone(&datalog),
                client,
                (*mcp_base_url).clone(),
            );

            let mut interval = tokio::time::interval(trigger_interval);

            loop {
                tokio::select! {
                    biased;

                    _ = shutdown_rx.changed() => {
                        if *shutdown_rx.borrow() {
                            break;
                        }
                    }

                    _ = interval.tick() => {
                        if let Err(e) = engine.evaluate_all().await {
                            warn!(error = %e, "trigger evaluation failed");
                        }
                    }
                }
            }

            info!("trigger evaluation task stopped");
        })
    };

    // Spawn persistent session loop (replaces idle watcher + worker pool)
    let session_handle = {
        let agent = Arc::clone(&agent);
        let identity_manager = Arc::clone(&identity_manager);
        let cache = Arc::clone(&cache);
        let client = Arc::clone(&client);
        let interruption_state = Arc::clone(&interruption_state);
        let mut shutdown_rx = shutdown_rx.clone();

        tokio::spawn(async move {
            info!("persistent session loop starting");

            loop {
                if *shutdown_rx.borrow() {
                    break;
                }

                // Load identity for context
                let identity = match identity_manager.load().await {
                    Ok(id) => id,
                    Err(e) => {
                        error!(error = ?e, "failed to load identity for persistent session");
                        tokio::time::sleep(Duration::from_secs(5)).await;
                        continue;
                    }
                };

                // Fetch context
                let (directives, rule_heads, recent_thoughts) = tokio::join!(
                    fetch_directives(&client, Some(&cache)),
                    fetch_rule_heads(&client, Some(&cache)),
                    fetch_recent_thoughts_scoped(&client, Some(&cache), 10, &ScopeFilter::Global)
                );

                // Clear interruption state before starting
                interruption_state.clear().await;

                let context = AgentContext::new(identity)
                    .with_directives(directives)
                    .with_rule_heads(rule_heads)
                    .with_thoughts(recent_thoughts)
                    .with_trigger(ContextTrigger::PersistentSession);

                info!("starting persistent session");

                // Run persistent session
                tokio::select! {
                    biased;

                    _ = shutdown_rx.changed() => {
                        if *shutdown_rx.borrow() {
                            info!("shutdown during persistent session");
                            break;
                        }
                    }

                    result = agent.persistent_session(context) => {
                        match result {
                            Ok(_) => info!("persistent session completed"),
                            Err(e) => warn!(error = %e, "persistent session failed"),
                        }
                    }
                }

                // Clear interruption state after session ends
                interruption_state.clear().await;

                // Cooldown before restarting
                info!("persistent session ended, restarting in 5s");
                tokio::select! {
                    biased;
                    _ = shutdown_rx.changed() => {
                        if *shutdown_rx.borrow() {
                            break;
                        }
                    }
                    _ = tokio::time::sleep(Duration::from_secs(5)) => {}
                }
            }

            info!("persistent session loop stopped");
        })
    };

    // Spawn watchdog: if inbox has pending items and no tool calls for >5 min, restart session
    let watchdog_handle = {
        let http_client = Arc::clone(&http_client);
        let mcp_base_url = Arc::clone(&mcp_base_url);
        let mut shutdown_rx = shutdown_rx.clone();

        tokio::spawn(async move {
            info!("watchdog started");
            let mut interval = tokio::time::interval(Duration::from_secs(30));

            loop {
                tokio::select! {
                    biased;

                    _ = shutdown_rx.changed() => {
                        if *shutdown_rx.borrow() {
                            break;
                        }
                    }

                    _ = interval.tick() => {
                        // Query inbox status + last tool call time via HTTP
                        let status = get_inbox_status(&http_client, &mcp_base_url).await;

                        if status.is_empty {
                            continue;
                        }

                        let stale_secs = status.stale_secs();

                        if stale_secs > 300 {
                            warn!(
                                stale_secs,
                                inbox_pending = status.pending,
                                "watchdog: session appears stuck, inbox items pending for >5min with no tool calls"
                            );
                            // The session loop will handle restart — the watchdog just logs for now.
                            // A harder kill would need to abort the session handle, which we could
                            // add later if needed.
                        }
                    }
                }
            }

            info!("watchdog stopped");
        })
    };

    // Wait for shutdown signal
    let mut main_shutdown_rx = shutdown_rx.clone();
    loop {
        if main_shutdown_rx.changed().await.is_err() || *main_shutdown_rx.borrow() {
            break;
        }
    }

    info!("shutting down daemon tasks");

    // Wait for all tasks to complete
    let _ = dm_handle.await;
    let _ = notif_handle.await;
    let _ = scheduler_handle.await;
    let _ = follower_sync_handle.await;
    let _ = trigger_handle.await;
    let _ = session_handle.await;
    let _ = watchdog_handle.await;

    // Wait for sync coordinator to finish
    if let Some(handle) = sync_handle {
        handle.await.ok();
    }

    info!("daemon shut down gracefully");
    Ok(())
}


/// Push an inbox item to the MCP server via HTTP POST.
async fn push_inbox_item(http_client: &reqwest::Client, mcp_base_url: &str, item: InboxItem) {
    let url = format!("{}/inbox", mcp_base_url);
    match http_client.post(&url).json(&item).send().await {
        Ok(resp) if resp.status().is_success() => {
            debug!(id = %item.id, kind = %item.kind, "inbox item pushed via HTTP");
        }
        Ok(resp) => {
            warn!(
                status = %resp.status(),
                id = %item.id,
                "failed to push inbox item via HTTP"
            );
        }
        Err(e) => {
            warn!(error = %e, id = %item.id, "failed to push inbox item via HTTP");
        }
    }
}

/// Inbox status response from the MCP server.
struct InboxStatus {
    pending: usize,
    is_empty: bool,
    last_tool_call_at: Option<chrono::DateTime<chrono::Utc>>,
}

impl InboxStatus {
    /// Seconds since the last tool call. Returns 0 if unknown.
    fn stale_secs(&self) -> u64 {
        self.last_tool_call_at
            .map(|t| {
                let elapsed = chrono::Utc::now().signed_duration_since(t);
                elapsed.num_seconds().max(0) as u64
            })
            .unwrap_or(0)
    }
}

/// Query inbox status from the MCP server via HTTP GET.
async fn get_inbox_status(http_client: &reqwest::Client, mcp_base_url: &str) -> InboxStatus {
    let url = format!("{}/inbox/status", mcp_base_url);
    match http_client.get(&url).send().await {
        Ok(resp) if resp.status().is_success() => {
            if let Ok(body) = resp.json::<serde_json::Value>().await {
                let pending = body.get("pending").and_then(|v| v.as_u64()).unwrap_or(0) as usize;
                let is_empty = body.get("is_empty").and_then(|v| v.as_bool()).unwrap_or(true);
                let last_tool_call_at = body
                    .get("last_tool_call_at")
                    .and_then(|v| v.as_str())
                    .and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok())
                    .map(|dt| dt.with_timezone(&chrono::Utc));
                InboxStatus { pending, is_empty, last_tool_call_at }
            } else {
                InboxStatus { pending: 0, is_empty: true, last_tool_call_at: None }
            }
        }
        _ => {
            // If we can't reach the MCP server, treat as empty (avoid false alarms)
            InboxStatus { pending: 0, is_empty: true, last_tool_call_at: None }
        }
    }
}

/// Fetch recent DM history from the conversation (last 15 minutes).
///
/// Returns messages sorted chronologically (oldest first), excluding the triggering message.
async fn fetch_dm_history(
    bluesky: &BlueskyClient,
    convo_id: &str,
    triggering_message_id: &str,
    trigger_time: chrono::DateTime<chrono::Utc>,
) -> Vec<ConversationHistoryMessage> {
    let cutoff = trigger_time - chrono::Duration::minutes(15);
    let own_did = bluesky.did().await.unwrap_or_default();

    match bluesky.get_messages(convo_id, None).await {
        Ok(mut messages) => {
            // Sort chronologically (oldest first)
            messages.sort_by_key(|m| m.sent_at);

            messages
                .into_iter()
                .filter(|m| m.sent_at >= cutoff && m.id != triggering_message_id)
                .map(|m| ConversationHistoryMessage {
                    sender_label: if m.sender_did == own_did {
                        "You".to_string()
                    } else {
                        "Operator".to_string()
                    },
                    text: m.text,
                    sent_at: m.sent_at,
                })
                .collect()
        }
        Err(e) => {
            warn!(error = %e, "failed to fetch DM history");
            Vec::new()
        }
    }
}

/// Sync followers from the Bluesky API to the state record and datalog cache.
async fn sync_followers(
    bluesky: &BlueskyClient,
    state_manager: &StateManager,
    datalog_cache: &DatalogCache,
) -> Result<usize, BlueskyError> {
    let followers = bluesky.get_all_followers().await?;
    let count = followers.len();

    // Persist to PDS state record (so MCP servers can get it from CAR file)
    if let Err(e) = state_manager.set_followers(followers.clone()).await {
        warn!(error = %e, "failed to persist followers to state record");
    }

    // Update datalog cache for immediate query availability
    let followers_set: HashSet<String> = followers.into_iter().collect();
    datalog_cache.set_followers(followers_set).await;

    // Flush the dirty predicate to write is_followed_by.facts
    if let Err(e) = datalog_cache.flush_dirty_predicates().await {
        warn!(error = %e, "failed to flush is_followed_by after follower sync");
    }
    Ok(count)
}
