//! `roz trace` command implementation.

use crate::error::{Error, Result};
use crate::storage::MessageStore;
use crate::storage::file::{FileBackend, get_roz_home};

/// Run the trace command.
///
/// Shows trace events for a session.
///
/// # Errors
///
/// Returns an error if the storage backend fails or the session is not found.
pub fn run(session_id: &str, verbose: bool) -> Result<()> {
    let store = FileBackend::new(get_roz_home())?;

    let state = store
        .get_session(session_id)?
        .ok_or_else(|| Error::SessionNotFound(session_id.to_string()))?;

    // Print header
    println!("Session: {}", state.session_id);
    println!("Created: {}", state.created_at.format("%Y-%m-%dT%H:%M:%SZ"));
    println!("Events: {}", state.trace.len());
    println!();

    if state.trace.is_empty() {
        println!("(no trace events)");
        return Ok(());
    }

    // Print trace events
    for (i, event) in state.trace.iter().enumerate() {
        println!(
            "[{:>3}] {} {:?}",
            i + 1,
            event.timestamp.format("%H:%M:%S"),
            event.event_type
        );

        if verbose {
            // Pretty print the payload
            let payload = serde_json::to_string_pretty(&event.payload)?;
            // Indent each line
            for line in payload.lines() {
                println!("      {line}");
            }
            println!();
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use crate::core::state::{EventType, SessionState, TraceEvent};
    use crate::storage::MemoryBackend;
    use crate::storage::MessageStore;
    use chrono::Utc;
    use serde_json::json;

    #[test]
    fn trace_event_formatting() {
        let store = MemoryBackend::new();

        let mut state = SessionState::new("test-trace");
        state.trace.push(TraceEvent {
            id: "evt-1".to_string(),
            timestamp: Utc::now(),
            event_type: EventType::SessionStart,
            payload: json!({"source": "startup"}),
        });
        state.trace.push(TraceEvent {
            id: "evt-2".to_string(),
            timestamp: Utc::now(),
            event_type: EventType::PromptReceived,
            payload: json!({"prompt": "#roz test"}),
        });
        store.put_session(&state).unwrap();

        let retrieved = store.get_session("test-trace").unwrap().unwrap();
        assert_eq!(retrieved.trace.len(), 2);
        assert_eq!(retrieved.trace[0].event_type, EventType::SessionStart);
        assert_eq!(retrieved.trace[1].event_type, EventType::PromptReceived);
    }

    #[test]
    fn trace_empty_session() {
        let store = MemoryBackend::new();

        let state = SessionState::new("test-empty-trace");
        store.put_session(&state).unwrap();

        let retrieved = store.get_session("test-empty-trace").unwrap().unwrap();
        assert!(retrieved.trace.is_empty());
    }

    #[test]
    fn trace_all_event_types() {
        let store = MemoryBackend::new();

        let mut state = SessionState::new("test-all-events");
        let event_types = vec![
            EventType::SessionStart,
            EventType::PromptReceived,
            EventType::GateBlocked,
            EventType::GateAllowed,
            EventType::ToolCompleted,
            EventType::StopHookCalled,
            EventType::RozDecision,
            EventType::TraceCompacted,
            EventType::SessionEnd,
        ];

        for (i, event_type) in event_types.iter().enumerate() {
            state.trace.push(TraceEvent {
                id: format!("evt-{i}"),
                timestamp: Utc::now(),
                event_type: event_type.clone(),
                payload: json!({"type": format!("{:?}", event_type)}),
            });
        }
        store.put_session(&state).unwrap();

        let retrieved = store.get_session("test-all-events").unwrap().unwrap();
        assert_eq!(retrieved.trace.len(), 9);
    }

    #[test]
    fn trace_large_payload() {
        let store = MemoryBackend::new();

        let mut state = SessionState::new("test-large-payload");
        let large_data = "x".repeat(10000);
        state.trace.push(TraceEvent {
            id: "evt-large".to_string(),
            timestamp: Utc::now(),
            event_type: EventType::ToolCompleted,
            payload: json!({"output": large_data}),
        });
        store.put_session(&state).unwrap();

        let retrieved = store.get_session("test-large-payload").unwrap().unwrap();
        assert_eq!(retrieved.trace.len(), 1);
        assert!(retrieved.trace[0].payload["output"].as_str().unwrap().len() == 10000);
    }

    #[test]
    fn trace_complex_nested_payload() {
        let store = MemoryBackend::new();

        let mut state = SessionState::new("test-nested");
        state.trace.push(TraceEvent {
            id: "evt-nested".to_string(),
            timestamp: Utc::now(),
            event_type: EventType::GateBlocked,
            payload: json!({
                "tool": "Bash",
                "input": {
                    "command": "rm -rf /",
                    "flags": ["--force", "--recursive"]
                },
                "context": {
                    "session": "test",
                    "user": {
                        "name": "developer"
                    }
                }
            }),
        });
        store.put_session(&state).unwrap();

        let retrieved = store.get_session("test-nested").unwrap().unwrap();
        let payload = &retrieved.trace[0].payload;
        assert_eq!(payload["tool"], "Bash");
        assert_eq!(payload["input"]["command"], "rm -rf /");
    }
}
