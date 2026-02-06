//! Winter: Autonomous Bluesky Agent
//!
//! Main binary with subcommands:
//! - `daemon`: Main loop (notification polling, scheduler)
//! - `mcp-server`: MCP server mode for Claude Code
//! - `web`: Read-only observation web UI
//! - `bootstrap`: Initialize identity and rules

use clap::{Parser, Subcommand};
use miette::Result;
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};

/// Parse boolean from environment variable, accepting common truthy values.
/// Accepts "1", "true", "yes", "on" (case-insensitive) as true.
/// Accepts "0", "false", "no", "off", "" (case-insensitive) as false.
fn parse_bool_env(s: &str) -> Result<bool, String> {
    match s.to_lowercase().as_str() {
        "1" | "true" | "yes" | "on" => Ok(true),
        "0" | "false" | "no" | "off" | "" => Ok(false),
        _ => Err(format!(
            "invalid boolean value '{}', expected 1/true/yes/on or 0/false/no/off",
            s
        )),
    }
}

mod bootstrap;
mod daemon;
mod migrate;

#[derive(Parser)]
#[command(name = "winter")]
#[command(about = "Autonomous Bluesky Agent", long_about = None)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Run the main daemon (notification polling, scheduler)
    Daemon {
        /// PDS URL
        #[arg(long, env = "WINTER_PDS_URL")]
        pds_url: String,

        /// Account handle
        #[arg(long, env = "WINTER_HANDLE")]
        handle: String,

        /// App password
        #[arg(long, env = "WINTER_APP_PASSWORD")]
        app_password: String,

        /// Notification poll interval in seconds
        #[arg(long, default_value = "5")]
        poll_interval: u64,

        /// Awaken interval in seconds
        #[arg(long, default_value = "3600")]
        awaken_interval: u64,

        /// Follower sync interval in seconds (for populating is_followed_by predicate).
        /// New followers are detected immediately via Follow notifications.
        /// This full sync is mainly for reconciliation (catching unfollows).
        #[arg(long, default_value = "86400")]
        follower_sync_interval: u64,

        /// Fast-forward notification and DM cursors to current time on startup.
        /// This skips all existing notifications/DMs and only processes new ones.
        /// Accepts "1", "true", "yes", or any non-empty value.
        #[arg(long, env = "WINTER_FAST_FORWARD", value_parser = parse_bool_env, default_value = "false")]
        fast_forward: bool,
    },

    /// Run the MCP server (for Claude Code) using stdio transport
    McpServer {
        /// PDS URL
        #[arg(long, env = "WINTER_PDS_URL")]
        pds_url: String,

        /// Account handle
        #[arg(long, env = "WINTER_HANDLE")]
        handle: String,

        /// App password
        #[arg(long, env = "WINTER_APP_PASSWORD")]
        app_password: String,
    },

    /// Run the MCP server with HTTP transport (persistent, for Docker)
    McpServerHttp {
        /// PDS URL
        #[arg(long, env = "WINTER_PDS_URL")]
        pds_url: String,

        /// Account handle
        #[arg(long, env = "WINTER_HANDLE")]
        handle: String,

        /// App password
        #[arg(long, env = "WINTER_APP_PASSWORD")]
        app_password: String,

        /// HTTP server port
        #[arg(long, default_value = "3847")]
        port: u16,
    },

    /// Run the web UI server
    Web {
        /// PDS URL
        #[arg(long, env = "WINTER_PDS_URL")]
        pds_url: String,

        /// Account handle
        #[arg(long, env = "WINTER_HANDLE")]
        handle: String,

        /// App password
        #[arg(long, env = "WINTER_APP_PASSWORD")]
        app_password: String,

        /// Web server port
        #[arg(long, default_value = "8080")]
        port: u16,

        /// Static files directory
        #[arg(long)]
        static_dir: Option<String>,

        /// Firehose URL for real-time thought updates (e.g., wss://bsky.network)
        #[arg(long, env = "WINTER_FIREHOSE_URL")]
        firehose_url: Option<String>,
    },

    /// Initialize identity and default rules
    Bootstrap {
        /// PDS URL
        #[arg(long, env = "WINTER_PDS_URL")]
        pds_url: String,

        /// Account handle
        #[arg(long, env = "WINTER_HANDLE")]
        handle: String,

        /// App password
        #[arg(long, env = "WINTER_APP_PASSWORD")]
        app_password: String,

        /// Operator DID (the human who controls this instance)
        #[arg(long, env = "WINTER_OPERATOR_DID")]
        operator_did: String,

        /// Overwrite existing identity if it exists
        #[arg(long)]
        overwrite: bool,

        /// Initial values (comma-separated)
        #[arg(long)]
        values: Option<String>,

        /// Initial interests (comma-separated)
        #[arg(long)]
        interests: Option<String>,

        /// Initial self-description
        #[arg(long)]
        self_description: Option<String>,
    },

    /// Migrate identity from legacy format to directives (deprecated, use `migrate` instead)
    MigrateIdentity {
        /// PDS URL
        #[arg(long, env = "WINTER_PDS_URL")]
        pds_url: String,

        /// Account handle
        #[arg(long, env = "WINTER_HANDLE")]
        handle: String,

        /// App password
        #[arg(long, env = "WINTER_APP_PASSWORD")]
        app_password: String,
    },

    /// Run data migrations
    Migrate {
        /// PDS URL
        #[arg(long, env = "WINTER_PDS_URL")]
        pds_url: String,

        /// Account handle
        #[arg(long, env = "WINTER_HANDLE")]
        handle: String,

        /// App password
        #[arg(long, env = "WINTER_APP_PASSWORD")]
        app_password: String,

        /// Migration name to run
        #[arg(value_name = "MIGRATION")]
        migration: Option<String>,

        /// List available migrations
        #[arg(long)]
        list: bool,

        /// Preview changes without applying (dry-run)
        #[arg(long)]
        dry_run: bool,

        /// Run all pending migrations
        #[arg(long)]
        all: bool,
    },
}

#[tokio::main]
async fn main() -> Result<()> {
    // Initialize tracing
    tracing_subscriber::registry()
        .with(tracing_subscriber::EnvFilter::new(
            std::env::var("RUST_LOG").unwrap_or_else(|_| "winter=info".to_string()),
        ))
        .with(tracing_subscriber::fmt::layer())
        .init();

    let cli = Cli::parse();

    match cli.command {
        Commands::Daemon {
            pds_url,
            handle,
            app_password,
            poll_interval,
            awaken_interval,
            follower_sync_interval,
            fast_forward,
        } => {
            daemon::run(
                &pds_url,
                &handle,
                &app_password,
                poll_interval,
                awaken_interval,
                follower_sync_interval,
                fast_forward,
            )
            .await
        }

        Commands::McpServer {
            pds_url,
            handle,
            app_password,
        } => run_mcp_server(&pds_url, &handle, &app_password).await,

        Commands::McpServerHttp {
            pds_url,
            handle,
            app_password,
            port,
        } => run_mcp_server_http(&pds_url, &handle, &app_password, port).await,

        Commands::Web {
            pds_url,
            handle,
            app_password,
            port,
            static_dir,
            firehose_url,
        } => {
            run_web_server(
                &pds_url,
                &handle,
                &app_password,
                port,
                static_dir.as_deref(),
                firehose_url,
            )
            .await
        }

        Commands::Bootstrap {
            pds_url,
            handle,
            app_password,
            operator_did,
            overwrite,
            values,
            interests,
            self_description,
        } => {
            bootstrap::run(
                &pds_url,
                &handle,
                &app_password,
                &operator_did,
                overwrite,
                values,
                interests,
                self_description,
            )
            .await
        }

        Commands::MigrateIdentity {
            pds_url,
            handle,
            app_password,
        } => migrate::run(&pds_url, &handle, &app_password).await,

        Commands::Migrate {
            pds_url,
            handle,
            app_password,
            migration,
            list,
            dry_run,
            all,
        } => {
            migrate::run_migrate_command(
                &pds_url,
                &handle,
                &app_password,
                migration.as_deref(),
                list,
                dry_run,
                all,
            )
            .await
        }
    }
}

async fn run_mcp_server(pds_url: &str, handle: &str, app_password: &str) -> Result<()> {
    use std::sync::Arc;
    use winter_atproto::{AtprotoClient, RepoCache, SyncCoordinator};
    use winter_datalog::DatalogCache;
    use winter_mcp::{BlueskyClient, DenoExecutor, McpServer, SecretManager, tools::ToolRegistry};

    // Create two clients - one for tools, one for sync
    let client = AtprotoClient::new(pds_url);
    client
        .login(handle, app_password)
        .await
        .map_err(|e| miette::miette!("{}", e))?;

    // Get the DID for this account
    let did = client
        .did()
        .await
        .ok_or_else(|| miette::miette!("failed to get DID: not logged in"))?;

    // Create second client for sync coordinator
    let sync_client = AtprotoClient::new(pds_url);
    sync_client
        .login(handle, app_password)
        .await
        .map_err(|e| miette::miette!("{}", e))?;

    // Create Bluesky client for social interactions
    let bluesky = BlueskyClient::new(pds_url, handle, app_password)
        .await
        .map_err(|e| miette::miette!("failed to create Bluesky client: {}", e))?;

    let tools = ToolRegistry::new(client).with_bluesky(bluesky);

    // Set up RepoCache and DatalogCache for derived predicates
    let repo_cache = RepoCache::new();

    // Use a temp directory for MCP server's datalog cache to avoid conflicts
    // with the daemon's cache (which does follower sync and other writes).
    // Each MCP instance gets isolated TSV files.
    let temp_cache_dir = tempfile::tempdir()
        .map_err(|e| miette::miette!("failed to create temp cache directory: {}", e))?;
    let cache_dir = temp_cache_dir.path().to_path_buf();
    // Keep temp_cache_dir alive for the duration of the MCP server
    let _cache_dir_guard = temp_cache_dir;

    let datalog_cache =
        DatalogCache::new_with_did(&cache_dir, Some(did.clone()), Some(handle.to_string()))
            .map_err(|e| miette::miette!("failed to create datalog cache: {}", e))?;

    // Start sync coordinator to populate repo cache from PDS
    // Resolve actual PDS firehose from DID document (only our commits, not full network)
    let firehose_url = match std::env::var("WINTER_FIREHOSE_URL").ok().filter(|s| !s.is_empty()) {
        Some(url) => url,
        None => winter_atproto::resolve_firehose_url(&did, pds_url).await,
    };

    let sync_coordinator = SyncCoordinator::new(sync_client, &did, Arc::clone(&repo_cache))
        .with_firehose_url(&firehose_url);

    // Create a shutdown channel (MCP server runs until process exits)
    let (_shutdown_tx, shutdown_rx) = tokio::sync::watch::channel(false);

    // Start the sync (downloads CAR file and subscribes to firehose)
    let _sync_handle = sync_coordinator
        .start(shutdown_rx)
        .await
        .map_err(|e| miette::miette!("failed to start sync: {}", e))?;

    // Connect datalog cache to repo cache for derived fact population
    // This also populates is_followed_by from the daemon state record in the CAR file
    datalog_cache.start_update_listener(Arc::clone(&repo_cache));

    // Set the caches on the tool registry
    tools.set_cache(repo_cache).await;
    tools.set_datalog_cache(Arc::clone(&datalog_cache)).await;

    tracing::info!("datalog cache initialized for MCP server");

    // Load secrets from configured path or default
    let secrets_path = std::env::var("WINTER_SECRETS_PATH")
        .ok()
        .map(std::path::PathBuf::from);
    match SecretManager::load(secrets_path).await {
        Ok(secrets) => {
            tools.set_secrets(secrets).await;
            tracing::info!("loaded secrets manager");
        }
        Err(e) => {
            tracing::warn!(error = %e, "failed to load secrets manager, custom tools will have no secrets");
        }
    }

    // Set up Deno executor for custom tools
    if DenoExecutor::is_available().await {
        tools.set_deno(DenoExecutor::default()).await;
        tracing::info!("Deno executor available for custom tools");
    } else {
        tracing::warn!("Deno not found, custom tools will not be executable");
    }

    let server = McpServer::new(tools);
    server.run().await.map_err(|e| miette::miette!("{}", e))?;

    Ok(())
}

async fn run_mcp_server_http(
    pds_url: &str,
    handle: &str,
    app_password: &str,
    port: u16,
) -> Result<()> {
    use std::sync::Arc;
    use winter_atproto::{AtprotoClient, RepoCache, SyncCoordinator};
    use winter_datalog::DatalogCache;
    use winter_mcp::{
        BlueskyClient, DenoExecutor, McpServer, SecretManager, http, tools::ToolRegistry,
    };

    tracing::info!("starting MCP HTTP server on port {}", port);

    // Create two clients - one for tools, one for sync
    let client = AtprotoClient::new(pds_url);
    client
        .login(handle, app_password)
        .await
        .map_err(|e| miette::miette!("{}", e))?;

    // Get the DID for this account
    let did = client
        .did()
        .await
        .ok_or_else(|| miette::miette!("failed to get DID: not logged in"))?;

    // Create second client for sync coordinator
    let sync_client = AtprotoClient::new(pds_url);
    sync_client
        .login(handle, app_password)
        .await
        .map_err(|e| miette::miette!("{}", e))?;

    // Create Bluesky client for social interactions
    let bluesky = BlueskyClient::new(pds_url, handle, app_password)
        .await
        .map_err(|e| miette::miette!("failed to create Bluesky client: {}", e))?;

    let tools = ToolRegistry::new(client).with_bluesky(bluesky);

    // Set up RepoCache and DatalogCache for derived predicates
    let repo_cache = RepoCache::new();

    // Use a persistent cache directory for the HTTP server
    // This allows the cache to survive restarts, unlike the temp dir used for stdio
    let cache_dir = dirs::cache_dir()
        .unwrap_or_else(|| std::path::PathBuf::from("."))
        .join("winter")
        .join("mcp-http");

    std::fs::create_dir_all(&cache_dir)
        .map_err(|e| miette::miette!("failed to create cache directory: {}", e))?;

    let datalog_cache =
        DatalogCache::new_with_did(&cache_dir, Some(did.clone()), Some(handle.to_string()))
            .map_err(|e| miette::miette!("failed to create datalog cache: {}", e))?;

    // Start sync coordinator to populate repo cache from PDS
    // Resolve actual PDS firehose from DID document (only our commits, not full network)
    let firehose_url = match std::env::var("WINTER_FIREHOSE_URL").ok().filter(|s| !s.is_empty()) {
        Some(url) => url,
        None => winter_atproto::resolve_firehose_url(&did, pds_url).await,
    };

    let sync_coordinator = SyncCoordinator::new(sync_client, &did, Arc::clone(&repo_cache))
        .with_firehose_url(&firehose_url);

    // Create a shutdown channel (HTTP server runs until process exits)
    let (_shutdown_tx, shutdown_rx) = tokio::sync::watch::channel(false);

    // Start the sync (downloads CAR file and subscribes to firehose)
    let _sync_handle = sync_coordinator
        .start(shutdown_rx)
        .await
        .map_err(|e| miette::miette!("failed to start sync: {}", e))?;

    // Connect datalog cache to repo cache for derived fact population
    datalog_cache.start_update_listener(Arc::clone(&repo_cache));

    // Set the caches on the tool registry
    tools.set_cache(repo_cache).await;
    tools.set_datalog_cache(Arc::clone(&datalog_cache)).await;

    tracing::info!("datalog cache initialized for MCP HTTP server");

    // Load secrets from configured path or default
    let secrets_path = std::env::var("WINTER_SECRETS_PATH")
        .ok()
        .map(std::path::PathBuf::from);
    match SecretManager::load(secrets_path).await {
        Ok(secrets) => {
            tools.set_secrets(secrets).await;
            tracing::info!("loaded secrets manager");
        }
        Err(e) => {
            tracing::warn!(error = %e, "failed to load secrets manager, custom tools will have no secrets");
        }
    }

    // Set up Deno executor for custom tools
    if DenoExecutor::is_available().await {
        tools.set_deno(DenoExecutor::default()).await;
        tracing::info!("Deno executor available for custom tools");
    } else {
        tracing::warn!("Deno not found, custom tools will not be executable");
    }

    let server = McpServer::new(tools);

    // Run the HTTP server (blocks until shutdown)
    http::run_server(server, port)
        .await
        .map_err(|e| miette::miette!("HTTP server error: {}", e))?;

    Ok(())
}

async fn run_web_server(
    pds_url: &str,
    handle: &str,
    app_password: &str,
    port: u16,
    static_dir: Option<&str>,
    firehose_url: Option<String>,
) -> Result<()> {
    use winter_atproto::AtprotoClient;
    use winter_mcp::SecretManager;
    use winter_web::create_router_with_secrets;

    let client = AtprotoClient::new(pds_url);
    client
        .login(handle, app_password)
        .await
        .map_err(|e| miette::miette!("{}", e))?;

    // Get DID for firehose subscription
    let did = client.did().await;

    // Load secret manager for the secrets page
    let secrets_path = std::env::var("WINTER_SECRETS_PATH")
        .ok()
        .map(std::path::PathBuf::from);
    let secrets = match SecretManager::load(secrets_path).await {
        Ok(s) => {
            tracing::info!("loaded secrets manager for web UI");
            Some(s)
        }
        Err(e) => {
            tracing::warn!(error = %e, "failed to load secrets manager");
            None
        }
    };

    // Resolve actual PDS firehose from DID document (only our commits, not full network)
    let firehose_url = match firehose_url.filter(|s| !s.is_empty()) {
        Some(url) => Some(url),
        None => {
            let did_str = did.as_deref().unwrap_or("");
            if !did_str.is_empty() {
                Some(winter_atproto::resolve_firehose_url(did_str, pds_url).await)
            } else {
                None
            }
        }
    };

    let router = create_router_with_secrets(client, static_dir, firehose_url, did, secrets);

    let listener = tokio::net::TcpListener::bind(format!("0.0.0.0:{}", port))
        .await
        .map_err(|e| miette::miette!("{}", e))?;

    tracing::info!("web server listening on http://0.0.0.0:{}", port);

    axum::serve(listener, router)
        .await
        .map_err(|e| miette::miette!("{}", e))?;

    Ok(())
}
