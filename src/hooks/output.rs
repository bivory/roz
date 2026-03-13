//! Hook output types.

use serde::Serialize;
use serde_json::Value;

/// Output returned from hooks.
///
/// Per Claude Code docs: omit `decision` to allow, set to `"block"` to block.
/// For context injection (`SessionStart`, `UserPromptSubmit`), use `hook_specific_output`.
#[derive(Debug, Clone, Serialize)]
pub struct HookOutput {
    /// The decision - only set to "block" when blocking.
    /// Omit (None) to allow the action to proceed.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub decision: Option<HookDecision>,

    /// Reason for the decision (required when blocking).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,

    /// Hook-specific output for context injection.
    /// Used by `SessionStart` and `UserPromptSubmit` to inject `additionalContext`.
    #[serde(rename = "hookSpecificOutput", skip_serializing_if = "Option::is_none")]
    pub hook_specific_output: Option<ContextOutput>,
}

/// Hook decision type.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum HookDecision {
    /// Block the action.
    Block,
}

/// Context output for hooks that inject `additionalContext` via `hookSpecificOutput`.
///
/// Used by `SessionStart` and `UserPromptSubmit` hooks.
#[derive(Debug, Clone, Serialize)]
pub struct ContextOutput {
    /// The hook event name (e.g., `"SessionStart"`, `"UserPromptSubmit"`).
    #[serde(rename = "hookEventName")]
    pub hook_event_name: String,

    /// Additional context to inject into the conversation.
    #[serde(rename = "additionalContext")]
    pub additional_context: String,
}

impl HookOutput {
    /// Create an approve decision (empty output - omit decision to allow).
    #[must_use]
    pub fn approve() -> Self {
        Self {
            decision: None,
            reason: None,
            hook_specific_output: None,
        }
    }

    /// Create a block decision with a reason.
    #[must_use]
    pub fn block(reason: &str) -> Self {
        Self {
            decision: Some(HookDecision::Block),
            reason: Some(reason.to_string()),
            hook_specific_output: None,
        }
    }

    /// Create an approve decision with additional context injected via `hookSpecificOutput`.
    ///
    /// Used by `SessionStart` and `UserPromptSubmit` hooks.
    #[must_use]
    pub fn approve_with_context(hook_event_name: &str, context: &str) -> Self {
        Self {
            decision: None,
            reason: None,
            hook_specific_output: Some(ContextOutput {
                hook_event_name: hook_event_name.to_string(),
                additional_context: context.to_string(),
            }),
        }
    }
}

/// Output specifically for `PreToolUse` hooks (different schema from Stop hooks).
#[derive(Debug, Clone, Serialize)]
pub struct PreToolUseOutput {
    /// The hook-specific output for pre-tool-use decisions.
    #[serde(rename = "hookSpecificOutput")]
    pub hook_specific_output: PreToolUseDecision,
}

/// Decision for `PreToolUse` hooks.
#[derive(Debug, Clone, Serialize)]
pub struct PreToolUseDecision {
    /// Always `"PreToolUse"`.
    #[serde(rename = "hookEventName")]
    pub hook_event_name: String,

    /// The permission decision.
    #[serde(rename = "permissionDecision")]
    pub permission_decision: PermissionDecision,

    /// Reason for the decision (optional).
    #[serde(
        skip_serializing_if = "Option::is_none",
        rename = "permissionDecisionReason"
    )]
    pub reason: Option<String>,

    /// Updated input to replace the original (optional).
    #[serde(skip_serializing_if = "Option::is_none", rename = "updatedInput")]
    pub updated_input: Option<Value>,
}

/// Permission decision for `PreToolUse` hooks.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum PermissionDecision {
    /// Allow the tool to execute.
    Allow,
    /// Deny the tool execution.
    Deny,
    /// Ask the user for permission.
    Ask,
}

impl PreToolUseOutput {
    /// Create an allow decision.
    #[must_use]
    pub fn allow() -> Self {
        Self {
            hook_specific_output: PreToolUseDecision {
                hook_event_name: "PreToolUse".to_string(),
                permission_decision: PermissionDecision::Allow,
                reason: None,
                updated_input: None,
            },
        }
    }

    /// Create a deny decision with a reason.
    #[must_use]
    pub fn deny(reason: &str) -> Self {
        Self {
            hook_specific_output: PreToolUseDecision {
                hook_event_name: "PreToolUse".to_string(),
                permission_decision: PermissionDecision::Deny,
                reason: Some(reason.to_string()),
                updated_input: None,
            },
        }
    }

    /// Create an ask decision with a reason.
    #[must_use]
    pub fn ask(reason: &str) -> Self {
        Self {
            hook_specific_output: PreToolUseDecision {
                hook_event_name: "PreToolUse".to_string(),
                permission_decision: PermissionDecision::Ask,
                reason: Some(reason.to_string()),
                updated_input: None,
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn approve_serialization() {
        let output = HookOutput::approve();
        let json = serde_json::to_string(&output).unwrap();
        // Empty object - decision omitted to allow
        assert_eq!(json, "{}");
    }

    #[test]
    fn block_serialization() {
        let output = HookOutput::block("Review required");
        let json = serde_json::to_string(&output).unwrap();
        assert_eq!(json, r#"{"decision":"block","reason":"Review required"}"#);
    }

    #[test]
    fn hook_decision_values() {
        assert_eq!(
            serde_json::to_string(&HookDecision::Block).unwrap(),
            r#""block""#
        );
    }

    #[test]
    fn approve_with_context() {
        let output =
            HookOutput::approve_with_context("SessionStart", "roz second opinion sources: codex");
        let json = serde_json::to_string(&output).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();

        // Must be wrapped in hookSpecificOutput
        let specific = &parsed["hookSpecificOutput"];
        assert_eq!(specific["hookEventName"], "SessionStart");
        assert_eq!(
            specific["additionalContext"],
            "roz second opinion sources: codex"
        );

        // decision should be omitted
        assert!(!json.contains("\"decision\""));
        // additionalContext must NOT be at top level
        assert!(parsed.get("additionalContext").is_none());
    }

    #[test]
    fn approve_with_context_user_prompt() {
        let output = HookOutput::approve_with_context("UserPromptSubmit", "extra context");
        let json = serde_json::to_string(&output).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();

        let specific = &parsed["hookSpecificOutput"];
        assert_eq!(specific["hookEventName"], "UserPromptSubmit");
        assert_eq!(specific["additionalContext"], "extra context");
    }

    #[test]
    fn pre_tool_use_allow_serialization() {
        let output = PreToolUseOutput::allow();
        let json = serde_json::to_string(&output).unwrap();
        assert!(json.contains("hookSpecificOutput"));
        assert!(json.contains("PreToolUse"));
        assert!(json.contains("allow"));
        // permissionDecisionReason should be omitted when None
        assert!(!json.contains("permissionDecisionReason"));
    }

    #[test]
    fn pre_tool_use_deny_serialization() {
        let output = PreToolUseOutput::deny("Review required before this action.");
        let json = serde_json::to_string(&output).unwrap();
        assert!(json.contains("hookSpecificOutput"));
        assert!(json.contains("PreToolUse"));
        assert!(json.contains("deny"));
        assert!(json.contains("permissionDecisionReason"));
        assert!(json.contains("Review required"));
    }

    #[test]
    fn pre_tool_use_ask_serialization() {
        let output = PreToolUseOutput::ask("This tool requires user approval.");
        let json = serde_json::to_string(&output).unwrap();
        assert!(json.contains("ask"));
        assert!(json.contains("permissionDecisionReason"));
        assert!(json.contains("user approval"));
    }

    #[test]
    fn pre_tool_use_deny_uses_correct_field_name() {
        let output = PreToolUseOutput::deny("test reason");
        let json = serde_json::to_string(&output).unwrap();
        // Must use permissionDecisionReason, not deprecated 'reason'
        let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();
        let specific = &parsed["hookSpecificOutput"];
        assert!(specific.get("permissionDecisionReason").is_some());
        assert!(specific.get("reason").is_none());
    }

    #[test]
    fn permission_decision_values() {
        assert_eq!(
            serde_json::to_string(&PermissionDecision::Allow).unwrap(),
            r#""allow""#
        );
        assert_eq!(
            serde_json::to_string(&PermissionDecision::Deny).unwrap(),
            r#""deny""#
        );
        assert_eq!(
            serde_json::to_string(&PermissionDecision::Ask).unwrap(),
            r#""ask""#
        );
    }
}
