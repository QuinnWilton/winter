//! Execute Soufflé queries.

use std::path::Path;
use std::time::Duration;

use tokio::process::Command;
use tracing::{debug, warn};

use crate::DatalogError;

/// Executes Soufflé datalog queries.
pub struct SouffleExecutor {
    /// Timeout for query execution.
    timeout: Duration,
}

impl SouffleExecutor {
    /// Create a new executor with default timeout (30 seconds).
    pub fn new() -> Self {
        Self {
            timeout: Duration::from_secs(30),
        }
    }

    /// Create a new executor with custom timeout.
    pub fn with_timeout(timeout: Duration) -> Self {
        Self { timeout }
    }

    /// Execute a Soufflé program and return the output.
    pub async fn execute(&self, program: &str, fact_dir: &Path) -> Result<String, DatalogError> {
        // Check if Soufflé is available
        if !Self::is_souffle_available().await {
            return Err(DatalogError::SouffleNotFound);
        }

        // Write program to temp file
        let temp_dir = tempfile::tempdir()?;
        let program_path = temp_dir.path().join("query.dl");
        tokio::fs::write(&program_path, program).await?;

        debug!(
            program_path = %program_path.display(),
            fact_dir = %fact_dir.display(),
            "executing Soufflé query"
        );

        // Execute Soufflé
        let result = tokio::time::timeout(self.timeout, async {
            Command::new("souffle")
                .arg("-F")
                .arg(fact_dir)
                .arg("-D-") // Output to stdout
                .arg(&program_path)
                .output()
                .await
        })
        .await;

        match result {
            Ok(Ok(output)) => {
                let stderr = String::from_utf8_lossy(&output.stderr);
                if !output.status.success() {
                    warn!(stderr = %stderr, "Soufflé execution failed");
                    return Err(DatalogError::Execution(stderr.to_string()));
                }

                // Log stderr warnings even on success
                if !stderr.is_empty() {
                    debug!(stderr = %stderr, "Soufflé stderr (non-fatal)");
                }

                let stdout = String::from_utf8_lossy(&output.stdout);
                debug!(output_len = stdout.len(), "Soufflé query completed");
                Ok(stdout.to_string())
            }
            Ok(Err(e)) => Err(DatalogError::Execution(e.to_string())),
            Err(_) => Err(DatalogError::Timeout(self.timeout.as_millis() as u64)),
        }
    }

    /// Check if Soufflé is available in PATH.
    async fn is_souffle_available() -> bool {
        Command::new("which")
            .arg("souffle")
            .output()
            .await
            .map(|o| o.status.success())
            .unwrap_or(false)
    }

    /// Parse Soufflé output into tuples.
    ///
    /// Soufflé's `-D-` output format includes relation name headers:
    /// ```text
    /// ---------------
    /// relationName
    /// col1
    /// col2
    /// ===============
    /// tuple1_col1\ttuple1_col2
    /// ===============
    /// ```
    /// This function filters out the separator lines, relation name, and column headers,
    /// returning only the actual tuple data.
    ///
    /// Also handles raw TSV data without headers (for backwards compatibility).
    pub fn parse_output(output: &str) -> Vec<Vec<String>> {
        let mut results = Vec::new();

        // State machine for parsing Soufflé output:
        // - InData: collecting data tuples (initial state for raw TSV)
        // - InHeader: skipping relation name and column headers until === separator
        enum State {
            InData,
            InHeader,
        }

        let mut state = State::InData;

        for line in output.lines() {
            let line = line.trim();

            // Skip empty lines and comments
            if line.is_empty() || line.starts_with("//") {
                continue;
            }

            // Check for separator lines (--- or ===)
            let is_dash_separator = line.chars().all(|c| c == '-') && line.len() >= 3;
            let is_equals_separator = line.chars().all(|c| c == '=') && line.len() >= 3;

            if is_dash_separator {
                // Start of a new relation header section
                state = State::InHeader;
                continue;
            }

            if is_equals_separator {
                state = match state {
                    State::InHeader => State::InData, // End of header, data follows
                    State::InData => State::InHeader, // Start of next relation's header
                };
                continue;
            }

            match state {
                State::InHeader => {
                    // Skip relation name and column headers
                }
                State::InData => {
                    // Soufflé outputs tab-separated values
                    let tuple: Vec<String> = line.split('\t').map(String::from).collect();
                    if !tuple.is_empty() && !tuple[0].is_empty() {
                        results.push(tuple);
                    }
                }
            }
        }

        results
    }
}

impl Default for SouffleExecutor {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_output() {
        let output = "did:a\tdid:b\nDid:b\tdid:c\n";
        let results = SouffleExecutor::parse_output(output);

        assert_eq!(results.len(), 2);
        assert_eq!(results[0], vec!["did:a", "did:b"]);
        assert_eq!(results[1], vec!["Did:b", "did:c"]);
    }

    #[test]
    fn test_parse_output_with_headers() {
        // Soufflé -D- output includes relation name headers between === separators
        let output = "===============\nresult\n===============\ndid:a\n";
        let results = SouffleExecutor::parse_output(output);

        // Should only return actual data, not the relation name header
        assert_eq!(results.len(), 1);
        assert_eq!(results[0], vec!["did:a"]);
    }

    #[test]
    fn test_parse_output_multiple_relations() {
        // Multiple relations in output
        let output = "\
===============
follows
===============
did:a\tdid:b
did:c\tdid:d
===============
likes
===============
did:a\tpost:1
";
        let results = SouffleExecutor::parse_output(output);

        assert_eq!(results.len(), 3);
        assert_eq!(results[0], vec!["did:a", "did:b"]);
        assert_eq!(results[1], vec!["did:c", "did:d"]);
        assert_eq!(results[2], vec!["did:a", "post:1"]);
    }

    #[test]
    fn test_parse_output_empty() {
        let output = "";
        let results = SouffleExecutor::parse_output(output);
        assert!(results.is_empty());
    }

    #[test]
    fn test_parse_output_only_headers() {
        // Relation with no data
        let output = "===============\nempty_relation\n===============\n";
        let results = SouffleExecutor::parse_output(output);
        assert!(results.is_empty());
    }

    #[test]
    fn test_parse_output_comments_ignored() {
        let output = "// comment\ndid:a\tdid:b\n// another comment\n";
        let results = SouffleExecutor::parse_output(output);

        assert_eq!(results.len(), 1);
        assert_eq!(results[0], vec!["did:a", "did:b"]);
    }

    #[test]
    fn test_parse_output_whitespace_handling() {
        let output = "  did:a\tdid:b  \n\n  \ndid:c\tdid:d\n";
        let results = SouffleExecutor::parse_output(output);

        assert_eq!(results.len(), 2);
        // Trim is applied to the line, so leading/trailing spaces are removed
        assert_eq!(results[0], vec!["did:a", "did:b"]);
        assert_eq!(results[1], vec!["did:c", "did:d"]);
    }
}
