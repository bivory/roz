//! Hook dispatch logic.

use crate::core::{handle_session_start, handle_stop, handle_subagent_stop, handle_user_prompt};
use crate::hooks::{HookInput, HookOutput};
use crate::storage::MessageStore;

/// Dispatch a hook by name.
///
/// Returns the appropriate `HookOutput` for the given hook.
pub fn dispatch_hook(name: &str, input: &HookInput, store: &dyn MessageStore) -> HookOutput {
    match name {
        "session-start" => handle_session_start(input, store),
        "user-prompt" => handle_user_prompt(input, store),
        "stop" => handle_stop(input, store),
        "subagent-stop" => handle_subagent_stop(input, store),
        _ => {
            eprintln!("roz: warning: unknown hook: {name}");
            HookOutput::approve() // Fail open for unknown hooks
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::MemoryBackend;
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
    fn dispatch_user_prompt() {
        let store = MemoryBackend::new();
        let mut input = make_input("test-123");
        input.prompt = Some("#roz test".to_string());

        let output = dispatch_hook("user-prompt", &input, &store);
        assert!(
            output.decision.is_none(),
            "expected approve (decision=None)"
        );
    }

    #[test]
    fn dispatch_stop() {
        let store = MemoryBackend::new();
        let input = make_input("test-123");

        let output = dispatch_hook("stop", &input, &store);
        assert!(
            output.decision.is_none(),
            "expected approve (decision=None)"
        );
    }

    #[test]
    fn dispatch_subagent_stop() {
        let store = MemoryBackend::new();
        let input = make_input("test-123");

        let output = dispatch_hook("subagent-stop", &input, &store);
        assert!(
            output.decision.is_none(),
            "expected approve (decision=None)"
        );
    }

    #[test]
    fn dispatch_session_start() {
        let store = MemoryBackend::new();
        let mut input = make_input("test-123");
        input.source = Some("startup".to_string());

        let output = dispatch_hook("session-start", &input, &store);
        assert!(
            output.decision.is_none(),
            "expected approve (decision=None)"
        );

        // Session should be created
        let state = store.get_session("test-123").unwrap().unwrap();
        assert!(!state.trace.is_empty());
    }

    #[test]
    fn dispatch_unknown_hook() {
        let store = MemoryBackend::new();
        let input = make_input("test-123");

        let output = dispatch_hook("unknown", &input, &store);
        // Unknown hooks fail open
        assert!(
            output.decision.is_none(),
            "expected approve (decision=None)"
        );
    }
}
