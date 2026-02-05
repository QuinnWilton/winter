//! Daemon command for running Winter's main loop.
//!
//! The daemon uses a parallelized architecture:
//! - Dedicated operator DM poller (priority path, never blocked)
//! - Notification poller + bounded work queue
//! - Worker pool for parallel notification processing
//! - DatalogCoordinator for serialized TSV file access

use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use miette::Result;
use tokio::sync::{Mutex, mpsc, watch};
use tracing::{debug, error, info, warn};

use winter_agent::{
    Agent, AgentContext, ContextTrigger, ConversationHistoryMessage, IdentityManager, PostRef,
    StateManager,
};
use winter_atproto::{
    AtprotoClient, DIRECTIVE_COLLECTION, Directive, Identity, RULE_COLLECTION, RepoCache, Rule,
    ScopeFilter, SyncCoordinator, SyncState, THOUGHT_COLLECTION, Thought, ThoughtKind, Tid,
};
use winter_datalog::{DatalogCache, DatalogCoordinator};
use winter_mcp::InterruptionState;
use winter_mcp::bluesky::{BlueskyNotification, DirectMessage, NotificationReason};
use winter_mcp::{BlueskyClient, BlueskyError};
use winter_scheduler::Scheduler;

/// Default number of notification workers.
const DEFAULT_WORKER_COUNT: usize = 3;

/// Default work queue size.
const DEFAULT_QUEUE_SIZE: usize = 50;

/// Default DM poll interval in seconds.
const DEFAULT_DM_POLL_INTERVAL: u64 = 5;

/// Default notification poll interval in seconds.
const DEFAULT_NOTIF_POLL_INTERVAL: u64 = 10;

/// Default idle awaken timeout in seconds (60 minutes).
const DEFAULT_IDLE_AWAKEN_TIMEOUT: u64 = 3600;

/// Default background session grace period in seconds.
const DEFAULT_BACKGROUND_GRACE_SECS: u64 = 60;

/// Default background session idle threshold in seconds (5 minutes).
const DEFAULT_BACKGROUND_IDLE_SECS: u64 = 300;

/// Configuration for the daemon.
pub struct DaemonConfig {
    pub pds_url: String,
    pub handle: String,
    pub app_password: String,
    pub poll_interval: u64,
    pub awaken_interval: u64,
    pub mcp_config_path: PathBuf,
    /// Optional firehose URL (defaults to wss://bsky.network).
    pub firehose_url: Option<String>,
    /// Interval in seconds for syncing followers from the Bluesky API.
    pub follower_sync_interval: u64,
    /// If true, fast-forward notification and DM cursors to current time on startup.
    /// This skips all existing notifications/DMs and only processes new ones.
    pub fast_forward: bool,
    /// Number of notification workers (default 3).
    pub worker_count: Option<usize>,
    /// Work queue size (default 50).
    pub queue_size: Option<usize>,
    /// DM poll interval in seconds (default 5).
    pub dm_poll_interval: Option<u64>,
    /// Notification poll interval in seconds (default 10).
    pub notif_poll_interval: Option<u64>,
    /// Idle timeout in seconds before triggering awaken (0 to disable).
    pub idle_awaken_timeout: Option<u64>,
    /// Enable background sessions (default: true).
    pub background_sessions_enabled: Option<bool>,
    /// Grace period for background session interruption in seconds (default: 60).
    pub background_grace_secs: Option<u64>,
    /// How long queue must be empty before starting background session in seconds (default: 300).
    pub background_idle_secs: Option<u64>,
}

/// Work item for notification processing.
struct NotificationWork {
    notification: BlueskyNotification,
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

/// Run the daemon.
pub async fn run(
    pds_url: &str,
    handle: &str,
    app_password: &str,
    poll_interval: u64,
    awaken_interval: u64,
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
        awaken_interval,
        mcp_config_path,
        firehose_url: None,
        follower_sync_interval,
        fast_forward,
        worker_count: None,
        queue_size: None,
        dm_poll_interval: None,
        notif_poll_interval: None,
        idle_awaken_timeout: None,
        background_sessions_enabled: None,
        background_grace_secs: None,
        background_idle_secs: None,
    })
    .await
}

/// Run the daemon with full configuration.
pub async fn run_with_config(config: DaemonConfig) -> Result<()> {
    info!("starting Winter daemon");

    // Read configuration from env vars with defaults
    let worker_count = config.worker_count.unwrap_or_else(|| {
        std::env::var("WINTER_WORKER_COUNT")
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or(DEFAULT_WORKER_COUNT)
    });
    let queue_size = config.queue_size.unwrap_or_else(|| {
        std::env::var("WINTER_QUEUE_SIZE")
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or(DEFAULT_QUEUE_SIZE)
    });
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
    let idle_awaken_timeout = config.idle_awaken_timeout.unwrap_or_else(|| {
        std::env::var("WINTER_IDLE_AWAKEN_TIMEOUT")
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or(DEFAULT_IDLE_AWAKEN_TIMEOUT)
    });
    let background_sessions_enabled = config.background_sessions_enabled.unwrap_or_else(|| {
        std::env::var("WINTER_BACKGROUND_ENABLED")
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or(true)
    });
    let background_grace_secs = config.background_grace_secs.unwrap_or_else(|| {
        std::env::var("WINTER_BACKGROUND_GRACE_SECS")
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or(DEFAULT_BACKGROUND_GRACE_SECS)
    });
    let background_idle_secs = config.background_idle_secs.unwrap_or_else(|| {
        std::env::var("WINTER_BACKGROUND_IDLE_SECS")
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or(DEFAULT_BACKGROUND_IDLE_SECS)
    });

    info!(
        worker_count,
        queue_size,
        dm_poll_interval_secs = dm_poll_interval.as_secs(),
        notif_poll_interval_secs = notif_poll_interval.as_secs(),
        idle_awaken_timeout_secs = idle_awaken_timeout,
        background_sessions_enabled,
        background_grace_secs,
        background_idle_secs,
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
    // Note: SyncCoordinator needs its own client for firehose operations
    let cache = RepoCache::new();

    let sync_client = AtprotoClient::new(&config.pds_url);
    sync_client
        .login(&config.handle, &config.app_password)
        .await
        .map_err(|e| miette::miette!("{}", e))?;

    let mut sync_coordinator = SyncCoordinator::new(sync_client, &did, Arc::clone(&cache));
    if let Some(ref firehose_url) = config.firehose_url {
        sync_coordinator = sync_coordinator.with_firehose_url(firehose_url);
    }

    // Create shutdown channel
    let (shutdown_tx, shutdown_rx) = watch::channel(false);

    // Create activity channel for idle awaken (signals when wakeup-triggering activity occurs)
    let (activity_tx, activity_rx) = watch::channel(std::time::Instant::now());

    // Handle shutdown signals
    let shutdown_tx_clone = shutdown_tx.clone();
    tokio::spawn(async move {
        tokio::signal::ctrl_c().await.ok();
        info!("received shutdown signal");
        let _ = shutdown_tx_clone.send(true);
    });

    // Start sync coordinator (firehose + CAR hydration)
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

    // Create DatalogCoordinator if we have a cache
    let datalog_coordinator = datalog_cache.as_ref().map(|dc| {
        let handle = DatalogCoordinator::spawn(Arc::clone(dc));
        info!("datalog coordinator started");
        handle
    });

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

    // Clean up duplicate awaken jobs (from non-idempotent bootstrap)
    match scheduler.deduplicate_jobs_by_name("awaken").await {
        Ok(0) => {}
        Ok(count) => info!(count, "cleaned up duplicate awaken jobs"),
        Err(e) => warn!(error = %e, "failed to deduplicate awaken jobs"),
    }

    // Reset awaken job if it exists and is in a failed/interrupted state
    if let Some(awaken_job) = scheduler.get_job_by_name("awaken").await {
        match scheduler.reset_job(&awaken_job.rkey).await {
            Ok(true) => info!("reset awaken job to pending"),
            Ok(false) => {} // Job was not in a resettable state
            Err(e) => warn!(error = %e, "failed to reset awaken job"),
        }
    } else {
        // Create awaken job if it doesn't exist
        if let Err(e) = scheduler
            .schedule_recurring(
                "awaken".to_string(),
                "Autonomous awaken cycle - reflect, browse timeline, engage".to_string(),
                config.awaken_interval,
            )
            .await
        {
            error!(error = %e, "failed to create awaken job");
        }
    }

    // Create agent for Claude invocation
    let agent = Arc::new(Agent::new(&config.mcp_config_path));

    // Create executor for scheduled jobs
    let executor: winter_scheduler::JobExecutor = {
        let agent = Arc::clone(&agent);
        let identity_manager = Arc::clone(&identity_manager);
        let client = Arc::clone(&client);
        let cache = Arc::clone(&cache);

        Box::new(move |job| {
            let agent = Arc::clone(&agent);
            let identity_manager = Arc::clone(&identity_manager);
            let client = Arc::clone(&client);
            let cache = Arc::clone(&cache);

            Box::pin(async move {
                let start = std::time::Instant::now();
                info!(name = %job.name, "executing scheduled job");

                // Build trigger string for thought recording
                // Awaken uses None (global), other jobs use job:{name}
                let trigger_str = if job.name == "awaken" {
                    None
                } else {
                    Some(format!("job:{}", job.name))
                };

                // Load identity for context
                let identity = match identity_manager.load().await {
                    Ok(id) => id,
                    Err(e) => {
                        error!(error = ?e, "failed to load identity for job");
                        return Err(format!("failed to load identity: {}", e));
                    }
                };

                // Build scope filter for thought fetching
                let scope = if job.name == "awaken" {
                    ScopeFilter::Global
                } else {
                    ScopeFilter::Job {
                        name: job.name.clone(),
                    }
                };

                // Fetch directives, rule heads, and recent thoughts for context (in parallel)
                // Use scoped thought fetching to only include relevant thoughts
                let (directives, rule_heads, recent_thoughts) = tokio::join!(
                    fetch_directives(&client, Some(&cache)),
                    fetch_rule_heads(&client, Some(&cache)),
                    fetch_recent_thoughts_scoped(&client, Some(&cache), 10, &scope)
                );

                // Build context
                let trigger = if job.name == "awaken" {
                    ContextTrigger::Awaken
                } else {
                    ContextTrigger::Job {
                        id: job.name.clone(),
                        name: job.name.clone(),
                    }
                };

                let context = AgentContext::new(identity)
                    .with_directives(directives)
                    .with_rule_heads(rule_heads)
                    .with_thoughts(recent_thoughts)
                    .with_trigger(trigger);

                // Execute via agent
                let result = if job.name == "awaken" {
                    agent.awaken(context).await
                } else {
                    agent.execute_job(context, &job.instructions).await
                };

                match result {
                    Ok(response) => {
                        let duration_ms = start.elapsed().as_millis() as u64;

                        // Record completion thought for non-awaken jobs
                        // (awaken thoughts are recorded by the agent itself)
                        if job.name != "awaken" {
                            let completion = Thought {
                                kind: ThoughtKind::Response,
                                content: truncate_chars(&response, 500),
                                trigger: trigger_str.clone(),
                                tags: vec![],
                                duration_ms: Some(duration_ms),
                                created_at: chrono::Utc::now(),
                            };

                            let rkey = Tid::now().to_string();
                            if let Err(e) = client
                                .create_record(THOUGHT_COLLECTION, Some(&rkey), &completion)
                                .await
                            {
                                warn!(error = %e, "failed to record job completion thought");
                            }
                        }

                        debug!(response_len = response.len(), job = %job.name, duration_ms, "job completed");
                        Ok(())
                    }
                    Err(e) => {
                        let duration_ms = start.elapsed().as_millis() as u64;

                        // Record error thought
                        let error_thought = Thought {
                            kind: ThoughtKind::Error,
                            content: format!("Job '{}' failed: {}", job.name, e),
                            trigger: trigger_str.clone(),
                            tags: vec![],
                            duration_ms: Some(duration_ms),
                            created_at: chrono::Utc::now(),
                        };

                        let rkey = Tid::now().to_string();
                        if let Err(e2) = client
                            .create_record(THOUGHT_COLLECTION, Some(&rkey), &error_thought)
                            .await
                        {
                            warn!(error = %e2, "failed to record job error thought");
                        }

                        error!(error = ?e, job = %job.name, "job failed");
                        Err(format!("agent error: {}", e))
                    }
                }
            })
        })
    };

    // Create work queue for notifications
    let (work_tx, work_rx) = mpsc::channel::<NotificationWork>(queue_size);
    let work_rx = Arc::new(Mutex::new(work_rx));

    // Create shared interruption state for background sessions
    let interruption_state = Arc::new(InterruptionState::new());

    // Spawn worker pool
    let mut worker_handles = Vec::with_capacity(worker_count);
    for worker_id in 0..worker_count {
        let work_rx = Arc::clone(&work_rx);
        let agent = Arc::clone(&agent);
        let identity_manager = Arc::clone(&identity_manager);
        let client = Arc::clone(&client);
        let cache = Arc::clone(&cache);
        let mut shutdown_rx = shutdown_rx.clone();

        let handle = tokio::spawn(async move {
            info!(worker_id, "notification worker started");

            loop {
                // Check for shutdown
                if *shutdown_rx.borrow() {
                    break;
                }

                // Try to get work item
                let work = {
                    let mut rx = work_rx.lock().await;
                    tokio::select! {
                        biased;
                        _ = shutdown_rx.changed() => {
                            if *shutdown_rx.borrow() {
                                break;
                            }
                            continue;
                        }
                        work = rx.recv() => work,
                    }
                };

                let Some(NotificationWork { notification }) = work else {
                    // Channel closed
                    break;
                };

                info!(
                    worker_id,
                    reason = ?notification.reason,
                    author = %notification.author_handle,
                    "worker processing notification"
                );

                // Load identity for context
                let identity = match identity_manager.load().await {
                    Ok(id) => id,
                    Err(e) => {
                        error!(error = ?e, worker_id, "failed to load identity for notification");
                        continue;
                    }
                };

                // Handle the notification
                handle_notification(&notification, &client, &agent, identity, Some(&cache)).await;
            }

            info!(worker_id, "notification worker stopped");
        });

        worker_handles.push(handle);
    }

    // Spawn dedicated operator DM poller
    let dm_handle = {
        let operator_did = operator_did.clone();
        let agent = Arc::clone(&agent);
        let identity_manager = Arc::clone(&identity_manager);
        let dm_client = Arc::clone(&dm_client);
        let state_manager = Arc::clone(&state_manager);
        let cache = Arc::clone(&cache);
        let mut shutdown_rx = shutdown_rx.clone();
        let activity_tx = activity_tx.clone();

        tokio::spawn(async move {
            info!("operator DM poller started");
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
                        // Poll for DMs
                        match dm_bluesky.get_unread_dms().await {
                            Ok(dms) => {
                                // Filter to only operator DMs
                                let operator_dms: Vec<_> = dms.into_iter()
                                    .filter(|dm| dm.sender_did == operator_did)
                                    .collect();

                                // Log and skip non-operator DMs
                                // (they were already filtered out, but log for visibility)
                                if operator_dms.is_empty() {
                                    continue;
                                }

                                // Process operator DMs immediately (priority path)
                                for dm in operator_dms {
                                    info!(
                                        sender = %dm.sender_did,
                                        convo_id = %dm.convo_id,
                                        text = %dm.text,
                                        "processing operator DM (priority)"
                                    );

                                    // Persist DM cursor BEFORE processing
                                    if let Some(cursor) = dm_bluesky.last_dm_cursor() {
                                        debug!(cursor = %cursor, "persisting DM cursor");
                                        if let Err(e) = state_manager.set_dm_cursor(Some(cursor.to_string())).await {
                                            warn!(error = %e, "failed to persist DM cursor");
                                        }
                                    }

                                    // Load identity for context
                                    let identity = match identity_manager.load().await {
                                        Ok(id) => id,
                                        Err(e) => {
                                            error!(error = ?e, "failed to load identity for operator DM");
                                            continue;
                                        }
                                    };

                                    // Handle the DM inline (not queued) using dedicated DM client
                                    handle_dm(&dm, &dm_client, &dm_bluesky, &agent, identity, Some(&cache)).await;

                                    // Signal activity after processing operator DM
                                    let _ = activity_tx.send(std::time::Instant::now());
                                }
                            }
                            Err(e) => {
                                warn!(error = %e, "DM poll failed");
                            }
                        }
                    }
                }
            }

            info!("operator DM poller stopped");
        })
    };

    // Spawn notification poller (takes ownership of work_tx)
    let notif_handle = {
        let state_manager = Arc::clone(&state_manager);
        let datalog_cache = datalog_cache.clone();
        let mut shutdown_rx = shutdown_rx.clone();
        let work_tx = work_tx; // Move work_tx into this closure
        let activity_tx = activity_tx.clone();
        let interruption_state = Arc::clone(&interruption_state);

        tokio::spawn(async move {
            info!("notification poller started");
            let mut interval = tokio::time::interval(notif_poll_interval);
            let mut rate_limit_backoff = Duration::from_secs(0);

            loop {
                // If we're in backoff mode, sleep before polling
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
                                // Reset backoff on success
                                rate_limit_backoff = Duration::ZERO;

                                // Track whether all notifications were successfully enqueued
                                // Cursor is only persisted AFTER successful enqueuing to prevent
                                // notification loss on queue full/timeout conditions
                                let mut all_enqueued = true;
                                let mut channel_closed = false;

                                // Process notifications
                                for notif in notifications {
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
                                            // Flush immediately so queries see the update
                                            if let Err(e) = dc.flush_dirty_predicates().await {
                                                warn!(error = %e, "failed to flush follower update");
                                            }
                                        }
                                        continue; // Don't queue Follow notifications for agent processing
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

                                    // Signal activity on receipt (not enqueue) - queue pressure doesn't mean no activity
                                    let _ = activity_tx.send(std::time::Instant::now());

                                    // Signal interruption if background session is running
                                    // This tells the background session to wrap up
                                    // For HTTP mode, also signal the MCP server
                                    interruption_state.set_interrupt("queue_pressure").await;
                                    if let Ok(mcp_url) = std::env::var("WINTER_MCP_URL") {
                                        // Fire-and-forget HTTP call to MCP server
                                        let url = format!("{}/interrupt", mcp_url.trim_end_matches('/'));
                                        tokio::spawn(async move {
                                            let client = reqwest::Client::new();
                                            let _ = client.post(&url)
                                                .json(&serde_json::json!({"reason": "queue_pressure"}))
                                                .send()
                                                .await;
                                        });
                                    }

                                    let work = NotificationWork { notification: notif };

                                    // Blocking send with timeout - applies backpressure instead of dropping
                                    // If workers are overloaded, this will wait up to 5s before giving up
                                    match tokio::time::timeout(
                                        Duration::from_secs(5),
                                        work_tx.send(work)
                                    ).await {
                                        Ok(Ok(())) => {}
                                        Ok(Err(_)) => {
                                            // Channel closed, stop polling
                                            channel_closed = true;
                                            break;
                                        }
                                        Err(_) => {
                                            // Timeout - queue is full and workers are overloaded
                                            // Don't persist cursor so these notifications will be re-fetched
                                            warn!("notification send timed out, will retry on next poll");
                                            all_enqueued = false;
                                            break;
                                        }
                                    }
                                }

                                if channel_closed {
                                    break;
                                }

                                // Only persist cursor if all notifications were successfully enqueued
                                // This ensures notifications are never lost - if we couldn't enqueue them,
                                // they'll be re-fetched on the next poll
                                if all_enqueued
                                    && let Some(cursor) = notif_bluesky.last_seen_at()
                                {
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
                                // Exponential backoff: 5s, 10s, 20s, 40s, up to 300s max
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

        // Create a separate Bluesky client for follower sync
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

    // Spawn idle watcher task (triggers awaken or background session when no activity)
    let idle_watcher_handle = if idle_awaken_timeout > 0 {
        let agent = Arc::clone(&agent);
        let identity_manager = Arc::clone(&identity_manager);
        let cache = Arc::clone(&cache);
        let client = Arc::clone(&client);
        let activity_tx = activity_tx.clone();
        let mut activity_rx = activity_rx.clone();
        let mut shutdown_rx = shutdown_rx.clone();
        let idle_timeout = Duration::from_secs(idle_awaken_timeout);
        let interruption_state = Arc::clone(&interruption_state);
        let background_idle = Duration::from_secs(background_idle_secs);
        // Note: grace period would be used for force-cancellation, but current implementation
        // relies on the agent calling check_interruption and exiting gracefully.
        // Force-cancel would require aborting the future, which we don't do here.
        let _grace_period = Duration::from_secs(background_grace_secs);

        Some(tokio::spawn(async move {
            info!(
                idle_timeout_secs = idle_awaken_timeout,
                background_enabled = background_sessions_enabled,
                background_idle_secs,
                "idle watcher started"
            );
            let mut last_activity = *activity_rx.borrow();

            loop {
                let elapsed = last_activity.elapsed();

                // Use shorter timeout for background sessions when enabled
                let target_timeout = if background_sessions_enabled {
                    background_idle
                } else {
                    idle_timeout
                };
                let remaining = target_timeout.saturating_sub(elapsed);

                tokio::select! {
                    biased;

                    _ = shutdown_rx.changed() => {
                        if *shutdown_rx.borrow() {
                            break;
                        }
                    }

                    _ = activity_rx.changed() => {
                        last_activity = *activity_rx.borrow();
                        debug!("activity detected, resetting idle timer");
                    }

                    _ = tokio::time::sleep(remaining) => {
                        // Double-check we actually reached the timeout
                        if last_activity.elapsed() >= target_timeout {
                            // Load identity for context
                            let identity = match identity_manager.load().await {
                                Ok(id) => id,
                                Err(e) => {
                                    warn!(error = ?e, "failed to load identity for idle session");
                                    last_activity = std::time::Instant::now();
                                    let _ = activity_tx.send(last_activity);
                                    continue;
                                }
                            };

                            // Fetch context
                            let (directives, rule_heads, recent_thoughts) = tokio::join!(
                                fetch_directives(&client, Some(&cache)),
                                fetch_rule_heads(&client, Some(&cache)),
                                fetch_recent_thoughts_scoped(&client, Some(&cache), 10, &ScopeFilter::Global)
                            );

                            if background_sessions_enabled {
                                // Start background session
                                info!("idle timeout reached, starting background session");

                                // Clear any previous interruption
                                interruption_state.clear().await;

                                let context = AgentContext::new(identity)
                                    .with_directives(directives)
                                    .with_rule_heads(rule_heads)
                                    .with_thoughts(recent_thoughts)
                                    .with_trigger(ContextTrigger::Background);

                                // Run background session with interruptibility
                                // When activity occurs, the notification poller sets interruption state
                                // The agent should call check_interruption and exit gracefully
                                // If not, we force-cancel after the session's internal timeout
                                let session_future = agent.background_session(context);

                                // Monitor for activity while session runs
                                tokio::select! {
                                    biased;

                                    _ = shutdown_rx.changed() => {
                                        if *shutdown_rx.borrow() {
                                            info!("shutdown during background session");
                                            break;
                                        }
                                    }

                                    result = session_future => {
                                        match result {
                                            Ok(_response) => {
                                                info!("background session completed");
                                            }
                                            Err(e) => {
                                                warn!(error = %e, "background session failed");
                                            }
                                        }
                                    }
                                }

                                // Clear interruption state after session ends
                                interruption_state.clear().await;
                            } else {
                                // Fall back to awaken cycle
                                info!("idle timeout reached, triggering awaken");

                                let context = AgentContext::new(identity)
                                    .with_directives(directives)
                                    .with_rule_heads(rule_heads)
                                    .with_thoughts(recent_thoughts)
                                    .with_trigger(ContextTrigger::Awaken);

                                match tokio::time::timeout(
                                    Duration::from_secs(1800),
                                    agent.awaken(context)
                                ).await {
                                    Ok(Ok(_response)) => {
                                        info!("idle awaken completed");
                                    }
                                    Ok(Err(e)) => {
                                        warn!(error = %e, "idle awaken failed");
                                    }
                                    Err(_) => {
                                        warn!("idle awaken timed out");
                                    }
                                }
                            }

                            // Reset idle timer after session (success or failure)
                            last_activity = std::time::Instant::now();
                            let _ = activity_tx.send(last_activity);
                        }
                    }
                }
            }
            info!("idle watcher stopped");
        }))
    } else {
        info!("idle watcher disabled (timeout = 0)");
        None
    };

    // Wait for shutdown signal
    let mut main_shutdown_rx = shutdown_rx.clone();
    loop {
        if main_shutdown_rx.changed().await.is_err() || *main_shutdown_rx.borrow() {
            break;
        }
    }

    info!("shutting down daemon tasks");

    // Shutdown datalog coordinator if present
    if let Some(ref coordinator) = datalog_coordinator {
        coordinator.shutdown().await;
    }

    // Note: work_tx is dropped when notif_handle completes, which signals workers to stop

    // Wait for all tasks to complete
    let _ = dm_handle.await;
    let _ = notif_handle.await;
    let _ = scheduler_handle.await;
    let _ = follower_sync_handle.await;
    if let Some(handle) = idle_watcher_handle {
        let _ = handle.await;
    }

    for handle in worker_handles {
        let _ = handle.await;
    }

    // Wait for sync coordinator to finish
    if let Some(handle) = sync_handle {
        handle.await.ok();
    }

    info!("daemon shut down gracefully");
    Ok(())
}

/// Handle a single notification.
async fn handle_notification(
    notif: &BlueskyNotification,
    atproto: &AtprotoClient,
    agent: &Agent,
    identity: Identity,
    cache: Option<&RepoCache>,
) {
    let start = std::time::Instant::now();

    // Record that we received this notification as a thought
    let reason_str = match notif.reason {
        NotificationReason::Mention => "mention",
        NotificationReason::Reply => "reply",
        NotificationReason::Quote => "quote",
        _ => "notification",
    };

    let content = format!(
        "Received {} from @{}: {}",
        reason_str,
        notif.author_handle,
        notif.text.as_deref().unwrap_or("[no text]")
    );

    // Build trigger string with root for thread continuity
    // Format: notification:{uri}:root={root_uri}
    let root_uri = notif
        .root
        .as_ref()
        .map(|r| r.uri.as_str())
        .unwrap_or(&notif.uri);
    let trigger_str = format!("notification:{}:root={}", notif.uri, root_uri);

    // Record insight thought
    let observation = Thought {
        kind: ThoughtKind::Insight,
        content: content.clone(),
        trigger: Some(trigger_str.clone()),
        tags: vec![],
        duration_ms: None,
        created_at: chrono::Utc::now(),
    };

    let rkey = Tid::now().to_string();
    if let Err(e) = atproto
        .create_record(THOUGHT_COLLECTION, Some(&rkey), &observation)
        .await
    {
        warn!(error = %e, "failed to record observation thought");
    }

    // Build context for Claude with threading information
    let trigger = ContextTrigger::Notification {
        kind: reason_str.to_string(),
        author_did: notif.author_did.clone(),
        author_handle: notif.author_handle.clone(),
        text: notif.text.clone(),
        uri: notif.uri.clone(),
        cid: notif.cid.clone(),
        parent: notif.parent.as_ref().map(|p| PostRef {
            uri: p.uri.clone(),
            cid: p.cid.clone(),
        }),
        root: notif.root.as_ref().map(|r| PostRef {
            uri: r.uri.clone(),
            cid: r.cid.clone(),
        }),
        facets: notif.facets.clone(),
    };

    // Build scope filter for thought fetching (same thread = same root URI)
    let scope = ScopeFilter::Thread {
        root_uri: root_uri.to_string(),
    };

    // Fetch directives, rule heads, and recent thoughts for context (in parallel)
    // Use scoped thought fetching to only include thoughts from this conversation
    let (directives, rule_heads, recent_thoughts) = tokio::join!(
        fetch_directives(atproto, cache),
        fetch_rule_heads(atproto, cache),
        fetch_recent_thoughts_scoped(atproto, cache, 10, &scope)
    );

    let context = AgentContext::new(identity)
        .with_directives(directives)
        .with_rule_heads(rule_heads)
        .with_thoughts(recent_thoughts)
        .with_trigger(trigger);

    // Build user message from notification
    let user_message = notif
        .text
        .as_deref()
        .unwrap_or("(notification with no text)");

    // Invoke Claude via agent
    match agent.handle_notification(context, user_message).await {
        Ok(response) => {
            let duration_ms = start.elapsed().as_millis() as u64;

            // Record completion thought with same trigger format for thread continuity
            let completion = Thought {
                kind: ThoughtKind::Response,
                content: truncate_chars(&response, 500),
                trigger: Some(trigger_str.clone()),
                tags: vec![],
                duration_ms: Some(duration_ms),
                created_at: chrono::Utc::now(),
            };

            let rkey = Tid::now().to_string();
            if let Err(e) = atproto
                .create_record(THOUGHT_COLLECTION, Some(&rkey), &completion)
                .await
            {
                warn!(error = %e, "failed to record completion thought");
            }

            info!(
                uri = %notif.uri,
                duration_ms,
                response_len = response.len(),
                "notification handled"
            );
        }
        Err(e) => {
            // Record error thought with same trigger format for thread continuity
            let error_thought = Thought {
                kind: ThoughtKind::Error,
                content: format!("Failed to handle notification: {}", e),
                trigger: Some(trigger_str.clone()),
                tags: vec![],
                duration_ms: Some(start.elapsed().as_millis() as u64),
                created_at: chrono::Utc::now(),
            };

            let rkey = Tid::now().to_string();
            if let Err(e2) = atproto
                .create_record(THOUGHT_COLLECTION, Some(&rkey), &error_thought)
                .await
            {
                warn!(error = %e2, "failed to record error thought");
            }

            error!(error = ?e, uri = %notif.uri, "failed to handle notification");
        }
    }
}

/// Handle a single direct message.
async fn handle_dm(
    dm: &DirectMessage,
    atproto: &AtprotoClient,
    bluesky: &BlueskyClient,
    agent: &Agent,
    identity: Identity,
    cache: Option<&RepoCache>,
) {
    let start = std::time::Instant::now();

    // Resolve sender handle from DID
    let sender_handle = format!("did:{}", &dm.sender_did[4..]); // Fallback to DID if resolution fails

    let content = format!("Received DM from {}: {}", sender_handle, dm.text);

    // Build trigger string for DM conversation
    // Format: dm:{convo_id}:{message_id}
    let trigger_str = format!("dm:{}:{}", dm.convo_id, dm.id);

    // Record insight thought
    let observation = Thought {
        kind: ThoughtKind::Insight,
        content: content.clone(),
        trigger: Some(trigger_str.clone()),
        tags: vec![],
        duration_ms: None,
        created_at: chrono::Utc::now(),
    };

    let rkey = Tid::now().to_string();
    if let Err(e) = atproto
        .create_record(THOUGHT_COLLECTION, Some(&rkey), &observation)
        .await
    {
        warn!(error = %e, "failed to record DM observation thought");
    }

    // Fetch conversation history (last 15 minutes, excluding triggering message)
    let history = fetch_dm_history(bluesky, &dm.convo_id, &dm.id, dm.sent_at).await;

    // Build context for Claude
    let trigger = ContextTrigger::DirectMessage {
        convo_id: dm.convo_id.clone(),
        message_id: dm.id.clone(),
        sender_did: dm.sender_did.clone(),
        sender_handle: sender_handle.clone(),
        text: dm.text.clone(),
        facets: dm.facets.clone(),
        history,
    };

    // Build scope filter for thought fetching (same DM conversation)
    let scope = ScopeFilter::DirectMessage {
        convo_id: dm.convo_id.clone(),
    };

    // Fetch directives, rule heads, and recent thoughts for context (in parallel)
    // Use scoped thought fetching to only include thoughts from this conversation
    debug!(convo_id = %dm.convo_id, "fetching DM context");
    let (directives, rule_heads, recent_thoughts) = tokio::join!(
        fetch_directives(atproto, cache),
        fetch_rule_heads(atproto, cache),
        fetch_recent_thoughts_scoped(atproto, cache, 10, &scope)
    );
    debug!(
        convo_id = %dm.convo_id,
        directives = directives.len(),
        rules = rule_heads.len(),
        thoughts = recent_thoughts.len(),
        "DM context fetched"
    );

    let context = AgentContext::new(identity)
        .with_directives(directives)
        .with_rule_heads(rule_heads)
        .with_thoughts(recent_thoughts)
        .with_trigger(trigger);

    debug!(
        convo_id = %dm.convo_id,
        "invoking agent for DM"
    );

    // Invoke Claude via agent - Claude should use reply_to_dm tool to send the response
    match agent.handle_dm(context, &dm.text).await {
        Ok(response) => {
            let duration_ms = start.elapsed().as_millis() as u64;

            // Record completion thought (the actual reply is sent by Claude via reply_to_dm tool)
            let completion = Thought {
                kind: ThoughtKind::Response,
                content: truncate_chars(&response, 500),
                trigger: Some(trigger_str.clone()),
                tags: vec![],
                duration_ms: Some(duration_ms),
                created_at: chrono::Utc::now(),
            };

            let rkey = Tid::now().to_string();
            if let Err(e) = atproto
                .create_record(THOUGHT_COLLECTION, Some(&rkey), &completion)
                .await
            {
                warn!(error = %e, "failed to record DM completion thought");
            }

            info!(
                convo_id = %dm.convo_id,
                message_id = %dm.id,
                duration_ms,
                response_len = response.len(),
                "DM handled"
            );
        }
        Err(e) => {
            // Record error thought
            let error_thought = Thought {
                kind: ThoughtKind::Error,
                content: format!("Failed to handle DM: {}", e),
                trigger: Some(trigger_str.clone()),
                tags: vec![],
                duration_ms: Some(start.elapsed().as_millis() as u64),
                created_at: chrono::Utc::now(),
            };

            let rkey = Tid::now().to_string();
            if let Err(e2) = atproto
                .create_record(THOUGHT_COLLECTION, Some(&rkey), &error_thought)
                .await
            {
                warn!(error = %e2, "failed to record DM error thought");
            }

            error!(error = ?e, convo_id = %dm.convo_id, message_id = %dm.id, "failed to handle DM");
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
