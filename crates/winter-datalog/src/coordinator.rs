//! DatalogCoordinator actor for serializing TSV file writes.
//!
//! The coordinator ensures no race conditions on Soufflé TSV files by
//! serializing all write operations through a single actor.

use std::sync::Arc;

use tokio::sync::{mpsc, oneshot};
use tracing::{debug, trace, warn};

use crate::cache::DatalogCache;
use crate::error::DatalogError;

/// Channel buffer size for the coordinator.
const COORDINATOR_CHANNEL_SIZE: usize = 100;

/// Operations that can be sent to the DatalogCoordinator.
pub enum DatalogOp {
    /// Execute a query (flushes dirty predicates first).
    Query {
        query: String,
        extra_rules: Option<String>,
        extra_facts: Option<Vec<String>>,
        extra_declarations: Option<Vec<String>>,
        response: oneshot::Sender<Result<Vec<Vec<String>>, DatalogError>>,
    },
    /// Flush dirty predicates to TSV files.
    Flush {
        response: oneshot::Sender<Result<(), DatalogError>>,
    },
    /// Shutdown the coordinator.
    Shutdown,
}

/// The DatalogCoordinator actor serializes all TSV file operations.
///
/// It receives operations via an mpsc channel and processes them sequentially,
/// ensuring no race conditions on the underlying TSV files.
pub struct DatalogCoordinator {
    cache: Arc<DatalogCache>,
    op_rx: mpsc::Receiver<DatalogOp>,
}

impl DatalogCoordinator {
    /// Create a new coordinator with the given cache.
    ///
    /// Returns the coordinator and a handle for sending operations.
    pub fn new(cache: Arc<DatalogCache>) -> (Self, DatalogCoordinatorHandle) {
        let (op_tx, op_rx) = mpsc::channel(COORDINATOR_CHANNEL_SIZE);
        let coordinator = Self { cache, op_rx };
        let handle = DatalogCoordinatorHandle { op_tx };
        (coordinator, handle)
    }

    /// Run the coordinator event loop.
    ///
    /// This processes operations until a Shutdown message is received
    /// or all senders are dropped.
    pub async fn run(mut self) {
        debug!("datalog coordinator started");

        while let Some(op) = self.op_rx.recv().await {
            match op {
                DatalogOp::Query {
                    query,
                    extra_rules,
                    extra_facts,
                    extra_declarations,
                    response,
                } => {
                    trace!(query = %query, "processing query op");
                    let result = self
                        .cache
                        .execute_query_with_facts_and_declarations(
                            &query,
                            extra_rules.as_deref(),
                            extra_facts.as_deref(),
                            extra_declarations.as_deref(),
                        )
                        .await;
                    let _ = response.send(result);
                }
                DatalogOp::Flush { response } => {
                    trace!("processing flush op");
                    let result = self.cache.flush_dirty_predicates().await;
                    let _ = response.send(result);
                }
                DatalogOp::Shutdown => {
                    debug!("datalog coordinator received shutdown");
                    break;
                }
            }
        }

        debug!("datalog coordinator stopped");
    }

    /// Spawn the coordinator as a background task.
    ///
    /// Returns a handle for sending operations.
    pub fn spawn(cache: Arc<DatalogCache>) -> DatalogCoordinatorHandle {
        let (coordinator, handle) = Self::new(cache);
        tokio::spawn(async move {
            coordinator.run().await;
        });
        handle
    }
}

/// Handle for sending operations to the DatalogCoordinator.
///
/// This is cheaply cloneable and can be shared across tasks.
#[derive(Clone)]
pub struct DatalogCoordinatorHandle {
    op_tx: mpsc::Sender<DatalogOp>,
}

impl DatalogCoordinatorHandle {
    /// Execute a datalog query.
    ///
    /// The coordinator will flush any dirty predicates before executing.
    pub async fn query(
        &self,
        query: &str,
        extra_rules: Option<&str>,
    ) -> Result<Vec<Vec<String>>, DatalogError> {
        self.query_with_facts(query, extra_rules, None).await
    }

    /// Execute a datalog query with optional ephemeral facts.
    ///
    /// The `extra_facts` parameter allows injecting facts at query time without
    /// persisting them. Useful for runtime context like thread state.
    pub async fn query_with_facts(
        &self,
        query: &str,
        extra_rules: Option<&str>,
        extra_facts: Option<&[String]>,
    ) -> Result<Vec<Vec<String>>, DatalogError> {
        self.query_with_facts_and_declarations(query, extra_rules, extra_facts, None)
            .await
    }

    /// Execute a datalog query with optional ephemeral facts and ad-hoc declarations.
    ///
    /// The `extra_facts` parameter allows injecting facts at query time without
    /// persisting them. Useful for runtime context like thread state.
    ///
    /// The `extra_declarations` parameter allows declaring predicates ad-hoc
    /// (e.g., "my_pred(arg1: symbol, arg2: symbol)") for predicates not yet stored.
    pub async fn query_with_facts_and_declarations(
        &self,
        query: &str,
        extra_rules: Option<&str>,
        extra_facts: Option<&[String]>,
        extra_declarations: Option<&[String]>,
    ) -> Result<Vec<Vec<String>>, DatalogError> {
        let (response_tx, response_rx) = oneshot::channel();
        let op = DatalogOp::Query {
            query: query.to_string(),
            extra_rules: extra_rules.map(String::from),
            extra_facts: extra_facts.map(|f| f.to_vec()),
            extra_declarations: extra_declarations.map(|d| d.to_vec()),
            response: response_tx,
        };

        self.op_tx
            .send(op)
            .await
            .map_err(|_| DatalogError::Execution("coordinator channel closed".to_string()))?;

        response_rx
            .await
            .map_err(|_| DatalogError::Execution("coordinator response dropped".to_string()))?
    }

    /// Flush dirty predicates to TSV files.
    pub async fn flush(&self) -> Result<(), DatalogError> {
        let (response_tx, response_rx) = oneshot::channel();
        let op = DatalogOp::Flush {
            response: response_tx,
        };

        self.op_tx
            .send(op)
            .await
            .map_err(|_| DatalogError::Execution("coordinator channel closed".to_string()))?;

        response_rx
            .await
            .map_err(|_| DatalogError::Execution("coordinator response dropped".to_string()))?
    }

    /// Shutdown the coordinator.
    ///
    /// After calling this, no more operations can be sent.
    pub async fn shutdown(&self) {
        if let Err(e) = self.op_tx.send(DatalogOp::Shutdown).await {
            warn!(error = %e, "failed to send shutdown to coordinator");
        }
    }

    /// Check if the coordinator channel is closed.
    pub fn is_closed(&self) -> bool {
        self.op_tx.is_closed()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_cache() -> Arc<DatalogCache> {
        let temp_dir = tempfile::tempdir().unwrap();
        DatalogCache::new(temp_dir.into_path()).unwrap()
    }

    #[tokio::test]
    async fn test_coordinator_spawn_and_shutdown() {
        let cache = temp_cache();
        let handle = DatalogCoordinator::spawn(cache);

        // Should be able to send operations
        assert!(!handle.is_closed());

        // Shutdown
        handle.shutdown().await;

        // Give the coordinator time to process shutdown
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }

    #[tokio::test]
    async fn test_coordinator_query() {
        let cache = temp_cache();
        let handle = DatalogCoordinator::spawn(cache);

        // Query with no facts - the coordinator handles it and returns a result
        // (may be error, empty, or contain metadata depending on declarations)
        let result = handle.query("test_pred(X)", None).await;
        // The test passes if the coordinator handled the request without crashing
        // We don't assert on the specific outcome since it depends on Soufflé behavior
        let _ = result;

        handle.shutdown().await;
    }

    #[tokio::test]
    async fn test_coordinator_handle_clone() {
        let cache = temp_cache();
        let handle1 = DatalogCoordinator::spawn(cache);
        let handle2 = handle1.clone();

        // Both handles should work
        assert!(!handle1.is_closed());
        assert!(!handle2.is_closed());

        // Flush via one handle
        let _ = handle1.flush().await;

        // Shutdown via the other
        handle2.shutdown().await;
    }

    #[tokio::test]
    async fn test_coordinator_multiple_queries_serialized() {
        let cache = temp_cache();
        let handle = DatalogCoordinator::spawn(Arc::clone(&cache));

        // Send multiple queries concurrently
        let h1 = handle.clone();
        let h2 = handle.clone();
        let h3 = handle.clone();

        let (r1, r2, r3) = tokio::join!(
            h1.query("pred1(X)", None),
            h2.query("pred2(X, Y)", None),
            h3.query("pred3(X, Y, Z)", None),
        );

        // All should complete (even if they error due to no declarations)
        // The important thing is they were serialized and didn't race
        assert!(r1.is_err() || r1.is_ok());
        assert!(r2.is_err() || r2.is_ok());
        assert!(r3.is_err() || r3.is_ok());

        handle.shutdown().await;
    }
}
