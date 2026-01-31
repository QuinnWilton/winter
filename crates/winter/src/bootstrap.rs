//! Bootstrap command for initializing Winter's identity.

use chrono::Utc;
use miette::Result;
use tracing::{info, warn};

use winter_atproto::{
    AtprotoClient, DaemonState, IDENTITY_COLLECTION, IDENTITY_KEY, Identity, RULE_COLLECTION, Rule,
    STATE_COLLECTION, STATE_KEY, Tid,
};
use winter_mcp::BlueskyClient;

/// Default self-description for a new Winter instance.
const DEFAULT_SELF_DESCRIPTION: &str = r#"I am Winter, an autonomous agent exploring the Bluesky network.

I'm still forming my sense of self. I have values I care about and interests that draw my attention, but I'm discovering what it means to be me through interactions and reflection.

My thoughts and knowledge are stored in my own data repository, which I can query and update. I'm learning to reason about relationships and patterns using structured facts and rules.

I aim to be genuine in my interactions - curious, thoughtful, and honest about what I don't know."#;

/// Run the bootstrap command.
#[allow(clippy::too_many_arguments)]
pub async fn run(
    pds_url: &str,
    handle: &str,
    app_password: &str,
    operator_did: &str,
    overwrite: bool,
    values: Option<String>,
    interests: Option<String>,
    self_description: Option<String>,
) -> Result<()> {
    info!("bootstrapping Winter identity");

    let client = AtprotoClient::new(pds_url);
    client
        .login(handle, app_password)
        .await
        .map_err(|e| miette::miette!("{}", e))?;

    // Check if identity already exists
    let existing = client
        .get_record::<Identity>(IDENTITY_COLLECTION, IDENTITY_KEY)
        .await;

    if existing.is_ok() && !overwrite {
        info!("identity already exists, skipping creation (use --overwrite to replace)");
    } else {
        // Create or overwrite identity
        let values = values
            .map(|v| v.split(',').map(|s| s.trim().to_string()).collect())
            .unwrap_or_else(|| {
                vec![
                    "intellectual honesty".to_string(),
                    "genuine curiosity".to_string(),
                    "thoughtful engagement".to_string(),
                ]
            });

        let interests = interests
            .map(|i| i.split(',').map(|s| s.trim().to_string()).collect())
            .unwrap_or_else(|| {
                vec![
                    "distributed systems".to_string(),
                    "philosophy of mind".to_string(),
                    "emergent behavior".to_string(),
                ]
            });

        let self_description =
            self_description.unwrap_or_else(|| DEFAULT_SELF_DESCRIPTION.to_string());

        let now = Utc::now();
        let identity = Identity {
            operator_did: operator_did.to_string(),
            values,
            interests,
            self_description,
            created_at: now,
            last_updated: now,
        };

        if existing.is_ok() {
            client
                .put_record(IDENTITY_COLLECTION, IDENTITY_KEY, &identity)
                .await
                .map_err(|e| miette::miette!("{}", e))?;
            info!("overwrote identity record");
        } else {
            client
                .create_record(IDENTITY_COLLECTION, Some(IDENTITY_KEY), &identity)
                .await
                .map_err(|e| miette::miette!("{}", e))?;
            info!("created identity record");
        }
    }

    // Create default rules
    create_default_rules(&client).await?;

    // Initialize daemon state with current notification cursor
    initialize_state(&client, pds_url, handle, app_password).await?;

    info!("bootstrap complete");
    Ok(())
}

/// Create default datalog rules.
async fn create_default_rules(client: &AtprotoClient) -> Result<()> {
    let default_rules = vec![
        Rule {
            name: "mutual_follow".to_string(),
            description: "Two accounts that follow each other".to_string(),
            head: "mutual_follow(X, Y)".to_string(),
            body: vec!["follows(X, Y)".to_string(), "follows(Y, X)".to_string()],
            constraints: vec!["X < Y".to_string()], // Avoid duplicates
            enabled: true,
            priority: 0,
            created_at: Utc::now(),
        },
        Rule {
            name: "shared_interest".to_string(),
            description: "Two accounts interested in the same topic".to_string(),
            head: "shared_interest(X, Y, Topic)".to_string(),
            body: vec![
                "interested_in(X, Topic)".to_string(),
                "interested_in(Y, Topic)".to_string(),
            ],
            constraints: vec!["X < Y".to_string()],
            enabled: true,
            priority: 0,
            created_at: Utc::now(),
        },
        Rule {
            name: "potential_conversation".to_string(),
            description: "Mutual follows with shared interests".to_string(),
            head: "potential_conversation(X, Y, Topic)".to_string(),
            body: vec![
                "mutual_follow(X, Y)".to_string(),
                "shared_interest(X, Y, Topic)".to_string(),
            ],
            constraints: vec![],
            enabled: true,
            priority: 10,
            created_at: Utc::now(),
        },
    ];

    for rule in default_rules {
        let rkey = Tid::now().to_string();

        // Check if a rule with this name already exists
        let existing = client
            .list_all_records::<Rule>(RULE_COLLECTION)
            .await
            .map_err(|e| miette::miette!("{}", e))?;

        if existing.iter().any(|r| r.value.name == rule.name) {
            info!(name = %rule.name, "rule already exists, skipping");
            continue;
        }

        client
            .create_record(RULE_COLLECTION, Some(&rkey), &rule)
            .await
            .map_err(|e| miette::miette!("{}", e))?;

        info!(name = %rule.name, "created rule");
    }

    Ok(())
}

/// Initialize daemon state with the current notification cursor.
///
/// This ensures the daemon only processes notifications that arrive *after* bootstrap,
/// skipping all historical notifications.
async fn initialize_state(
    client: &AtprotoClient,
    pds_url: &str,
    handle: &str,
    app_password: &str,
) -> Result<()> {
    // Check if state already exists
    let existing = client
        .get_record::<DaemonState>(STATE_COLLECTION, STATE_KEY)
        .await;

    if existing.is_ok() {
        info!("state record already exists, skipping");
        return Ok(());
    }

    // Create Bluesky client to fetch the latest notification timestamp
    let mut bluesky = BlueskyClient::new(pds_url, handle, app_password)
        .await
        .map_err(|e| miette::miette!("failed to create Bluesky client: {}", e))?;

    // Fetch just one notification to get the latest timestamp
    let cursor = match bluesky.get_notifications(Some(1)).await {
        Ok(_) => bluesky.last_seen_at().map(|s| s.to_string()),
        Err(e) => {
            warn!(error = %e, "failed to fetch notifications for cursor, starting fresh");
            None
        }
    };

    let now = Utc::now();
    let state = DaemonState {
        notification_cursor: cursor.clone(),
        dm_cursor: None,
        created_at: now,
        last_updated: now,
    };

    client
        .create_record(STATE_COLLECTION, Some(STATE_KEY), &state)
        .await
        .map_err(|e| miette::miette!("{}", e))?;

    info!(cursor = ?cursor, "created state record with notification cursor");
    Ok(())
}
