//! Hook input parsing.

use chrono::{DateTime, Utc};
use serde::Deserialize;
use serde_json::Value;
use std::path::PathBuf;

/// Input received from Claude Code hooks.
#[derive(Debug, Clone, Deserialize)]
pub struct HookInput {
    /// Session identifier.
    pub session_id: String,

    /// Current working directory.
    pub cwd: PathBuf,

    /// User prompt (for user-prompt hook).
    #[serde(default)]
    pub prompt: Option<String>,

    /// Tool name (for pre-tool-use hook).
    #[serde(default)]
    pub tool_name: Option<String>,

    /// Tool input (for pre-tool-use hook).
    #[serde(default)]
    pub tool_input: Option<Value>,

    /// Tool response (for post-tool-use hook).
    #[serde(default)]
    pub tool_response: Option<Value>,

    /// Source of session start (startup, resume, clear, compact).
    #[serde(default)]
    pub source: Option<String>,

    /// Subagent type (for subagent-stop hook).
    #[serde(default)]
    pub subagent_type: Option<String>,

    /// Subagent prompt (for subagent-stop hook).
    #[serde(default)]
    pub subagent_prompt: Option<String>,

    /// When subagent began execution (for subagent-stop hook).
    #[serde(default)]
    pub subagent_started_at: Option<DateTime<Utc>>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_minimal_input() {
        let json = r#"{"session_id": "test-123", "cwd": "/tmp"}"#;
        let input: HookInput = serde_json::from_str(json).unwrap();
        assert_eq!(input.session_id, "test-123");
        assert_eq!(input.cwd, PathBuf::from("/tmp"));
        assert!(input.prompt.is_none());
    }

    #[test]
    fn parse_with_prompt() {
        let json = r##"{"session_id": "test-123", "cwd": "/tmp", "prompt": "#roz fix the bug"}"##;
        let input: HookInput = serde_json::from_str(json).unwrap();
        assert_eq!(input.prompt, Some("#roz fix the bug".to_string()));
    }

    #[test]
    fn parse_with_subagent_fields() {
        let json = r#"{
            "session_id": "test-123",
            "cwd": "/tmp",
            "subagent_type": "roz:roz",
            "subagent_prompt": "SESSION_ID=test-123\n\n## Summary\nFixed the bug",
            "subagent_started_at": "2026-01-31T10:00:00Z"
        }"#;
        let input: HookInput = serde_json::from_str(json).unwrap();
        assert_eq!(input.subagent_type, Some("roz:roz".to_string()));
        assert!(input.subagent_started_at.is_some());
    }

    #[test]
    fn missing_session_id_fails() {
        let json = r#"{"cwd": "/tmp"}"#;
        let result: Result<HookInput, _> = serde_json::from_str(json);
        assert!(result.is_err());
    }

    #[test]
    fn extra_fields_ignored() {
        let json = r#"{"session_id": "test-123", "cwd": "/tmp", "unknown_field": "ignored"}"#;
        let result: Result<HookInput, _> = serde_json::from_str(json);
        assert!(result.is_ok());
    }

    #[test]
    fn malformed_json_fails() {
        let json = r#"{"session_id": "test-123", cwd: /tmp}"#;
        let result: Result<HookInput, _> = serde_json::from_str(json);
        assert!(result.is_err());
    }
}
