//! Soufflé datalog integration for Winter.
//!
//! This crate provides functionality to:
//! - Extract facts from ATProto records to TSV format
//! - Compile rules to Soufflé `.dl` format
//! - Execute Soufflé queries
//! - Parse query results
//! - Cache datalog state for efficient incremental queries

pub mod cache;
mod compiler;
mod error;
mod executor;
mod extractor;

pub use cache::{CachedFactData, DatalogCache};
pub use compiler::RuleCompiler;
pub use error::DatalogError;
pub use executor::SouffleExecutor;
pub use extractor::{ExtractResult, FactExtractor};
