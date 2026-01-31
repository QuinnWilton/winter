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

mod bootstrap;
mod daemon;

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
    },

    /// Run the MCP server (for Claude Code)
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
        } => {
            daemon::run(
                &pds_url,
                &handle,
                &app_password,
                poll_interval,
                awaken_interval,
            )
            .await
        }

        Commands::McpServer {
            pds_url,
            handle,
            app_password,
        } => run_mcp_server(&pds_url, &handle, &app_password).await,

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
    }
}

async fn run_mcp_server(pds_url: &str, handle: &str, app_password: &str) -> Result<()> {
    use winter_atproto::AtprotoClient;
    use winter_mcp::{BlueskyClient, McpServer, tools::ToolRegistry};

    let client = AtprotoClient::new(pds_url);
    client
        .login(handle, app_password)
        .await
        .map_err(|e| miette::miette!("{}", e))?;

    // Create Bluesky client for social interactions
    let bluesky = BlueskyClient::new(pds_url, handle, app_password)
        .await
        .map_err(|e| miette::miette!("failed to create Bluesky client: {}", e))?;

    let tools = ToolRegistry::new(client).with_bluesky(bluesky);
    let mut server = McpServer::new(tools);
    server.run().await.map_err(|e| miette::miette!("{}", e))?;

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
    use winter_web::create_router;

    let client = AtprotoClient::new(pds_url);
    client
        .login(handle, app_password)
        .await
        .map_err(|e| miette::miette!("{}", e))?;

    // Get DID for firehose subscription
    let did = client.did().await;

    let router = create_router(client, static_dir, firehose_url, did);

    let listener = tokio::net::TcpListener::bind(format!("0.0.0.0:{}", port))
        .await
        .map_err(|e| miette::miette!("{}", e))?;

    tracing::info!("web server listening on http://0.0.0.0:{}", port);

    axum::serve(listener, router)
        .await
        .map_err(|e| miette::miette!("{}", e))?;

    Ok(())
}
