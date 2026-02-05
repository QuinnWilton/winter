//! ATProto XRPC client implementation.

use std::sync::Arc;
use std::time::Duration;

use reqwest::Client;
use serde::{Serialize, de::DeserializeOwned};
use tokio::sync::RwLock;
use tracing::{debug, warn};

use serde::Deserialize;

use crate::{
    AtprotoError, CreateRecordResponse, GetRecordResponse, ListRecordItem, ListRecordsResponse,
    Session,
};

/// A single write operation for batch writes via `com.atproto.repo.applyWrites`.
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "$type")]
pub enum WriteOp {
    #[serde(rename = "com.atproto.repo.applyWrites#create")]
    Create {
        collection: String,
        rkey: String,
        value: serde_json::Value,
    },
    #[serde(rename = "com.atproto.repo.applyWrites#update")]
    Update {
        collection: String,
        rkey: String,
        value: serde_json::Value,
    },
    #[serde(rename = "com.atproto.repo.applyWrites#delete")]
    Delete { collection: String, rkey: String },
}

/// Response from `com.atproto.repo.applyWrites`.
#[derive(Debug, Deserialize)]
pub struct ApplyWritesResponse {
    pub commit: CommitInfo,
    pub results: Vec<WriteResult>,
}

/// Commit info returned from batch writes.
#[derive(Debug, Deserialize)]
pub struct CommitInfo {
    pub cid: String,
    pub rev: String,
}

/// Result of a single write operation in a batch.
#[derive(Debug, Deserialize)]
#[serde(tag = "$type")]
pub enum WriteResult {
    #[serde(rename = "com.atproto.repo.applyWrites#createResult")]
    Create { uri: String, cid: String },
    #[serde(rename = "com.atproto.repo.applyWrites#updateResult")]
    Update { uri: String, cid: String },
    #[serde(rename = "com.atproto.repo.applyWrites#deleteResult")]
    Delete {},
}

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

    /// Check if an error is transient and worth retrying.
    fn is_transient_error(err: &AtprotoError) -> bool {
        match err {
            AtprotoError::Xrpc { error, .. } => {
                // Upstream errors from PDS
                error == "UpstreamFailure"
                    || error == "UpstreamTimeout"
                    || error == "InternalServerError"
                    || error == "ServiceUnavailable"
            }
            AtprotoError::Network(_) => true,
            _ => false,
        }
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

        // Serialize record to JSON and add $type field
        // ATProto records must include $type for proper validation
        let mut record_value = serde_json::to_value(record)?;
        if let serde_json::Value::Object(ref mut map) = record_value {
            map.insert(
                "$type".to_string(),
                serde_json::Value::String(collection.to_string()),
            );
        }

        #[derive(Serialize)]
        struct CreateRequest<'a> {
            repo: &'a str,
            collection: &'a str,
            #[serde(skip_serializing_if = "Option::is_none")]
            rkey: Option<&'a str>,
            record: serde_json::Value,
        }

        let url = format!("{}/xrpc/com.atproto.repo.createRecord", self.pds_url);

        let request_body = CreateRequest {
            repo: &did,
            collection,
            rkey,
            record: record_value,
        };

        // Debug log the request body for troubleshooting
        if let Ok(json) = serde_json::to_string(&request_body) {
            debug!(collection = %collection, body = %json, "creating record");
        }

        // Retry up to 4 times: initial + 3 retries with backoff
        let mut last_error = None;
        for attempt in 0..4 {
            let token = self.access_token().await?;

            let response = self
                .http
                .post(&url)
                .header("Authorization", format!("Bearer {}", token))
                .json(&request_body)
                .send()
                .await?;

            let result = self.handle_response(response).await;

            match result {
                Ok(v) => return Ok(v),
                Err(ref e) if Self::is_expired_token_error(e) => {
                    if self.try_refresh().await {
                        continue;
                    }
                    return result;
                }
                Err(ref e) if Self::is_transient_error(e) && attempt < 3 => {
                    let backoff_ms = 500 * (1 << attempt); // 500ms, 1s, 2s
                    warn!(
                        attempt = attempt + 1,
                        backoff_ms,
                        error = %e,
                        "transient error in create_record, retrying"
                    );
                    tokio::time::sleep(Duration::from_millis(backoff_ms)).await;
                    last_error = Some(result);
                    continue;
                }
                Err(_) => return result,
            }
        }

        last_error.unwrap_or_else(|| Err(AtprotoError::InvalidResponse("retry exhausted".into())))
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

        // Retry up to 4 times: initial + 3 retries with backoff
        let mut last_error = None;
        for attempt in 0..4 {
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

            match result {
                Ok(v) => return Ok(v),
                Err(ref e) if Self::is_expired_token_error(e) => {
                    if self.try_refresh().await {
                        continue;
                    }
                    return result;
                }
                Err(ref e) if Self::is_transient_error(e) && attempt < 3 => {
                    let backoff_ms = 500 * (1 << attempt); // 500ms, 1s, 2s
                    debug!(
                        attempt = attempt + 1,
                        backoff_ms,
                        error = %e,
                        "transient error in get_record, retrying"
                    );
                    tokio::time::sleep(Duration::from_millis(backoff_ms)).await;
                    last_error = Some(result);
                    continue;
                }
                Err(_) => return result,
            }
        }

        last_error.unwrap_or_else(|| Err(AtprotoError::InvalidResponse("retry exhausted".into())))
    }

    /// Get multiple records by their AT URIs.
    ///
    /// Uses `com.atproto.repo.getRecords` to fetch multiple records in a single request.
    /// Returns results for all requested URIs; missing records have `value: None`.
    pub async fn get_records<T: DeserializeOwned>(
        &self,
        uris: &[&str],
    ) -> Result<crate::GetRecordsResponse<T>, AtprotoError> {
        if uris.is_empty() {
            return Ok(crate::GetRecordsResponse { records: vec![] });
        }

        let url = format!("{}/xrpc/com.atproto.repo.getRecords", self.pds_url);

        // Retry up to 4 times: initial + 3 retries with backoff
        let mut last_error = None;
        for attempt in 0..4 {
            let token = self.access_token().await?;

            // Build query parameters - multiple uris= params
            let query_params: Vec<(&str, &str)> = uris.iter().map(|u| ("uris", *u)).collect();

            let response = self
                .http
                .get(&url)
                .header("Authorization", format!("Bearer {}", token))
                .query(&query_params)
                .send()
                .await?;

            let result = self.handle_response(response).await;

            match result {
                Ok(v) => return Ok(v),
                Err(ref e) if Self::is_expired_token_error(e) => {
                    if self.try_refresh().await {
                        continue;
                    }
                    return result;
                }
                Err(ref e) if Self::is_transient_error(e) && attempt < 3 => {
                    let backoff_ms = 500 * (1 << attempt); // 500ms, 1s, 2s
                    debug!(
                        attempt = attempt + 1,
                        backoff_ms,
                        error = %e,
                        "transient error in get_records, retrying"
                    );
                    tokio::time::sleep(Duration::from_millis(backoff_ms)).await;
                    last_error = Some(result);
                    continue;
                }
                Err(_) => return result,
            }
        }

        last_error.unwrap_or_else(|| Err(AtprotoError::InvalidResponse("retry exhausted".into())))
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

        // Retry up to 4 times: initial + 3 retries with backoff
        let mut last_error = None;
        for attempt in 0..4 {
            let token = self.access_token().await?;

            let mut query_params: Vec<(&str, String)> = vec![
                ("repo", did.clone()),
                ("collection", collection.to_string()),
            ];
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

            match result {
                Ok(v) => return Ok(v),
                Err(ref e) if Self::is_expired_token_error(e) => {
                    if self.try_refresh().await {
                        continue;
                    }
                    return result;
                }
                Err(ref e) if Self::is_transient_error(e) && attempt < 3 => {
                    let backoff_ms = 500 * (1 << attempt); // 500ms, 1s, 2s
                    debug!(
                        attempt = attempt + 1,
                        backoff_ms,
                        error = %e,
                        "transient error in list_records, retrying"
                    );
                    tokio::time::sleep(Duration::from_millis(backoff_ms)).await;
                    last_error = Some(result);
                    continue;
                }
                Err(_) => return result,
            }
        }

        last_error.unwrap_or_else(|| Err(AtprotoError::InvalidResponse("retry exhausted".into())))
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

        // Serialize record to JSON and add $type field
        // ATProto records must include $type for proper validation
        let mut record_value = serde_json::to_value(record)?;
        if let serde_json::Value::Object(ref mut map) = record_value {
            map.insert(
                "$type".to_string(),
                serde_json::Value::String(collection.to_string()),
            );
        }

        #[derive(Serialize)]
        struct PutRequest<'a> {
            repo: &'a str,
            collection: &'a str,
            rkey: &'a str,
            record: serde_json::Value,
        }

        let url = format!("{}/xrpc/com.atproto.repo.putRecord", self.pds_url);

        // Retry up to 4 times: initial + 3 retries with backoff
        let mut last_error = None;
        for attempt in 0..4 {
            let token = self.access_token().await?;

            let response = self
                .http
                .post(&url)
                .header("Authorization", format!("Bearer {}", token))
                .json(&PutRequest {
                    repo: &did,
                    collection,
                    rkey,
                    record: record_value.clone(),
                })
                .send()
                .await?;

            let result = self.handle_response(response).await;

            match result {
                Ok(v) => return Ok(v),
                Err(ref e) if Self::is_expired_token_error(e) => {
                    if self.try_refresh().await {
                        continue;
                    }
                    return result;
                }
                Err(ref e) if Self::is_transient_error(e) && attempt < 3 => {
                    let backoff_ms = 500 * (1 << attempt); // 500ms, 1s, 2s
                    warn!(
                        attempt = attempt + 1,
                        backoff_ms,
                        error = %e,
                        "transient error in put_record, retrying"
                    );
                    tokio::time::sleep(Duration::from_millis(backoff_ms)).await;
                    last_error = Some(result);
                    continue;
                }
                Err(_) => return result,
            }
        }

        last_error.unwrap_or_else(|| Err(AtprotoError::InvalidResponse("retry exhausted".into())))
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

            // Use a longer timeout for CAR downloads - repos can be large
            let response = self
                .http
                .get(&url)
                .header("Authorization", format!("Bearer {}", token))
                .query(&[("did", did)])
                .timeout(Duration::from_secs(120))
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
                    if attempt == 0
                        && Self::is_expired_token_error(&err)
                        && self.try_refresh().await
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

        // Retry up to 4 times: initial + 3 retries with backoff
        let mut last_error: Option<AtprotoError> = None;
        for attempt in 0..4 {
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

            if response.status().is_success() {
                return Ok(());
            }

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

            // Check for XRPC error
            if let Ok(xrpc_error) = serde_json::from_str::<XrpcError>(&text) {
                let err = AtprotoError::Xrpc {
                    error: xrpc_error.error.clone(),
                    message: xrpc_error.message,
                };

                if Self::is_expired_token_error(&err) && self.try_refresh().await {
                    continue;
                }

                if Self::is_transient_error(&err) && attempt < 3 {
                    let backoff_ms = 500 * (1 << attempt); // 500ms, 1s, 2s
                    warn!(
                        attempt = attempt + 1,
                        backoff_ms,
                        error = %err,
                        "transient error in delete_record, retrying"
                    );
                    tokio::time::sleep(Duration::from_millis(backoff_ms)).await;
                    last_error = Some(err);
                    continue;
                }

                return Err(err);
            }

            return Err(AtprotoError::InvalidResponse(format!(
                "delete failed ({}): {}",
                status, text
            )));
        }

        Err(last_error.unwrap_or_else(|| AtprotoError::InvalidResponse("retry exhausted".into())))
    }

    /// Apply multiple write operations atomically.
    ///
    /// This uses `com.atproto.repo.applyWrites` to batch multiple create, update,
    /// or delete operations into a single transaction.
    pub async fn apply_writes(
        &self,
        writes: Vec<WriteOp>,
    ) -> Result<ApplyWritesResponse, AtprotoError> {
        if writes.is_empty() {
            return Err(AtprotoError::InvalidResponse(
                "apply_writes requires at least one write operation".to_string(),
            ));
        }

        let did = self
            .did()
            .await
            .ok_or_else(|| AtprotoError::Auth("not authenticated".to_string()))?;

        // Prepare writes with $type field in values
        let prepared_writes: Vec<serde_json::Value> = writes
            .into_iter()
            .map(|op| {
                match op {
                    WriteOp::Create {
                        collection,
                        rkey,
                        mut value,
                    } => {
                        // Add $type to the value if it's an object
                        if let serde_json::Value::Object(ref mut map) = value {
                            map.insert(
                                "$type".to_string(),
                                serde_json::Value::String(collection.clone()),
                            );
                        }
                        serde_json::json!({
                            "$type": "com.atproto.repo.applyWrites#create",
                            "collection": collection,
                            "rkey": rkey,
                            "value": value
                        })
                    }
                    WriteOp::Update {
                        collection,
                        rkey,
                        mut value,
                    } => {
                        if let serde_json::Value::Object(ref mut map) = value {
                            map.insert(
                                "$type".to_string(),
                                serde_json::Value::String(collection.clone()),
                            );
                        }
                        serde_json::json!({
                            "$type": "com.atproto.repo.applyWrites#update",
                            "collection": collection,
                            "rkey": rkey,
                            "value": value
                        })
                    }
                    WriteOp::Delete { collection, rkey } => {
                        serde_json::json!({
                            "$type": "com.atproto.repo.applyWrites#delete",
                            "collection": collection,
                            "rkey": rkey
                        })
                    }
                }
            })
            .collect();

        #[derive(Serialize)]
        struct ApplyWritesRequest {
            repo: String,
            writes: Vec<serde_json::Value>,
        }

        let url = format!("{}/xrpc/com.atproto.repo.applyWrites", self.pds_url);

        let request_body = ApplyWritesRequest {
            repo: did,
            writes: prepared_writes,
        };

        debug!(count = request_body.writes.len(), "applying batch writes");

        // Retry up to 4 times: initial + 3 retries with backoff
        let mut last_error = None;
        for attempt in 0..4 {
            let token = self.access_token().await?;

            let response = self
                .http
                .post(&url)
                .header("Authorization", format!("Bearer {}", token))
                .json(&request_body)
                .send()
                .await?;

            let result = self.handle_response(response).await;

            match result {
                Ok(v) => return Ok(v),
                Err(ref e) if Self::is_expired_token_error(e) => {
                    if self.try_refresh().await {
                        continue;
                    }
                    return result;
                }
                Err(ref e) if Self::is_transient_error(e) && attempt < 3 => {
                    let backoff_ms = 500 * (1 << attempt); // 500ms, 1s, 2s
                    warn!(
                        attempt = attempt + 1,
                        backoff_ms,
                        error = %e,
                        "transient error in apply_writes, retrying"
                    );
                    tokio::time::sleep(Duration::from_millis(backoff_ms)).await;
                    last_error = Some(result);
                    continue;
                }
                Err(_) => return result,
            }
        }

        last_error.unwrap_or_else(|| Err(AtprotoError::InvalidResponse("retry exhausted".into())))
    }

    /// Upload a blob to the PDS.
    ///
    /// Returns the blob reference JSON containing `$type`, `ref.$link`, `mimeType`, and `size`.
    pub async fn upload_blob(
        &self,
        data: &[u8],
        mime_type: &str,
    ) -> Result<serde_json::Value, AtprotoError> {
        // Validate MIME type
        const ALLOWED_MIME_TYPES: &[&str] = &["image/jpeg", "image/png", "image/webp", "image/gif"];
        if !ALLOWED_MIME_TYPES.contains(&mime_type) {
            return Err(AtprotoError::InvalidMimeType(mime_type.to_string()));
        }

        // Validate size (max 1MB)
        const MAX_BLOB_SIZE: usize = 1_000_000;
        if data.len() > MAX_BLOB_SIZE {
            return Err(AtprotoError::BlobTooLarge {
                size: data.len(),
                max: MAX_BLOB_SIZE,
            });
        }

        let url = format!("{}/xrpc/com.atproto.repo.uploadBlob", self.pds_url);

        // Retry up to 4 times: initial + 3 retries with backoff
        let mut last_error = None;
        for attempt in 0..4 {
            let token = self.access_token().await?;

            let response = self
                .http
                .post(&url)
                .header("Authorization", format!("Bearer {}", token))
                .header("Content-Type", mime_type)
                .body(data.to_vec())
                .send()
                .await?;

            let result = self.handle_response::<UploadBlobResponse>(response).await;

            match result {
                Ok(v) => {
                    debug!(size = data.len(), mime_type = %mime_type, "uploaded blob");
                    return Ok(v.blob);
                }
                Err(ref e) if Self::is_expired_token_error(e) => {
                    if self.try_refresh().await {
                        continue;
                    }
                    return Err(result.unwrap_err());
                }
                Err(ref e) if Self::is_transient_error(e) && attempt < 3 => {
                    let backoff_ms = 500 * (1 << attempt); // 500ms, 1s, 2s
                    warn!(
                        attempt = attempt + 1,
                        backoff_ms,
                        error = %e,
                        "transient error in upload_blob, retrying"
                    );
                    tokio::time::sleep(Duration::from_millis(backoff_ms)).await;
                    last_error = Some(result);
                    continue;
                }
                Err(_) => return Err(result.unwrap_err()),
            }
        }

        Err(last_error
            .unwrap_or_else(|| Err(AtprotoError::InvalidResponse("retry exhausted".into())))
            .unwrap_err())
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

/// Response from `com.atproto.repo.uploadBlob`.
#[derive(Debug, serde::Deserialize)]
struct UploadBlobResponse {
    blob: serde_json::Value,
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
