//! Daemon command for running Winter's main loop.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use miette::Result;
use tokio::sync::watch;
use tracing::{debug, error, info, warn};

use winter_agent::{Agent, AgentContext, ContextTrigger, IdentityManager, PostRef, StateManager};
use winter_atproto::{
    AtprotoClient, Identity, RULE_COLLECTION, RepoCache, Rule, SyncCoordinator, SyncState,
    THOUGHT_COLLECTION, Thought, ThoughtKind, Tid,
};
use winter_mcp::bluesky::{BlueskyNotification, DirectMessage, NotificationReason};
use winter_mcp::{BlueskyClient, BlueskyError};
use winter_scheduler::{Job, Scheduler};

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
}

/// Event types for the unified event loop.
enum Event {
    Notification(BlueskyNotification),
    DirectMessage(DirectMessage),
    Job(Job),
}

/// Fetch deduplicated rule heads from the PDS or cache.
/// Returns heads like "mutual_follow(X, Y)" for use in queries.
async fn fetch_rule_heads(client: &AtprotoClient, cache: Option<&RepoCache>) -> Vec<String> {
    // Try cache first
    if let Some(cache) = cache {
        if cache.state() == SyncState::Live {
            let mut heads: Vec<String> = cache
                .list_rules()
                .into_iter()
                .filter(|(_, r)| r.value.enabled)
                .map(|(_, r)| r.value.head)
                .collect();
            heads.sort();
            heads.dedup();
            return heads;
        }
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

/// Fetch recent thoughts from the PDS or cache.
/// Returns thoughts in reverse chronological order (most recent first).
async fn fetch_recent_thoughts(
    client: &AtprotoClient,
    cache: Option<&RepoCache>,
    limit: usize,
) -> Vec<Thought> {
    // Try cache first
    if let Some(cache) = cache {
        if cache.state() == SyncState::Live {
            return cache.recent_thoughts(limit);
        }
    }

    // Fall back to HTTP
    match client
        .list_records::<Thought>(THOUGHT_COLLECTION, Some(limit as u32), None)
        .await
    {
        Ok(response) => {
            // Records are returned oldest-first by TID, so reverse for recent-first
            let mut thoughts: Vec<Thought> =
                response.records.into_iter().map(|r| r.value).collect();
            thoughts.reverse();
            thoughts
        }
        Err(e) => {
            warn!(error = %e, "failed to fetch recent thoughts for context");
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
) -> Result<()> {
    // Use default MCP config path
    let mcp_config_path = dirs::home_dir()
        .map(|h| h.join(".config/winter/mcp.json"))
        .unwrap_or_else(|| PathBuf::from("/etc/winter/mcp.json"));

    run_with_config(DaemonConfig {
        pds_url: pds_url.to_string(),
        handle: handle.to_string(),
        app_password: app_password.to_string(),
        poll_interval,
        awaken_interval,
        mcp_config_path,
        firehose_url: None,
    })
    .await
}

/// Run the daemon with full configuration.
pub async fn run_with_config(config: DaemonConfig) -> Result<()> {
    info!("starting Winter daemon");

    // Create main ATProto client
    let client = AtprotoClient::new(&config.pds_url);
    client
        .login(&config.handle, &config.app_password)
        .await
        .map_err(|e| miette::miette!("{}", e))?;

    // Create identity manager
    let identity_client = AtprotoClient::new(&config.pds_url);
    identity_client
        .login(&config.handle, &config.app_password)
        .await
        .map_err(|e| miette::miette!("{}", e))?;
    let identity_manager = Arc::new(IdentityManager::new(identity_client));

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

    info!(
        values = ?identity.values,
        interests = ?identity.interests,
        "identity loaded"
    );

    // Create state manager and load cursors
    let state_client = AtprotoClient::new(&config.pds_url);
    state_client
        .login(&config.handle, &config.app_password)
        .await
        .map_err(|e| miette::miette!("{}", e))?;
    let state_manager = Arc::new(StateManager::new(state_client));

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

    let mut sync_coordinator = SyncCoordinator::new(sync_client, &did, Arc::clone(&cache));
    if let Some(ref firehose_url) = config.firehose_url {
        sync_coordinator = sync_coordinator.with_firehose_url(firehose_url);
    }

    // Create shutdown channel
    let (shutdown_tx, shutdown_rx) = watch::channel(false);

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

    // Create Bluesky client for polling
    let mut bluesky_client =
        BlueskyClient::new(&config.pds_url, &config.handle, &config.app_password)
            .await
            .map_err(|e| miette::miette!("failed to create Bluesky client: {}", e))?;

    // Initialize with persisted cursors
    bluesky_client.set_last_seen_at(notification_cursor);
    bluesky_client.set_last_dm_cursor(dm_cursor);

    // Create scheduler
    let scheduler_client = AtprotoClient::new(&config.pds_url);
    scheduler_client
        .login(&config.handle, &config.app_password)
        .await
        .map_err(|e| miette::miette!("{}", e))?;
    let scheduler = Arc::new(Scheduler::new(scheduler_client));

    // Load existing jobs
    if let Err(e) = scheduler.load_jobs().await {
        error!(error = %e, "failed to load jobs, starting with empty job list");
    }

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

    // Create ATProto client for recording thoughts
    let atproto = AtprotoClient::new(&config.pds_url);
    atproto
        .login(&config.handle, &config.app_password)
        .await
        .map_err(|e| miette::miette!("{}", e))?;

    // Create ATProto client for job executor (fetching rule names)
    let executor_client = Arc::new(AtprotoClient::new(&config.pds_url));
    executor_client
        .login(&config.handle, &config.app_password)
        .await
        .map_err(|e| miette::miette!("{}", e))?;

    // Create executor for scheduled jobs
    let executor: winter_scheduler::JobExecutor = {
        let agent = Arc::clone(&agent);
        let identity_manager = Arc::clone(&identity_manager);
        let executor_client = Arc::clone(&executor_client);
        let cache = Arc::clone(&cache);

        Box::new(move |job| {
            let agent = Arc::clone(&agent);
            let identity_manager = Arc::clone(&identity_manager);
            let executor_client = Arc::clone(&executor_client);
            let cache = Arc::clone(&cache);

            Box::pin(async move {
                info!(name = %job.name, "executing scheduled job");

                // Load identity for context
                let identity = match identity_manager.load().await {
                    Ok(id) => id,
                    Err(e) => {
                        error!(error = ?e, "failed to load identity for job");
                        return Err(format!("failed to load identity: {}", e));
                    }
                };

                // Fetch rule heads and recent thoughts for context
                let rule_heads = fetch_rule_heads(&executor_client, Some(&cache)).await;
                let recent_thoughts =
                    fetch_recent_thoughts(&executor_client, Some(&cache), 10).await;

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
                        debug!(response_len = response.len(), job = %job.name, "job completed");
                        Ok(())
                    }
                    Err(e) => {
                        error!(error = ?e, job = %job.name, "job failed");
                        Err(format!("agent error: {}", e))
                    }
                }
            })
        })
    };

    // Run unified event loop
    run_event_loop(
        bluesky_client,
        &atproto,
        Duration::from_secs(config.poll_interval),
        shutdown_rx.clone(),
        agent,
        identity_manager,
        state_manager,
        scheduler,
        executor,
        Some(cache),
    )
    .await;

    // Wait for sync coordinator to finish
    if let Some(handle) = sync_handle {
        handle.await.ok();
    }

    info!("daemon shut down gracefully");
    Ok(())
}

/// Run the unified event loop.
#[allow(clippy::too_many_arguments)]
async fn run_event_loop(
    mut bluesky: BlueskyClient,
    atproto: &AtprotoClient,
    poll_interval: Duration,
    mut shutdown_rx: watch::Receiver<bool>,
    agent: Arc<Agent>,
    identity_manager: Arc<IdentityManager>,
    state_manager: Arc<StateManager>,
    scheduler: Arc<Scheduler>,
    executor: winter_scheduler::JobExecutor,
    cache: Option<Arc<RepoCache>>,
) {
    info!(
        poll_interval_secs = poll_interval.as_secs(),
        "starting unified event loop"
    );

    let mut notif_interval = tokio::time::interval(poll_interval);
    let mut dm_interval = tokio::time::interval(poll_interval);

    loop {
        // Race all event sources - first one wins, gets processed, then loop
        let event = tokio::select! {
            biased;

            // Check shutdown first
            _ = shutdown_rx.changed() => {
                if *shutdown_rx.borrow() {
                    info!("event loop received shutdown signal");
                    break;
                }
                continue;
            }

            // Notification poll tick
            _ = notif_interval.tick() => {
                poll_notifications(&mut bluesky).await
            }

            // DM poll tick
            _ = dm_interval.tick() => {
                poll_dms(&mut bluesky).await
            }

            // Job due (sleep until next job, or max 60s)
            _ = scheduler.sleep_until_next_job() => {
                scheduler.take_due_job().await.map(Event::Job)
            }
        };

        // Process the single event (if any)
        if let Some(event) = event {
            match event {
                Event::Notification(notif) => {
                    info!(
                        reason = ?notif.reason,
                        author = %notif.author_handle,
                        text = ?notif.text,
                        "processing notification"
                    );

                    // Load identity for context
                    let identity = match identity_manager.load().await {
                        Ok(id) => id,
                        Err(e) => {
                            error!(error = ?e, "failed to load identity for notification");
                            continue;
                        }
                    };

                    // Handle the notification with Claude
                    handle_notification(&notif, atproto, &agent, identity, cache.as_deref()).await;

                    // Persist notification cursor after processing
                    if let Some(cursor) = bluesky.last_seen_at()
                        && let Err(e) = state_manager
                            .set_notification_cursor(Some(cursor.to_string()))
                            .await
                    {
                        warn!(error = %e, "failed to persist notification cursor");
                    }
                }

                Event::DirectMessage(dm) => {
                    info!(
                        sender = %dm.sender_did,
                        convo_id = %dm.convo_id,
                        text = %dm.text,
                        "processing direct message"
                    );

                    // Load identity for context
                    let identity = match identity_manager.load().await {
                        Ok(id) => id,
                        Err(e) => {
                            error!(error = ?e, "failed to load identity for DM");
                            continue;
                        }
                    };

                    // Handle the DM with Claude
                    handle_dm(&dm, atproto, &agent, identity, &bluesky, cache.as_deref()).await;

                    // Persist DM cursor after processing
                    if let Some(cursor) = bluesky.last_dm_cursor()
                        && let Err(e) = state_manager.set_dm_cursor(Some(cursor.to_string())).await
                    {
                        warn!(error = %e, "failed to persist DM cursor");
                    }
                }

                Event::Job(job) => {
                    info!(name = %job.name, "processing scheduled job");
                    scheduler.execute_job(job, &executor).await;
                }
            }
        }
    }

    info!("event loop shut down gracefully");
}

/// Poll for notifications and return the first wakeup-worthy one.
async fn poll_notifications(bluesky: &mut BlueskyClient) -> Option<Event> {
    match bluesky.get_notifications(Some(50)).await {
        Ok(notifications) => {
            // Find first wakeup-worthy notification
            for notif in notifications {
                if notif.reason.triggers_wakeup() {
                    return Some(Event::Notification(notif));
                } else {
                    debug!(
                        reason = ?notif.reason,
                        author = %notif.author_handle,
                        "received non-wakeup notification"
                    );
                }
            }
            None
        }
        Err(BlueskyError::RateLimited { endpoint }) => {
            warn!(endpoint = ?endpoint, "notification poll rate limited");
            None
        }
        Err(e) => {
            warn!(error = %e, "notification poll failed");
            None
        }
    }
}

/// Poll for DMs and return the first unread one.
async fn poll_dms(bluesky: &mut BlueskyClient) -> Option<Event> {
    match bluesky.get_unread_dms().await {
        Ok(dms) => dms.into_iter().next().map(Event::DirectMessage),
        Err(e) => {
            warn!(error = %e, "DM poll failed");
            None
        }
    }
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

    // Record insight thought
    let observation = Thought {
        kind: ThoughtKind::Insight,
        content: content.clone(),
        trigger: Some(format!("notification:{}", notif.uri)),
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
    };

    // Fetch rule heads and recent thoughts for context
    let rule_heads = fetch_rule_heads(atproto, cache).await;
    let recent_thoughts = fetch_recent_thoughts(atproto, cache, 10).await;

    let context = AgentContext::new(identity)
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

            // Record completion thought
            let completion = Thought {
                kind: ThoughtKind::Response,
                content: truncate_chars(&response, 500),
                trigger: Some(format!("notification:{}", notif.uri)),
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
            // Record error thought
            let error_thought = Thought {
                kind: ThoughtKind::Error,
                content: format!("Failed to handle notification: {}", e),
                trigger: Some(format!("notification:{}", notif.uri)),
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
    agent: &Agent,
    identity: Identity,
    bluesky: &BlueskyClient,
    cache: Option<&RepoCache>,
) {
    let start = std::time::Instant::now();

    // Resolve sender handle from DID
    let sender_handle = format!("did:{}", &dm.sender_did[4..]); // Fallback to DID if resolution fails

    let content = format!("Received DM from {}: {}", sender_handle, dm.text);

    // Record insight thought
    let observation = Thought {
        kind: ThoughtKind::Insight,
        content: content.clone(),
        trigger: Some(format!("dm:{}:{}", dm.convo_id, dm.id)),
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

    // Build context for Claude
    let trigger = ContextTrigger::DirectMessage {
        convo_id: dm.convo_id.clone(),
        message_id: dm.id.clone(),
        sender_did: dm.sender_did.clone(),
        sender_handle: sender_handle.clone(),
        text: dm.text.clone(),
    };

    // Fetch rule heads and recent thoughts for context
    let rule_heads = fetch_rule_heads(atproto, cache).await;
    let recent_thoughts = fetch_recent_thoughts(atproto, cache, 10).await;

    let context = AgentContext::new(identity)
        .with_rule_heads(rule_heads)
        .with_thoughts(recent_thoughts)
        .with_trigger(trigger);

    // Invoke Claude via agent - Claude should use reply_to_dm tool to send the response
    match agent.handle_dm(context, &dm.text).await {
        Ok(response) => {
            let duration_ms = start.elapsed().as_millis() as u64;

            // Record completion thought (the actual reply is sent by Claude via reply_to_dm tool)
            let completion = Thought {
                kind: ThoughtKind::Response,
                content: truncate_chars(&response, 500),
                trigger: Some(format!("dm:{}:{}", dm.convo_id, dm.id)),
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
                trigger: Some(format!("dm:{}:{}", dm.convo_id, dm.id)),
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

    // bluesky client passed for future use if needed
    let _ = bluesky;
}
