//! Core agent logic for Winter.
//!
//! This crate provides:
//! - Identity loading and management
//! - Daemon state management
//! - Context assembly for Claude prompts
//! - Persistent session with inbox-driven model
//! - Claude SDK integration for agent invocation

mod agent;
mod context;
mod error;
mod identity;
mod prompt;
mod state;

pub use agent::Agent;
pub use context::{AgentContext, ContextTrigger, ConversationHistoryMessage, CustomToolSummary};
pub use error::AgentError;
pub use identity::IdentityManager;
pub use prompt::PromptBuilder;
pub use state::StateManager;
