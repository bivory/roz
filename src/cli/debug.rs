//! `roz debug` command implementation.

use crate::error::{Error, Result};
use crate::storage::MessageStore;
use crate::storage::file::{FileBackend, get_roz_home};

/// Run the debug command.
///
/// Shows full session state dump for debugging.
///
/// # Errors
///
/// Returns an error if the storage backend fails or the session is not found.
pub fn run(session_id: &str) -> Result<()> {
    let store = FileBackend::new(get_roz_home())?;

    let state = store
        .get_session(session_id)?
        .ok_or_else(|| Error::SessionNotFound(session_id.to_string()))?;

    // Pretty print the full state as JSON
    let json = serde_json::to_string_pretty(&state)?;
    println!("{json}");

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::SessionState;
    use crate::core::state::{Decision, EventType, TraceEvent};
    use crate::storage::MemoryBackend;
    use chrono::Utc;
    use serde_json::json;

    #[test]
    fn debug_outputs_json() {
        let store = MemoryBackend::new();

        let mut state = SessionState::new("test-debug");
        state.review.enabled = true;
        state.review.user_prompts.push("#roz test".to_string());
        store.put_session(&state).unwrap();

        let retrieved = store.get_session("test-debug").unwrap().unwrap();
        let json = serde_json::to_string_pretty(&retrieved).unwrap();

        assert!(json.contains("test-debug"));
        assert!(json.contains("#roz test"));
    }

    #[test]
    fn debug_includes_all_state_fields() {
        let store = MemoryBackend::new();

        let mut state = SessionState::new("test-debug-full");
        state.review.enabled = true;
        state.review.decision = Decision::Complete {
            summary: "All good".to_string(),
            second_opinions: Some("Codex agreed".to_string()),
        };
        state.review.block_count = 2;
        state.trace.push(TraceEvent {
            id: "evt-1".to_string(),
            timestamp: Utc::now(),
            event_type: EventType::SessionStart,
            payload: json!({}),
        });
        store.put_session(&state).unwrap();

        let retrieved = store.get_session("test-debug-full").unwrap().unwrap();
        let json = serde_json::to_string_pretty(&retrieved).unwrap();

        assert!(json.contains("test-debug-full"));
        assert!(json.contains("\"enabled\": true"));
        assert!(json.contains("All good"));
        assert!(json.contains("Codex agreed"));
        assert!(json.contains("\"block_count\": 2"));
        assert!(json.contains("session_start"));
    }

    #[test]
    fn debug_session_not_found_error() {
        // Test error handling for missing session
        let result = Error::SessionNotFound("nonexistent".to_string());
        assert!(format!("{result}").contains("nonexistent"));
    }

    #[test]
    fn debug_complex_decision_history() {
        let store = MemoryBackend::new();

        let mut state = SessionState::new("test-history");
        state.review.enabled = true;
        state
            .review
            .decision_history
            .push(crate::core::state::DecisionRecord {
                decision: Decision::Pending,
                timestamp: Utc::now(),
            });
        state
            .review
            .decision_history
            .push(crate::core::state::DecisionRecord {
                decision: Decision::Issues {
                    summary: "Found bugs".to_string(),
                    message_to_agent: Some("Fix them".to_string()),
                },
                timestamp: Utc::now(),
            });
        state.review.decision = Decision::Complete {
            summary: "Fixed".to_string(),
            second_opinions: None,
        };
        store.put_session(&state).unwrap();

        let retrieved = store.get_session("test-history").unwrap().unwrap();
        let json = serde_json::to_string_pretty(&retrieved).unwrap();

        assert!(json.contains("Found bugs"));
        assert!(json.contains("Fix them"));
        assert!(json.contains("Fixed"));
    }
}
