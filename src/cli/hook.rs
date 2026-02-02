//! `roz hook` command implementation.

use crate::config::load_config;
use crate::core::handle_pre_tool_use;
use crate::error::Result;
use crate::hooks::{HookInput, HookOutput, PreToolUseOutput, dispatch_hook};
use crate::storage::file::{FileBackend, get_roz_home};
use serde::Serialize;
use std::io::{self, Read, Write};

/// Run a hook command.
///
/// Reads JSON from stdin, dispatches to the appropriate hook handler,
/// and writes JSON to stdout.
///
/// # Errors
///
/// Returns an error if writing to stdout fails.
pub fn run(hook_name: &str) -> Result<()> {
    // Read input from stdin
    let mut input_str = String::new();
    io::stdin().read_to_string(&mut input_str)?;

    // Parse input
    let input = match serde_json::from_str::<HookInput>(&input_str) {
        Ok(i) => i,
        Err(e) => {
            eprintln!("roz: warning: failed to parse input: {e}");
            // Fail open - return approve (different for pre-tool-use)
            return if hook_name == "pre-tool-use" {
                write_json(&PreToolUseOutput::allow())
            } else {
                write_json(&HookOutput::approve())
            };
        }
    };

    // Create storage backend
    let store = match FileBackend::new(get_roz_home()) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("roz: warning: storage init failed: {e}");
            // Fail open
            return if hook_name == "pre-tool-use" {
                write_json(&PreToolUseOutput::allow())
            } else {
                write_json(&HookOutput::approve())
            };
        }
    };

    // Dispatch hook - pre-tool-use has different output type
    if hook_name == "pre-tool-use" {
        let config = load_config().unwrap_or_default();
        let output = handle_pre_tool_use(&input, &config, &store);
        write_json(&output)
    } else {
        let output = dispatch_hook(hook_name, &input, &store);
        write_json(&output)
    }
}

/// Write a serializable value as JSON to stdout.
fn write_json<T: Serialize>(output: &T) -> Result<()> {
    let json = serde_json::to_string(output)?;
    io::stdout().write_all(json.as_bytes())?;
    io::stdout().write_all(b"\n")?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use crate::hooks::dispatch_hook;
    use crate::hooks::{HookDecision, HookInput, HookOutput, PreToolUseOutput};
    use crate::storage::{MemoryBackend, MessageStore};
    use std::path::PathBuf;

    fn make_input(session_id: &str) -> HookInput {
        HookInput {
            session_id: session_id.to_string(),
            cwd: PathBuf::from("/tmp"),
            prompt: None,
            tool_name: None,
            tool_input: None,
            tool_response: None,
            source: None,
            subagent_type: None,
            subagent_prompt: None,
            subagent_started_at: None,
        }
    }

    #[test]
    fn dispatch_user_prompt_hook() {
        let store = MemoryBackend::new();
        let mut input = make_input("test-dispatch-1");
        input.prompt = Some("#roz test".to_string());

        let output = dispatch_hook("user-prompt", &input, &store);
        assert!(
            output.decision.is_none(),
            "expected approve (decision=None)"
        );

        // Verify session was created with review enabled
        let state = store.get_session("test-dispatch-1").unwrap().unwrap();
        assert!(state.review.enabled);
    }

    #[test]
    fn dispatch_session_start_hook() {
        let store = MemoryBackend::new();
        let input = make_input("test-dispatch-2");

        let output = dispatch_hook("session-start", &input, &store);
        assert!(
            output.decision.is_none(),
            "expected approve (decision=None)"
        );

        // Verify session was created
        let state = store.get_session("test-dispatch-2").unwrap().unwrap();
        assert!(!state.review.enabled); // No #roz prefix
    }

    #[test]
    fn dispatch_stop_hook_approves_no_review() {
        let store = MemoryBackend::new();
        let input = make_input("test-dispatch-3");

        // First create session without review
        dispatch_hook("session-start", &input, &store);

        let output = dispatch_hook("stop", &input, &store);
        assert!(
            output.decision.is_none(),
            "expected approve (decision=None)"
        );
    }

    #[test]
    fn dispatch_stop_hook_blocks_pending_review() {
        let store = MemoryBackend::new();
        let mut input = make_input("test-dispatch-4");
        input.prompt = Some("#roz test".to_string());

        // Enable review
        dispatch_hook("user-prompt", &input, &store);

        // Stop should block
        let input = make_input("test-dispatch-4");
        let output = dispatch_hook("stop", &input, &store);
        assert!(matches!(output.decision, Some(HookDecision::Block)));
    }

    #[test]
    fn dispatch_unknown_hook_approves() {
        let store = MemoryBackend::new();
        let input = make_input("test-unknown");

        // Unknown hook should approve (fail-open)
        let output = dispatch_hook("nonexistent-hook", &input, &store);
        assert!(
            output.decision.is_none(),
            "expected approve (decision=None)"
        );
    }

    #[test]
    fn hook_output_approve_serialization() {
        let output = HookOutput::approve();
        let json = serde_json::to_string(&output).unwrap();
        // Approve outputs empty object (decision omitted per Claude Code spec)
        assert_eq!(json, "{}");
    }

    #[test]
    fn hook_output_block_serialization() {
        let output = HookOutput::block("test reason");
        let json = serde_json::to_string(&output).unwrap();
        assert!(json.contains("\"decision\":\"block\""));
        assert!(json.contains("test reason"));
    }

    #[test]
    fn pre_tool_use_output_allow_serialization() {
        let output = PreToolUseOutput::allow();
        let json = serde_json::to_string(&output).unwrap();
        assert!(json.contains("\"permissionDecision\":\"allow\""));
    }

    #[test]
    fn pre_tool_use_output_deny_serialization() {
        let output = PreToolUseOutput::deny("denied");
        let json = serde_json::to_string(&output).unwrap();
        assert!(json.contains("\"permissionDecision\":\"deny\""));
    }
}
