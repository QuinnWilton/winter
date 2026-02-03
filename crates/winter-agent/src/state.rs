//! Daemon state management for Winter.

use std::sync::Arc;

use chrono::Utc;
use winter_atproto::{AtprotoClient, DaemonState, STATE_COLLECTION, STATE_KEY};

use crate::AgentError;

/// Manages Winter's daemon state record.
pub struct StateManager {
    client: Arc<AtprotoClient>,
}

impl StateManager {
    /// Create a new state manager with a shared client.
    pub fn new(client: Arc<AtprotoClient>) -> Self {
        Self { client }
    }

    /// Load the current state, creating a default if it doesn't exist.
    pub async fn load(&self) -> Result<DaemonState, AgentError> {
        match self
            .client
            .get_record::<DaemonState>(STATE_COLLECTION, STATE_KEY)
            .await
        {
            Ok(record) => Ok(record.value),
            Err(winter_atproto::AtprotoError::NotFound { .. }) => {
                // Create default state
                let now = Utc::now();
                let state = DaemonState {
                    notification_cursor: None,
                    dm_cursor: None,
                    followers: Vec::new(),
                    created_at: now,
                    last_updated: now,
                };
                self.create(&state).await?;
                Ok(state)
            }
            Err(e) => Err(AgentError::Atproto(e)),
        }
    }

    /// Get the notification cursor.
    pub async fn get_notification_cursor(&self) -> Result<Option<String>, AgentError> {
        let state = self.load().await?;
        Ok(state.notification_cursor)
    }

    /// Set the notification cursor.
    pub async fn set_notification_cursor(&self, cursor: Option<String>) -> Result<(), AgentError> {
        let mut state = self.load().await?;
        state.notification_cursor = cursor;
        state.last_updated = Utc::now();
        self.update(&state).await
    }

    /// Get the DM cursor.
    pub async fn get_dm_cursor(&self) -> Result<Option<String>, AgentError> {
        let state = self.load().await?;
        Ok(state.dm_cursor)
    }

    /// Set the DM cursor.
    pub async fn set_dm_cursor(&self, cursor: Option<String>) -> Result<(), AgentError> {
        let mut state = self.load().await?;
        state.dm_cursor = cursor;
        state.last_updated = Utc::now();
        self.update(&state).await
    }

    /// Get the followers list.
    pub async fn get_followers(&self) -> Result<Vec<String>, AgentError> {
        let state = self.load().await?;
        Ok(state.followers)
    }

    /// Set the followers list.
    pub async fn set_followers(&self, followers: Vec<String>) -> Result<(), AgentError> {
        let mut state = self.load().await?;
        state.followers = followers;
        state.last_updated = Utc::now();
        self.update(&state).await
    }

    /// Update the state record.
    async fn update(&self, state: &DaemonState) -> Result<(), AgentError> {
        self.client
            .put_record(STATE_COLLECTION, STATE_KEY, state)
            .await?;
        Ok(())
    }

    /// Create initial state (for bootstrap or first load).
    pub async fn create(&self, state: &DaemonState) -> Result<(), AgentError> {
        self.client
            .create_record(STATE_COLLECTION, Some(STATE_KEY), state)
            .await?;
        Ok(())
    }
}
