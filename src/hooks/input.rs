//! Hook input parsing.

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

    /// Agent type name (for subagent-stop hook, e.g. "roz:roz").
    #[serde(default)]
    pub agent_type: Option<String>,

    /// Unique agent identifier (for subagent-stop hook).
    #[serde(default)]
    pub agent_id: Option<String>,

    /// Path to the subagent's transcript file (for subagent-stop hook).
    #[serde(default)]
    pub agent_transcript_path: Option<PathBuf>,

    /// Last message from the subagent (for stop/subagent-stop hooks).
    #[serde(default)]
    pub last_assistant_message: Option<String>,

    /// Whether a stop hook is already active (for stop/subagent-stop hooks).
    #[serde(default)]
    pub stop_hook_active: Option<bool>,

    /// Reason for session end (for session-end hook).
    #[serde(default)]
    pub reason: Option<String>,
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
    fn parse_with_agent_fields() {
        let json = r#"{
            "session_id": "test-123",
            "cwd": "/tmp",
            "agent_type": "roz:roz",
            "agent_id": "agent-abc-123",
            "agent_transcript_path": "/tmp/transcript.json",
            "last_assistant_message": "Review complete. All changes verified.",
            "stop_hook_active": false
        }"#;
        let input: HookInput = serde_json::from_str(json).unwrap();
        assert_eq!(input.agent_type, Some("roz:roz".to_string()));
        assert_eq!(input.agent_id, Some("agent-abc-123".to_string()));
        assert_eq!(
            input.agent_transcript_path,
            Some(PathBuf::from("/tmp/transcript.json"))
        );
        assert_eq!(
            input.last_assistant_message,
            Some("Review complete. All changes verified.".to_string())
        );
        assert_eq!(input.stop_hook_active, Some(false));
    }

    #[test]
    fn parse_with_stop_hook_active() {
        let json = r#"{
            "session_id": "test-123",
            "cwd": "/tmp",
            "stop_hook_active": true
        }"#;
        let input: HookInput = serde_json::from_str(json).unwrap();
        assert_eq!(input.stop_hook_active, Some(true));
    }

    #[test]
    fn parse_agent_fields_default_to_none() {
        let json = r#"{"session_id": "test-123", "cwd": "/tmp"}"#;
        let input: HookInput = serde_json::from_str(json).unwrap();
        assert!(input.agent_type.is_none());
        assert!(input.agent_id.is_none());
        assert!(input.agent_transcript_path.is_none());
        assert!(input.last_assistant_message.is_none());
        assert!(input.stop_hook_active.is_none());
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

    #[test]
    fn empty_input_fails() {
        let json = "";
        let result: Result<HookInput, _> = serde_json::from_str(json);
        assert!(result.is_err());
    }

    #[test]
    fn empty_object_fails() {
        let json = "{}";
        let result: Result<HookInput, _> = serde_json::from_str(json);
        assert!(result.is_err(), "missing session_id and cwd should fail");
    }

    #[test]
    fn wrong_type_session_id_fails() {
        let json = r#"{"session_id": 123, "cwd": "/tmp"}"#;
        let result: Result<HookInput, _> = serde_json::from_str(json);
        assert!(result.is_err(), "session_id should be string");
    }

    #[test]
    fn null_optional_fields_ok() {
        let json = r#"{"session_id": "test-123", "cwd": "/tmp", "prompt": null}"#;
        let input: HookInput = serde_json::from_str(json).unwrap();
        assert!(input.prompt.is_none());
    }

    #[test]
    fn parse_with_reason() {
        let json = r#"{"session_id": "test-123", "cwd": "/tmp", "reason": "logout"}"#;
        let input: HookInput = serde_json::from_str(json).unwrap();
        assert_eq!(input.reason, Some("logout".to_string()));
    }

    #[test]
    fn reason_defaults_to_none() {
        let json = r#"{"session_id": "test-123", "cwd": "/tmp"}"#;
        let input: HookInput = serde_json::from_str(json).unwrap();
        assert!(input.reason.is_none());
    }
}
