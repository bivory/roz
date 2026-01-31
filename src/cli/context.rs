//! `roz context` command implementation.

use crate::error::{Error, Result};
use crate::storage::MessageStore;
use crate::storage::file::{FileBackend, get_roz_home};

/// Run the context command.
///
/// Shows user prompts and session context for review.
///
/// # Errors
///
/// Returns an error if the storage backend fails or the session is not found.
pub fn run(session_id: &str) -> Result<()> {
    let store = FileBackend::new(get_roz_home())?;

    let state = store
        .get_session(session_id)?
        .ok_or_else(|| Error::SessionNotFound(session_id.to_string()))?;

    // Print session header
    println!("Session: {}", state.session_id);
    println!("Created: {}", state.created_at.format("%Y-%m-%dT%H:%M:%SZ"));
    println!("Updated: {}", state.updated_at.format("%Y-%m-%dT%H:%M:%SZ"));
    println!();

    // Print review state
    println!("Review enabled: {}", state.review.enabled);
    println!(
        "Decision: {}",
        match &state.review.decision {
            crate::core::Decision::Pending => "Pending".to_string(),
            crate::core::Decision::Complete { summary, .. } => format!("Complete - {summary}"),
            crate::core::Decision::Issues { summary, .. } => format!("Issues - {summary}"),
        }
    );
    println!("Block count: {}", state.review.block_count);
    println!();

    // Print gate trigger info if present
    if let Some(ref trigger) = state.review.gate_trigger {
        println!("Gate trigger:");
        println!("  Tool: {}", trigger.tool_name);
        println!("  Pattern: {}", trigger.pattern_matched);
        println!(
            "  Time: {}",
            trigger.triggered_at.format("%Y-%m-%dT%H:%M:%SZ")
        );
        println!("  Input:");
        // Pretty print the input JSON with indentation
        let input_json = serde_json::to_string_pretty(&trigger.tool_input.value)
            .unwrap_or_else(|_| "null".to_string());
        for line in input_json.lines() {
            println!("    {line}");
        }
        if trigger.tool_input.truncated {
            if let Some(size) = trigger.tool_input.original_size {
                println!("    (truncated, original size: {size} bytes)");
            }
        }
        println!();
    }

    // Print user prompts
    if state.review.user_prompts.is_empty() {
        println!("User prompts: (none)");
    } else {
        println!("User prompts:");
        for (i, prompt) in state.review.user_prompts.iter().enumerate() {
            println!("[{}] {}", i + 1, truncate_prompt(prompt, 200));
        }
    }

    Ok(())
}

/// Truncate a prompt for display.
fn truncate_prompt(prompt: &str, max_len: usize) -> String {
    // Take first line or up to max_len
    let first_line = prompt.lines().next().unwrap_or(prompt);
    if first_line.len() > max_len {
        format!("{}...", &first_line[..max_len])
    } else if prompt.lines().count() > 1 {
        format!("{first_line} [...]")
    } else {
        first_line.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::SessionState;
    use crate::core::state::{Decision, GateTrigger, TruncatedInput};
    use crate::storage::MemoryBackend;
    use crate::storage::MessageStore;
    use chrono::Utc;
    use serde_json::json;

    #[test]
    fn truncate_short_prompt() {
        let prompt = "Short prompt";
        assert_eq!(truncate_prompt(prompt, 100), "Short prompt");
    }

    #[test]
    fn truncate_long_prompt() {
        let prompt = "A".repeat(300);
        let result = truncate_prompt(&prompt, 200);
        assert!(result.len() < 210); // 200 + "..."
        assert!(result.ends_with("..."));
    }

    #[test]
    fn truncate_multiline_prompt() {
        let prompt = "First line\nSecond line\nThird line";
        let result = truncate_prompt(prompt, 100);
        assert_eq!(result, "First line [...]");
    }

    #[test]
    fn context_session_with_gate_trigger() {
        let store = MemoryBackend::new();

        let mut state = SessionState::new("test-gate-context");
        state.review.enabled = true;
        state.review.gate_trigger = Some(GateTrigger {
            tool_name: "mcp__tissue__close".to_string(),
            tool_input: TruncatedInput::from_value(json!({"issue_id": 123})),
            triggered_at: Utc::now(),
            pattern_matched: "mcp__tissue__close*".to_string(),
        });
        store.put_session(&state).unwrap();

        let retrieved = store.get_session("test-gate-context").unwrap().unwrap();
        let trigger = retrieved.review.gate_trigger.unwrap();
        assert_eq!(trigger.tool_name, "mcp__tissue__close");
        assert_eq!(trigger.pattern_matched, "mcp__tissue__close*");
    }

    #[test]
    fn context_session_with_multiple_prompts() {
        let store = MemoryBackend::new();

        let mut state = SessionState::new("test-multi-prompt");
        state.review.enabled = true;
        state
            .review
            .user_prompts
            .push("#roz First task".to_string());
        state
            .review
            .user_prompts
            .push("#roz Second task with more detail".to_string());
        state
            .review
            .user_prompts
            .push("#roz Third task".to_string());
        store.put_session(&state).unwrap();

        let retrieved = store.get_session("test-multi-prompt").unwrap().unwrap();
        assert_eq!(retrieved.review.user_prompts.len(), 3);
    }

    #[test]
    fn context_session_with_decision_states() {
        let store = MemoryBackend::new();

        // Test Pending
        let mut state = SessionState::new("test-pending");
        state.review.enabled = true;
        state.review.decision = Decision::Pending;
        store.put_session(&state).unwrap();
        let retrieved = store.get_session("test-pending").unwrap().unwrap();
        assert!(matches!(retrieved.review.decision, Decision::Pending));

        // Test Complete
        let mut state = SessionState::new("test-complete");
        state.review.enabled = true;
        state.review.decision = Decision::Complete {
            summary: "All verified".to_string(),
            second_opinions: Some("External review agreed".to_string()),
        };
        store.put_session(&state).unwrap();
        let retrieved = store.get_session("test-complete").unwrap().unwrap();
        if let Decision::Complete {
            summary,
            second_opinions,
        } = &retrieved.review.decision
        {
            assert_eq!(summary, "All verified");
            assert_eq!(second_opinions, &Some("External review agreed".to_string()));
        } else {
            panic!("Expected Complete decision");
        }

        // Test Issues
        let mut state = SessionState::new("test-issues");
        state.review.enabled = true;
        state.review.decision = Decision::Issues {
            summary: "Found problems".to_string(),
            message_to_agent: Some("Please fix the tests".to_string()),
        };
        store.put_session(&state).unwrap();
        let retrieved = store.get_session("test-issues").unwrap().unwrap();
        if let Decision::Issues {
            summary,
            message_to_agent,
        } = &retrieved.review.decision
        {
            assert_eq!(summary, "Found problems");
            assert_eq!(message_to_agent, &Some("Please fix the tests".to_string()));
        } else {
            panic!("Expected Issues decision");
        }
    }

    #[test]
    fn context_empty_prompts() {
        let store = MemoryBackend::new();

        let state = SessionState::new("test-no-prompts");
        store.put_session(&state).unwrap();

        let retrieved = store.get_session("test-no-prompts").unwrap().unwrap();
        assert!(retrieved.review.user_prompts.is_empty());
    }

    #[test]
    fn truncate_exact_boundary() {
        let prompt = "A".repeat(100);
        let result = truncate_prompt(&prompt, 100);
        // Exactly at boundary should not truncate
        assert_eq!(result.len(), 100);
        assert!(!result.ends_with("..."));
    }

    #[test]
    fn truncate_one_over_boundary() {
        let prompt = "A".repeat(101);
        let result = truncate_prompt(&prompt, 100);
        // One over should truncate
        assert!(result.len() < 105);
        assert!(result.ends_with("..."));
    }
}
