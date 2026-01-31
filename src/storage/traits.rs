//! Storage trait definitions.

use crate::core::SessionState;
use crate::error::Result;
use chrono::{DateTime, Utc};

/// Storage backend for session state.
pub trait MessageStore: Send + Sync {
    /// Get session state by ID.
    ///
    /// # Errors
    ///
    /// Returns an error if the storage operation fails.
    fn get_session(&self, session_id: &str) -> Result<Option<SessionState>>;

    /// Save session state.
    ///
    /// # Errors
    ///
    /// Returns an error if the storage operation fails.
    fn put_session(&self, state: &SessionState) -> Result<()>;

    /// List recent sessions.
    ///
    /// # Errors
    ///
    /// Returns an error if the storage operation fails.
    fn list_sessions(&self, limit: usize) -> Result<Vec<SessionSummary>>;

    /// Delete a session.
    ///
    /// # Errors
    ///
    /// Returns an error if the storage operation fails.
    fn delete_session(&self, session_id: &str) -> Result<()>;
}

/// Summary information for a session.
#[derive(Debug, Clone)]
pub struct SessionSummary {
    /// Session identifier.
    pub session_id: String,

    /// First user prompt (if any).
    pub first_prompt: Option<String>,

    /// When the session was created.
    pub created_at: DateTime<Utc>,

    /// Number of trace events.
    pub event_count: usize,
}
