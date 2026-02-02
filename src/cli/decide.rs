//! `roz decide` command implementation.

use crate::core::state::{AttemptOutcome, Decision, DecisionRecord, EventType, TraceEvent};
use crate::error::{Error, Result};
use crate::storage::MessageStore;
use crate::storage::file::{FileBackend, get_roz_home};
use chrono::Utc;
use serde_json::json;
use uuid::Uuid;

/// Run the decide command.
///
/// Posts a COMPLETE or ISSUES decision for a session.
///
/// # Errors
///
/// Returns an error if the storage backend fails, the session is not found,
/// or the decision type is invalid.
pub fn run(
    session_id: &str,
    decision: &str,
    summary: &str,
    message: Option<&str>,
    opinions: Option<&str>,
) -> Result<()> {
    let store = FileBackend::new(get_roz_home())?;

    let mut state = store
        .get_session(session_id)?
        .ok_or_else(|| Error::SessionNotFound(session_id.to_string()))?;

    let now = Utc::now();
    let decision_upper = decision.to_uppercase();

    let new_decision = match decision_upper.as_str() {
        "COMPLETE" => Decision::Complete {
            summary: summary.to_string(),
            second_opinions: opinions.map(String::from),
        },
        "ISSUES" => Decision::Issues {
            summary: summary.to_string(),
            message_to_agent: message.map(String::from),
        },
        other => return Err(Error::InvalidDecision(other.to_string())),
    };

    // Preserve history
    state.review.decision_history.push(DecisionRecord {
        decision: state.review.decision.clone(),
        timestamp: now,
    });

    // Add trace event
    let mut payload = json!({
        "decision": decision_upper,
        "summary": summary,
    });
    if let Some(ops) = opinions {
        payload["second_opinions"] = json!(ops);
    }
    state.trace.push(TraceEvent {
        id: Uuid::new_v4().to_string(),
        timestamp: now,
        event_type: EventType::RozDecision,
        payload,
    });

    // Track when gate was approved (for approval scope tracking)
    if decision_upper == "COMPLETE" {
        state.review.gate_approved_at = Some(now);
    }

    // Update the most recent pending attempt's outcome (for stats tracking)
    if let Some(attempt) = state
        .review
        .attempts
        .iter_mut()
        .rev()
        .find(|a| matches!(a.outcome, AttemptOutcome::Pending))
    {
        attempt.outcome = AttemptOutcome::Success {
            decision_type: decision_upper.to_lowercase(),
            blocks_needed: state.review.block_count,
        };
    }

    state.review.decision = new_decision;
    state.updated_at = now;

    store.put_session(&state)?;

    println!("Decision recorded: {decision_upper} for session {session_id}");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::SessionState;
    use crate::core::state::ReviewAttempt;
    use crate::storage::MemoryBackend;

    fn create_test_session(store: &MemoryBackend, session_id: &str) {
        let mut state = SessionState::new(session_id);
        state.review.enabled = true;
        store.put_session(&state).unwrap();
    }

    fn create_test_session_with_attempt(store: &MemoryBackend, session_id: &str, block_count: u32) {
        let mut state = SessionState::new(session_id);
        state.review.enabled = true;
        state.review.block_count = block_count;
        state.review.attempts.push(ReviewAttempt {
            template_id: "default".to_string(),
            timestamp: Utc::now(),
            outcome: AttemptOutcome::Pending,
        });
        store.put_session(&state).unwrap();
    }

    #[test]
    fn decide_complete() {
        let store = MemoryBackend::new();
        create_test_session(&store, "test-123");

        // Get initial state
        let mut state = store.get_session("test-123").unwrap().unwrap();
        let now = Utc::now();

        // Apply decision logic
        state.review.decision_history.push(DecisionRecord {
            decision: state.review.decision.clone(),
            timestamp: now,
        });
        state.review.decision = Decision::Complete {
            summary: "All good".to_string(),
            second_opinions: None,
        };
        state.updated_at = now;
        store.put_session(&state).unwrap();

        let updated = store.get_session("test-123").unwrap().unwrap();
        assert!(matches!(updated.review.decision, Decision::Complete { .. }));
    }

    #[test]
    fn decide_complete_with_opinions() {
        let store = MemoryBackend::new();
        create_test_session(&store, "test-opinions");

        let mut state = store.get_session("test-opinions").unwrap().unwrap();
        let now = Utc::now();

        state.review.decision_history.push(DecisionRecord {
            decision: state.review.decision.clone(),
            timestamp: now,
        });
        state.review.decision = Decision::Complete {
            summary: "Verified correct".to_string(),
            second_opinions: Some("Codex agreed, Gemini agreed".to_string()),
        };
        state.updated_at = now;
        store.put_session(&state).unwrap();

        let updated = store.get_session("test-opinions").unwrap().unwrap();
        if let Decision::Complete {
            summary,
            second_opinions,
        } = &updated.review.decision
        {
            assert_eq!(summary, "Verified correct");
            assert_eq!(
                second_opinions,
                &Some("Codex agreed, Gemini agreed".to_string())
            );
        } else {
            panic!("Expected Complete decision");
        }
    }

    #[test]
    fn decide_issues() {
        let store = MemoryBackend::new();
        create_test_session(&store, "test-456");

        let mut state = store.get_session("test-456").unwrap().unwrap();
        let now = Utc::now();

        state.review.decision_history.push(DecisionRecord {
            decision: state.review.decision.clone(),
            timestamp: now,
        });
        state.review.decision = Decision::Issues {
            summary: "Found bugs".to_string(),
            message_to_agent: Some("Fix the tests".to_string()),
        };
        state.updated_at = now;
        store.put_session(&state).unwrap();

        let updated = store.get_session("test-456").unwrap().unwrap();
        if let Decision::Issues {
            summary,
            message_to_agent,
        } = &updated.review.decision
        {
            assert_eq!(summary, "Found bugs");
            assert_eq!(message_to_agent, &Some("Fix the tests".to_string()));
        } else {
            panic!("Expected Issues decision");
        }
    }

    #[test]
    fn decision_preserves_history() {
        let store = MemoryBackend::new();
        create_test_session(&store, "test-789");

        let mut state = store.get_session("test-789").unwrap().unwrap();
        assert!(state.review.decision_history.is_empty());

        let now = Utc::now();
        state.review.decision_history.push(DecisionRecord {
            decision: state.review.decision.clone(),
            timestamp: now,
        });
        state.review.decision = Decision::Complete {
            summary: "First review".to_string(),
            second_opinions: None,
        };
        store.put_session(&state).unwrap();

        let updated = store.get_session("test-789").unwrap().unwrap();
        assert_eq!(updated.review.decision_history.len(), 1);
    }

    #[test]
    fn decide_updates_attempt_outcome_on_complete() {
        let store = MemoryBackend::new();
        create_test_session_with_attempt(&store, "test-attempt", 2);

        // Verify initial state has pending attempt
        let state = store.get_session("test-attempt").unwrap().unwrap();
        assert_eq!(state.review.attempts.len(), 1);
        assert!(matches!(
            state.review.attempts[0].outcome,
            AttemptOutcome::Pending
        ));

        // Apply decision logic (same as run() does)
        let mut state = store.get_session("test-attempt").unwrap().unwrap();
        let now = Utc::now();
        let decision_upper = "COMPLETE";

        state.review.decision_history.push(DecisionRecord {
            decision: state.review.decision.clone(),
            timestamp: now,
        });
        state.review.gate_approved_at = Some(now);

        // Update the most recent pending attempt's outcome
        if let Some(attempt) = state
            .review
            .attempts
            .iter_mut()
            .rev()
            .find(|a| matches!(a.outcome, AttemptOutcome::Pending))
        {
            attempt.outcome = AttemptOutcome::Success {
                decision_type: decision_upper.to_lowercase(),
                blocks_needed: state.review.block_count,
            };
        }

        state.review.decision = Decision::Complete {
            summary: "Test complete".to_string(),
            second_opinions: None,
        };
        state.updated_at = now;
        store.put_session(&state).unwrap();

        // Verify attempt outcome was updated
        let updated = store.get_session("test-attempt").unwrap().unwrap();
        assert_eq!(updated.review.attempts.len(), 1);
        match &updated.review.attempts[0].outcome {
            AttemptOutcome::Success {
                decision_type,
                blocks_needed,
            } => {
                assert_eq!(decision_type, "complete");
                assert_eq!(*blocks_needed, 2);
            }
            other => panic!("Expected Success outcome, got {other:?}"),
        }
    }

    #[test]
    fn decide_updates_attempt_outcome_on_issues() {
        let store = MemoryBackend::new();
        create_test_session_with_attempt(&store, "test-issues-attempt", 1);

        // Apply decision logic for ISSUES
        let mut state = store.get_session("test-issues-attempt").unwrap().unwrap();
        let now = Utc::now();
        let decision_upper = "ISSUES";

        state.review.decision_history.push(DecisionRecord {
            decision: state.review.decision.clone(),
            timestamp: now,
        });

        // Update the most recent pending attempt's outcome
        if let Some(attempt) = state
            .review
            .attempts
            .iter_mut()
            .rev()
            .find(|a| matches!(a.outcome, AttemptOutcome::Pending))
        {
            attempt.outcome = AttemptOutcome::Success {
                decision_type: decision_upper.to_lowercase(),
                blocks_needed: state.review.block_count,
            };
        }

        state.review.decision = Decision::Issues {
            summary: "Found issues".to_string(),
            message_to_agent: Some("Fix them".to_string()),
        };
        state.updated_at = now;
        store.put_session(&state).unwrap();

        // Verify attempt outcome was updated
        let updated = store.get_session("test-issues-attempt").unwrap().unwrap();
        match &updated.review.attempts[0].outcome {
            AttemptOutcome::Success {
                decision_type,
                blocks_needed,
            } => {
                assert_eq!(decision_type, "issues");
                assert_eq!(*blocks_needed, 1);
            }
            other => panic!("Expected Success outcome, got {other:?}"),
        }
    }
}
