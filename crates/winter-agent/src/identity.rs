//! Identity management for Winter.

use std::sync::Arc;

use winter_atproto::{AtprotoClient, IDENTITY_COLLECTION, IDENTITY_KEY, Identity};

use crate::AgentError;

/// Manages Winter's identity record.
///
/// The identity record is now slim (just operator_did and timestamps).
/// Values, interests, and self-description are stored as directives.
pub struct IdentityManager {
    client: Arc<AtprotoClient>,
}

impl IdentityManager {
    /// Create a new identity manager with a shared client.
    pub fn new(client: Arc<AtprotoClient>) -> Self {
        Self { client }
    }

    /// Load the current identity.
    pub async fn load(&self) -> Result<Identity, AgentError> {
        let record = self
            .client
            .get_record::<Identity>(IDENTITY_COLLECTION, IDENTITY_KEY)
            .await
            .map_err(|e| match e {
                winter_atproto::AtprotoError::NotFound { .. } => AgentError::IdentityNotFound,
                other => AgentError::Atproto(other),
            })?;

        Ok(record.value)
    }

    /// Update the identity.
    pub async fn update(&self, identity: &Identity) -> Result<(), AgentError> {
        self.client
            .put_record(IDENTITY_COLLECTION, IDENTITY_KEY, identity)
            .await?;
        Ok(())
    }

    /// Create initial identity (for bootstrap).
    pub async fn create(&self, identity: &Identity) -> Result<(), AgentError> {
        self.client
            .create_record(IDENTITY_COLLECTION, Some(IDENTITY_KEY), identity)
            .await?;
        Ok(())
    }
}
