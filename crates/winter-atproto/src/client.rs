//! ATProto XRPC client implementation.

use std::sync::Arc;
use std::time::Duration;

use reqwest::Client;
use serde::{Serialize, de::DeserializeOwned};
use tokio::sync::RwLock;
use tracing::{debug, warn};

use crate::{
    AtprotoError, CreateRecordResponse, GetRecordResponse, ListRecordItem, ListRecordsResponse,
    Session,
};

/// Client for interacting with an ATProto PDS.
pub struct AtprotoClient {
    http: Client,
    pds_url: String,
    session: Arc<RwLock<Option<Session>>>,
}

impl AtprotoClient {
    /// Create a new client for the given PDS URL.
    pub fn new(pds_url: impl Into<String>) -> Self {
        let http = Client::builder()
            .connect_timeout(Duration::from_secs(10))
            .timeout(Duration::from_secs(30))
            .build()
            .expect("failed to build HTTP client");

        Self {
            http,
            pds_url: pds_url.into(),
            session: Arc::new(RwLock::new(None)),
        }
    }

    /// Authenticate with the PDS using identifier and password.
    pub async fn login(&self, identifier: &str, password: &str) -> Result<(), AtprotoError> {
        #[derive(Serialize)]
        struct LoginRequest<'a> {
            identifier: &'a str,
            password: &'a str,
        }

        let url = format!("{}/xrpc/com.atproto.server.createSession", self.pds_url);

        let response = self
            .http
            .post(&url)
            .json(&LoginRequest {
                identifier,
                password,
            })
            .send()
            .await?;

        if !response.status().is_success() {
            let status = response.status();
            let text = response.text().await.map_err(|e| {
                AtprotoError::Auth(format!(
                    "login failed ({}): failed to read response: {}",
                    status, e
                ))
            })?;
            return Err(AtprotoError::Auth(format!(
                "login failed ({}): {}",
                status, text
            )));
        }

        let session: Session = response.json().await?;
        debug!(did = %session.did, handle = %session.handle, "authenticated with PDS");

        *self.session.write().await = Some(session);
        Ok(())
    }

    /// Refresh the current session tokens.
    pub async fn refresh_session(&self) -> Result<(), AtprotoError> {
        let refresh_jwt = {
            let session = self.session.read().await;
            session
                .as_ref()
                .map(|s| s.refresh_jwt.clone())
                .ok_or_else(|| AtprotoError::Auth("no session to refresh".to_string()))?
        };

        let url = format!("{}/xrpc/com.atproto.server.refreshSession", self.pds_url);

        let response = self
            .http
            .post(&url)
            .header("Authorization", format!("Bearer {}", refresh_jwt))
            .send()
            .await?;

        if !response.status().is_success() {
            let status = response.status();
            let text = response.text().await.map_err(|e| {
                AtprotoError::Auth(format!(
                    "refresh failed ({}): failed to read response: {}",
                    status, e
                ))
            })?;
            return Err(AtprotoError::Auth(format!(
                "refresh failed ({}): {}",
                status, text
            )));
        }

        let session: Session = response.json().await?;
        debug!(did = %session.did, "refreshed session");

        *self.session.write().await = Some(session);
        Ok(())
    }

    /// Get the current session DID.
    pub async fn did(&self) -> Option<String> {
        self.session.read().await.as_ref().map(|s| s.did.clone())
    }

    /// Get the current session handle.
    pub async fn handle(&self) -> Option<String> {
        self.session.read().await.as_ref().map(|s| s.handle.clone())
    }

    /// Get the current access token.
    async fn access_token(&self) -> Result<String, AtprotoError> {
        self.session
            .read()
            .await
            .as_ref()
            .map(|s| s.access_jwt.clone())
            .ok_or_else(|| AtprotoError::Auth("not authenticated".to_string()))
    }

    /// Check if an error indicates an expired token.
    fn is_expired_token_error(err: &AtprotoError) -> bool {
        matches!(
            err,
            AtprotoError::Xrpc { error, .. } if error == "ExpiredToken"
        )
    }

    /// Try to refresh the session if possible.
    /// Returns true if refresh succeeded, false if it failed or wasn't possible.
    async fn try_refresh(&self) -> bool {
        match self.refresh_session().await {
            Ok(()) => {
                debug!("automatically refreshed expired session");
                true
            }
            Err(e) => {
                warn!(error = %e, "failed to auto-refresh session");
                false
            }
        }
    }

    /// Create a new record.
    pub async fn create_record<T: Serialize>(
        &self,
        collection: &str,
        rkey: Option<&str>,
        record: &T,
    ) -> Result<CreateRecordResponse, AtprotoError> {
        let did = self
            .did()
            .await
            .ok_or_else(|| AtprotoError::Auth("not authenticated".to_string()))?;

        #[derive(Serialize)]
        struct CreateRequest<'a, T> {
            repo: &'a str,
            collection: &'a str,
            #[serde(skip_serializing_if = "Option::is_none")]
            rkey: Option<&'a str>,
            record: &'a T,
        }

        let url = format!("{}/xrpc/com.atproto.repo.createRecord", self.pds_url);

        for attempt in 0..2 {
            let token = self.access_token().await?;

            let response = self
                .http
                .post(&url)
                .header("Authorization", format!("Bearer {}", token))
                .json(&CreateRequest {
                    repo: &did,
                    collection,
                    rkey,
                    record,
                })
                .send()
                .await?;

            let result = self.handle_response(response).await;

            // Retry once on expired token
            if attempt == 0 {
                if let Err(ref e) = result {
                    if Self::is_expired_token_error(e) && self.try_refresh().await {
                        continue;
                    }
                }
            }

            return result;
        }

        unreachable!()
    }

    /// Get a record by collection and rkey.
    pub async fn get_record<T: DeserializeOwned>(
        &self,
        collection: &str,
        rkey: &str,
    ) -> Result<GetRecordResponse<T>, AtprotoError> {
        let did = self
            .did()
            .await
            .ok_or_else(|| AtprotoError::Auth("not authenticated".to_string()))?;

        let url = format!("{}/xrpc/com.atproto.repo.getRecord", self.pds_url);

        for attempt in 0..2 {
            let token = self.access_token().await?;

            let response = self
                .http
                .get(&url)
                .header("Authorization", format!("Bearer {}", token))
                .query(&[
                    ("repo", &did),
                    ("collection", &collection.to_string()),
                    ("rkey", &rkey.to_string()),
                ])
                .send()
                .await?;

            if response.status() == reqwest::StatusCode::NOT_FOUND {
                return Err(AtprotoError::NotFound {
                    collection: collection.to_string(),
                    rkey: rkey.to_string(),
                });
            }

            let result = self.handle_response(response).await;

            // Retry once on expired token
            if attempt == 0 {
                if let Err(ref e) = result {
                    if Self::is_expired_token_error(e) && self.try_refresh().await {
                        continue;
                    }
                }
            }

            return result;
        }

        unreachable!()
    }

    /// List records in a collection.
    pub async fn list_records<T: DeserializeOwned>(
        &self,
        collection: &str,
        limit: Option<u32>,
        cursor: Option<&str>,
    ) -> Result<ListRecordsResponse<T>, AtprotoError> {
        let did = self
            .did()
            .await
            .ok_or_else(|| AtprotoError::Auth("not authenticated".to_string()))?;

        let url = format!("{}/xrpc/com.atproto.repo.listRecords", self.pds_url);

        for attempt in 0..2 {
            let token = self.access_token().await?;

            let mut query_params: Vec<(&str, String)> =
                vec![("repo", did.clone()), ("collection", collection.to_string())];
            if let Some(limit) = limit {
                query_params.push(("limit", limit.to_string()));
            }
            if let Some(cursor) = cursor {
                query_params.push(("cursor", cursor.to_string()));
            }

            let response = self
                .http
                .get(&url)
                .header("Authorization", format!("Bearer {}", token))
                .query(&query_params)
                .send()
                .await?;

            let result = self.handle_response(response).await;

            // Retry once on expired token
            if attempt == 0 {
                if let Err(ref e) = result {
                    if Self::is_expired_token_error(e) && self.try_refresh().await {
                        continue;
                    }
                }
            }

            return result;
        }

        unreachable!()
    }

    /// List all records in a collection (handles pagination).
    pub async fn list_all_records<T: DeserializeOwned>(
        &self,
        collection: &str,
    ) -> Result<Vec<ListRecordItem<T>>, AtprotoError> {
        let mut all_records = Vec::new();
        let mut cursor = None;

        loop {
            let response: ListRecordsResponse<T> = self
                .list_records(collection, Some(100), cursor.as_deref())
                .await?;

            all_records.extend(response.records);

            if response.cursor.is_none() {
                break;
            }
            cursor = response.cursor;
        }

        Ok(all_records)
    }

    /// Update (put) a record.
    pub async fn put_record<T: Serialize>(
        &self,
        collection: &str,
        rkey: &str,
        record: &T,
    ) -> Result<CreateRecordResponse, AtprotoError> {
        let did = self
            .did()
            .await
            .ok_or_else(|| AtprotoError::Auth("not authenticated".to_string()))?;

        #[derive(Serialize)]
        struct PutRequest<'a, T> {
            repo: &'a str,
            collection: &'a str,
            rkey: &'a str,
            record: &'a T,
        }

        let url = format!("{}/xrpc/com.atproto.repo.putRecord", self.pds_url);

        for attempt in 0..2 {
            let token = self.access_token().await?;

            let response = self
                .http
                .post(&url)
                .header("Authorization", format!("Bearer {}", token))
                .json(&PutRequest {
                    repo: &did,
                    collection,
                    rkey,
                    record,
                })
                .send()
                .await?;

            let result = self.handle_response(response).await;

            // Retry once on expired token
            if attempt == 0 {
                if let Err(ref e) = result {
                    if Self::is_expired_token_error(e) && self.try_refresh().await {
                        continue;
                    }
                }
            }

            return result;
        }

        unreachable!()
    }

    /// Get the PDS URL.
    pub fn pds_url(&self) -> &str {
        &self.pds_url
    }

    /// Get the full repository as a CAR file.
    ///
    /// Returns the CAR bytes and the repository revision from the response header.
    pub async fn get_repo(&self, did: &str) -> Result<(Vec<u8>, Option<String>), AtprotoError> {
        let url = format!("{}/xrpc/com.atproto.sync.getRepo", self.pds_url);

        for attempt in 0..2 {
            let token = self.access_token().await?;

            let response = self
                .http
                .get(&url)
                .header("Authorization", format!("Bearer {}", token))
                .query(&[("did", did)])
                .send()
                .await?;

            if response.status() == reqwest::StatusCode::TOO_MANY_REQUESTS {
                let retry_after_secs = response
                    .headers()
                    .get("Retry-After")
                    .and_then(|v| v.to_str().ok())
                    .and_then(|s| s.parse().ok());
                return Err(AtprotoError::RateLimited {
                    endpoint: Some("com.atproto.sync.getRepo".to_string()),
                    retry_after_secs,
                });
            }

            if !response.status().is_success() {
                let status = response.status();
                let text = response.text().await.map_err(|e| {
                    AtprotoError::InvalidResponse(format!(
                        "get_repo failed ({}): failed to read response: {}",
                        status, e
                    ))
                })?;

                // Check for expired token before returning error
                if let Ok(xrpc_error) = serde_json::from_str::<XrpcError>(&text) {
                    let err = AtprotoError::Xrpc {
                        error: xrpc_error.error.clone(),
                        message: xrpc_error.message,
                    };
                    if attempt == 0 && Self::is_expired_token_error(&err) && self.try_refresh().await
                    {
                        continue;
                    }
                    return Err(err);
                }

                return Err(AtprotoError::InvalidResponse(format!(
                    "get_repo failed ({}): {}",
                    status, text
                )));
            }

            // Extract the Atproto-Repo-Rev header
            let repo_rev = response
                .headers()
                .get("Atproto-Repo-Rev")
                .and_then(|v| v.to_str().ok())
                .map(String::from);

            let bytes = response.bytes().await?.to_vec();
            debug!(size = bytes.len(), rev = ?repo_rev, "fetched repo CAR");

            return Ok((bytes, repo_rev));
        }

        unreachable!()
    }

    /// Delete a record.
    pub async fn delete_record(&self, collection: &str, rkey: &str) -> Result<(), AtprotoError> {
        let did = self
            .did()
            .await
            .ok_or_else(|| AtprotoError::Auth("not authenticated".to_string()))?;

        #[derive(Serialize)]
        struct DeleteRequest<'a> {
            repo: &'a str,
            collection: &'a str,
            rkey: &'a str,
        }

        let url = format!("{}/xrpc/com.atproto.repo.deleteRecord", self.pds_url);

        for attempt in 0..2 {
            let token = self.access_token().await?;

            let response = self
                .http
                .post(&url)
                .header("Authorization", format!("Bearer {}", token))
                .json(&DeleteRequest {
                    repo: &did,
                    collection,
                    rkey,
                })
                .send()
                .await?;

            if !response.status().is_success() {
                let status = response.status();
                if status == reqwest::StatusCode::NOT_FOUND {
                    return Err(AtprotoError::NotFound {
                        collection: collection.to_string(),
                        rkey: rkey.to_string(),
                    });
                }
                let text = response.text().await.map_err(|e| {
                    AtprotoError::InvalidResponse(format!(
                        "delete failed ({}): failed to read response: {}",
                        status, e
                    ))
                })?;

                // Check for expired token before returning error
                if let Ok(xrpc_error) = serde_json::from_str::<XrpcError>(&text) {
                    let err = AtprotoError::Xrpc {
                        error: xrpc_error.error.clone(),
                        message: xrpc_error.message,
                    };
                    if attempt == 0 && Self::is_expired_token_error(&err) && self.try_refresh().await
                    {
                        continue;
                    }
                    return Err(err);
                }

                return Err(AtprotoError::InvalidResponse(format!(
                    "delete failed ({}): {}",
                    status, text
                )));
            }

            return Ok(());
        }

        unreachable!()
    }

    /// Handle HTTP response and parse JSON.
    async fn handle_response<T: DeserializeOwned>(
        &self,
        response: reqwest::Response,
    ) -> Result<T, AtprotoError> {
        let status = response.status();

        if status == reqwest::StatusCode::TOO_MANY_REQUESTS {
            let retry_after_secs = response
                .headers()
                .get("Retry-After")
                .and_then(|v| v.to_str().ok())
                .and_then(|s| s.parse().ok());
            return Err(AtprotoError::RateLimited {
                endpoint: None,
                retry_after_secs,
            });
        }

        if !status.is_success() {
            let text = response.text().await.map_err(|e| {
                AtprotoError::InvalidResponse(format!(
                    "request failed ({}): failed to read response: {}",
                    status, e
                ))
            })?;

            // Try to parse as XRPC error
            if let Ok(xrpc_error) = serde_json::from_str::<XrpcError>(&text) {
                return Err(AtprotoError::Xrpc {
                    error: xrpc_error.error,
                    message: xrpc_error.message,
                });
            }

            return Err(AtprotoError::InvalidResponse(format!(
                "request failed ({}): {}",
                status, text
            )));
        }

        let body = response.json().await?;
        Ok(body)
    }
}

/// XRPC error response format.
#[derive(Debug, serde::Deserialize)]
struct XrpcError {
    error: String,
    message: String,
}

#[cfg(test)]
mod tests {
    use super::*;
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    #[test]
    fn test_client_creation() {
        let client = AtprotoClient::new("https://example.com");
        assert_eq!(client.pds_url, "https://example.com");
    }

    #[test]
    fn test_client_pds_url() {
        let client = AtprotoClient::new("https://my-pds.example.com");
        assert_eq!(client.pds_url(), "https://my-pds.example.com");
    }

    #[tokio::test]
    async fn test_client_did_without_session() {
        let client = AtprotoClient::new("https://example.com");
        assert_eq!(client.did().await, None);
    }

    #[tokio::test]
    async fn test_login_success() {
        let mock_server = MockServer::start().await;

        Mock::given(method("POST"))
            .and(path("/xrpc/com.atproto.server.createSession"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "did": "did:plc:testuser123",
                "handle": "test.example.com",
                "accessJwt": "test-access-token",
                "refreshJwt": "test-refresh-token"
            })))
            .mount(&mock_server)
            .await;

        let client = AtprotoClient::new(mock_server.uri());
        let result = client.login("test.example.com", "password123").await;

        assert!(result.is_ok());
        assert_eq!(client.did().await, Some("did:plc:testuser123".to_string()));
    }

    #[tokio::test]
    async fn test_login_failure() {
        let mock_server = MockServer::start().await;

        Mock::given(method("POST"))
            .and(path("/xrpc/com.atproto.server.createSession"))
            .respond_with(ResponseTemplate::new(401).set_body_json(serde_json::json!({
                "error": "AuthenticationRequired",
                "message": "Invalid credentials"
            })))
            .mount(&mock_server)
            .await;

        let client = AtprotoClient::new(mock_server.uri());
        let result = client.login("test.example.com", "wrong-password").await;

        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(matches!(err, AtprotoError::Auth(_)));
    }

    #[tokio::test]
    async fn test_access_token_without_session() {
        let client = AtprotoClient::new("https://example.com");
        let result = client.access_token().await;

        assert!(result.is_err());
        assert!(matches!(result.unwrap_err(), AtprotoError::Auth(_)));
    }

    #[tokio::test]
    async fn test_refresh_session_without_session() {
        let client = AtprotoClient::new("https://example.com");
        let result = client.refresh_session().await;

        assert!(result.is_err());
        assert!(matches!(result.unwrap_err(), AtprotoError::Auth(_)));
    }

    #[tokio::test]
    async fn test_get_record_not_found() {
        let mock_server = MockServer::start().await;

        // First, login to get a session
        Mock::given(method("POST"))
            .and(path("/xrpc/com.atproto.server.createSession"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "did": "did:plc:testuser123",
                "handle": "test.example.com",
                "accessJwt": "test-access-token",
                "refreshJwt": "test-refresh-token"
            })))
            .mount(&mock_server)
            .await;

        Mock::given(method("GET"))
            .and(path("/xrpc/com.atproto.repo.getRecord"))
            .respond_with(ResponseTemplate::new(404))
            .mount(&mock_server)
            .await;

        let client = AtprotoClient::new(mock_server.uri());
        client.login("test.example.com", "password").await.unwrap();

        let result = client
            .get_record::<serde_json::Value>("test.collection", "nonexistent")
            .await;

        assert!(result.is_err());
        assert!(matches!(result.unwrap_err(), AtprotoError::NotFound { .. }));
    }

    #[tokio::test]
    async fn test_rate_limited() {
        let mock_server = MockServer::start().await;

        // Login first
        Mock::given(method("POST"))
            .and(path("/xrpc/com.atproto.server.createSession"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "did": "did:plc:testuser123",
                "handle": "test.example.com",
                "accessJwt": "test-access-token",
                "refreshJwt": "test-refresh-token"
            })))
            .mount(&mock_server)
            .await;

        Mock::given(method("GET"))
            .and(path("/xrpc/com.atproto.repo.listRecords"))
            .respond_with(ResponseTemplate::new(429))
            .mount(&mock_server)
            .await;

        let client = AtprotoClient::new(mock_server.uri());
        client.login("test.example.com", "password").await.unwrap();

        let result = client
            .list_records::<serde_json::Value>("test.collection", None, None)
            .await;

        assert!(result.is_err());
        assert!(matches!(
            result.unwrap_err(),
            AtprotoError::RateLimited { .. }
        ));
    }
}
