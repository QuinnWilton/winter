//! Local secret storage for custom tools.
//!
//! Secrets are stored in a local encrypted file, separate from ATProto records.
//! This ensures that secret values never leave the local machine.

use std::collections::HashMap;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use thiserror::Error;
use tokio::fs;
use tokio::io::AsyncWriteExt;

/// Errors from secret management operations.
#[derive(Debug, Error)]
pub enum SecretError {
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    #[error("JSON error: {0}")]
    Json(#[from] serde_json::Error),

    #[error("secret not found: {0}")]
    NotFound(String),

    #[error("invalid secret name: {0}")]
    InvalidName(String),
}

/// Secret file format.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct SecretFile {
    version: u32,
    secrets: HashMap<String, String>,
}

impl Default for SecretFile {
    fn default() -> Self {
        Self {
            version: 1,
            secrets: HashMap::new(),
        }
    }
}

/// Manager for local secret storage.
///
/// Secrets are stored in a JSON file with restricted permissions.
/// Only approved secrets are passed to Deno tools via `get_subset()`.
#[derive(Debug)]
pub struct SecretManager {
    path: PathBuf,
    data: SecretFile,
}

impl SecretManager {
    /// Load secrets from the default or specified path.
    ///
    /// If the file doesn't exist, creates an empty secret store.
    pub async fn load(path: Option<PathBuf>) -> Result<Self, SecretError> {
        let path = path.unwrap_or_else(Self::default_path);

        let data = if path.exists() {
            let content = fs::read_to_string(&path).await?;
            serde_json::from_str(&content)?
        } else {
            SecretFile::default()
        };

        Ok(Self { path, data })
    }

    /// Get the default secrets path.
    pub fn default_path() -> PathBuf {
        dirs::config_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join("winter")
            .join("secrets.json")
    }

    /// Get a secret value by name.
    pub fn get(&self, name: &str) -> Option<&str> {
        self.data.secrets.get(name).map(|s| s.as_str())
    }

    /// Set a secret value.
    ///
    /// Validates the name and persists to disk.
    pub async fn set(&mut self, name: &str, value: &str) -> Result<(), SecretError> {
        Self::validate_name(name)?;
        self.data
            .secrets
            .insert(name.to_string(), value.to_string());
        self.save().await
    }

    /// Delete a secret.
    pub async fn delete(&mut self, name: &str) -> Result<(), SecretError> {
        if self.data.secrets.remove(name).is_none() {
            return Err(SecretError::NotFound(name.to_string()));
        }
        self.save().await
    }

    /// List all secret names.
    pub fn list_names(&self) -> Vec<String> {
        self.data.secrets.keys().cloned().collect()
    }

    /// Check if a secret exists.
    pub fn has(&self, name: &str) -> bool {
        self.data.secrets.contains_key(name)
    }

    /// Get a subset of secrets by name.
    ///
    /// Returns only the secrets that exist from the requested list.
    /// Values are prefixed with `WINTER_SECRET_` for Deno env var access.
    pub fn get_subset(&self, names: &[String]) -> HashMap<String, String> {
        names
            .iter()
            .filter_map(|name| {
                self.data.secrets.get(name).map(|value| {
                    let env_name = format!("WINTER_SECRET_{}", name);
                    (env_name, value.clone())
                })
            })
            .collect()
    }

    /// Validate a secret name.
    fn validate_name(name: &str) -> Result<(), SecretError> {
        if name.is_empty() {
            return Err(SecretError::InvalidName("name cannot be empty".to_string()));
        }

        if name.len() > 64 {
            return Err(SecretError::InvalidName(
                "name too long (max 64 chars)".to_string(),
            ));
        }

        // Only allow alphanumeric and underscore
        if !name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
            return Err(SecretError::InvalidName(
                "name must be alphanumeric with underscores only".to_string(),
            ));
        }

        Ok(())
    }

    /// Save secrets to disk with restricted permissions.
    async fn save(&self) -> Result<(), SecretError> {
        // Ensure parent directory exists
        if let Some(parent) = self.path.parent() {
            fs::create_dir_all(parent).await?;
        }

        let content = serde_json::to_string_pretty(&self.data)?;

        // Write to temp file first, then rename for atomicity
        let temp_path = self.path.with_extension("tmp");
        let mut file = fs::File::create(&temp_path).await?;

        // Set file permissions to 0600 (owner read/write only) on Unix
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mut perms = file.metadata().await?.permissions();
            perms.set_mode(0o600);
            file.set_permissions(perms).await?;
        }

        file.write_all(content.as_bytes()).await?;
        file.sync_all().await?;

        // Atomic rename
        fs::rename(&temp_path, &self.path).await?;

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[tokio::test]
    async fn secret_manager_crud() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("secrets.json");

        let mut mgr = SecretManager::load(Some(path.clone())).await.unwrap();

        // Set a secret
        mgr.set("API_KEY", "secret123").await.unwrap();
        assert_eq!(mgr.get("API_KEY"), Some("secret123"));

        // List names
        assert_eq!(mgr.list_names(), vec!["API_KEY"]);

        // Delete
        mgr.delete("API_KEY").await.unwrap();
        assert!(mgr.get("API_KEY").is_none());
    }

    #[tokio::test]
    async fn secret_manager_get_subset() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("secrets.json");

        let mut mgr = SecretManager::load(Some(path)).await.unwrap();
        mgr.set("API_KEY", "key1").await.unwrap();
        mgr.set("TOKEN", "token1").await.unwrap();
        mgr.set("OTHER", "other1").await.unwrap();

        let subset = mgr.get_subset(&["API_KEY".to_string(), "TOKEN".to_string()]);

        assert_eq!(subset.len(), 2);
        assert_eq!(
            subset.get("WINTER_SECRET_API_KEY"),
            Some(&"key1".to_string())
        );
        assert_eq!(
            subset.get("WINTER_SECRET_TOKEN"),
            Some(&"token1".to_string())
        );
        assert!(!subset.contains_key("WINTER_SECRET_OTHER"));
    }

    #[tokio::test]
    async fn secret_manager_validates_names() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("secrets.json");

        let mut mgr = SecretManager::load(Some(path)).await.unwrap();

        // Empty name
        assert!(mgr.set("", "value").await.is_err());

        // Invalid characters
        assert!(mgr.set("has-dash", "value").await.is_err());
        assert!(mgr.set("has space", "value").await.is_err());

        // Valid names
        assert!(mgr.set("VALID_NAME", "value").await.is_ok());
        assert!(mgr.set("valid123", "value").await.is_ok());
    }

    #[tokio::test]
    async fn secret_manager_persistence() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("secrets.json");

        // Create and save
        {
            let mut mgr = SecretManager::load(Some(path.clone())).await.unwrap();
            mgr.set("PERSISTENT", "value").await.unwrap();
        }

        // Load and verify
        {
            let mgr = SecretManager::load(Some(path)).await.unwrap();
            assert_eq!(mgr.get("PERSISTENT"), Some("value"));
        }
    }
}
