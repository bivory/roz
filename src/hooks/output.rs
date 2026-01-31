//! Hook output types.

use serde::Serialize;
use serde_json::Value;

/// Output returned from hooks.
#[derive(Debug, Clone, Serialize)]
pub struct HookOutput {
    /// The decision (approve or block).
    pub decision: HookDecision,

    /// Reason for the decision (only for block).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,

    /// Additional context to inject into the conversation.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub context: Option<String>,
}

/// Hook decision type.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum HookDecision {
    /// Allow the action to proceed.
    Approve,
    /// Block the action.
    Block,
}

impl HookOutput {
    /// Create an approve decision.
    #[must_use]
    pub fn approve() -> Self {
        Self {
            decision: HookDecision::Approve,
            reason: None,
            context: None,
        }
    }

    /// Create a block decision with a reason.
    #[must_use]
    pub fn block(reason: &str) -> Self {
        Self {
            decision: HookDecision::Block,
            reason: Some(reason.to_string()),
            context: None,
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
    #[serde(skip_serializing_if = "Option::is_none")]
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
        assert_eq!(json, r#"{"decision":"approve"}"#);
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
            serde_json::to_string(&HookDecision::Approve).unwrap(),
            r#""approve""#
        );
        assert_eq!(
            serde_json::to_string(&HookDecision::Block).unwrap(),
            r#""block""#
        );
    }

    #[test]
    fn approve_with_context() {
        let output = HookOutput {
            decision: HookDecision::Approve,
            reason: None,
            context: Some("roz second opinion sources: codex".to_string()),
        };
        let json = serde_json::to_string(&output).unwrap();
        assert!(json.contains("context"));
        assert!(json.contains("codex"));
    }

    #[test]
    fn pre_tool_use_allow_serialization() {
        let output = PreToolUseOutput::allow();
        let json = serde_json::to_string(&output).unwrap();
        assert!(json.contains("hookSpecificOutput"));
        assert!(json.contains("PreToolUse"));
        assert!(json.contains("allow"));
        // reason should be omitted when None
        assert!(!json.contains("reason"));
    }

    #[test]
    fn pre_tool_use_deny_serialization() {
        let output = PreToolUseOutput::deny("Review required before this action.");
        let json = serde_json::to_string(&output).unwrap();
        assert!(json.contains("hookSpecificOutput"));
        assert!(json.contains("PreToolUse"));
        assert!(json.contains("deny"));
        assert!(json.contains("Review required"));
    }

    #[test]
    fn pre_tool_use_ask_serialization() {
        let output = PreToolUseOutput::ask("This tool requires user approval.");
        let json = serde_json::to_string(&output).unwrap();
        assert!(json.contains("ask"));
        assert!(json.contains("user approval"));
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
