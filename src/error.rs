//! Error types for roz.

use std::io;
use thiserror::Error;

/// Result type alias for roz operations.
pub type Result<T> = std::result::Result<T, Error>;

/// Errors that can occur in roz operations.
#[derive(Debug, Error)]
pub enum Error {
    /// Storage I/O error.
    #[error("Storage error: {0}")]
    Storage(#[from] io::Error),

    /// JSON serialization/deserialization error.
    #[error("Serialization error: {0}")]
    Serde(#[from] serde_json::Error),

    /// Invalid state encountered.
    #[error("Invalid state: {0}")]
    InvalidState(String),

    /// Session not found.
    #[error("Session not found: {0}")]
    SessionNotFound(String),

    /// Invalid decision type.
    #[error("Invalid decision: {0}")]
    InvalidDecision(String),

    /// Missing required field in hook input.
    #[error("Missing required field: {0}")]
    MissingField(String),

    /// Configuration error.
    #[error("Configuration error: {0}")]
    Config(String),
}
