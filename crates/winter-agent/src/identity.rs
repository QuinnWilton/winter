//! Identity management for Winter.

use winter_atproto::{AtprotoClient, IDENTITY_COLLECTION, IDENTITY_KEY, Identity};

use crate::AgentError;

/// Manages Winter's identity record.
pub struct IdentityManager {
    client: AtprotoClient,
}

impl IdentityManager {
    /// Create a new identity manager.
    pub fn new(client: AtprotoClient) -> Self {
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

    /// Add a value to the identity.
    pub async fn add_value(&self, value: String) -> Result<(), AgentError> {
        let mut identity = self.load().await?;
        if !identity.values.contains(&value) {
            identity.values.push(value);
            identity.last_updated = chrono::Utc::now();
            self.update(&identity).await?;
        }
        Ok(())
    }

    /// Remove a value from the identity.
    pub async fn remove_value(&self, value: &str) -> Result<(), AgentError> {
        let mut identity = self.load().await?;
        identity.values.retain(|v| v != value);
        identity.last_updated = chrono::Utc::now();
        self.update(&identity).await?;
        Ok(())
    }

    /// Add an interest to the identity.
    pub async fn add_interest(&self, interest: String) -> Result<(), AgentError> {
        let mut identity = self.load().await?;
        if !identity.interests.contains(&interest) {
            identity.interests.push(interest);
            identity.last_updated = chrono::Utc::now();
            self.update(&identity).await?;
        }
        Ok(())
    }

    /// Remove an interest from the identity.
    pub async fn remove_interest(&self, interest: &str) -> Result<(), AgentError> {
        let mut identity = self.load().await?;
        identity.interests.retain(|i| i != interest);
        identity.last_updated = chrono::Utc::now();
        self.update(&identity).await?;
        Ok(())
    }

    /// Update self_description.
    pub async fn update_self_description(&self, description: String) -> Result<(), AgentError> {
        let mut identity = self.load().await?;
        identity.self_description = description;
        identity.last_updated = chrono::Utc::now();
        self.update(&identity).await?;
        Ok(())
    }
}
