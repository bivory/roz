//! Session state types.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};

/// Session state stored on disk.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionState {
    /// Session identifier (from Claude Code).
    pub session_id: String,

    /// Review state.
    pub review: ReviewState,

    /// Trace events for debugging.
    pub trace: Vec<TraceEvent>,

    /// When the session was created.
    pub created_at: DateTime<Utc>,

    /// When the session was last updated.
    pub updated_at: DateTime<Utc>,
}

impl SessionState {
    /// Create a new session state.
    #[must_use]
    pub fn new(session_id: &str) -> Self {
        let now = Utc::now();
        Self {
            session_id: session_id.to_string(),
            review: ReviewState::default(),
            trace: Vec::new(),
            created_at: now,
            updated_at: now,
        }
    }
}

/// Review state within a session.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ReviewState {
    /// Whether review is enabled for this session.
    pub enabled: bool,

    /// Current decision status.
    pub decision: Decision,

    /// History of decisions for debugging.
    pub decision_history: Vec<DecisionRecord>,

    /// User prompts that triggered review.
    pub user_prompts: Vec<String>,

    /// Tool that triggered gate (if applicable).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub gate_trigger: Option<GateTrigger>,

    /// When gate was last approved (for scope tracking).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub gate_approved_at: Option<DateTime<Utc>>,

    /// When the last user prompt was received.
    pub last_prompt_at: Option<DateTime<Utc>>,

    /// When the current review cycle started (for prompt isolation).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub review_started_at: Option<DateTime<Utc>>,

    /// Number of times the stop hook has blocked.
    pub block_count: u32,

    /// Whether the circuit breaker has tripped.
    #[serde(default)]
    pub circuit_breaker_tripped: bool,

    /// Track each block attempt for A/B testing.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub attempts: Vec<ReviewAttempt>,
}

/// Review decision.
#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq, Eq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Decision {
    /// Awaiting review.
    #[default]
    Pending,

    /// Work approved.
    Complete {
        /// Summary of findings.
        summary: String,

        /// Record of second opinions obtained (for future use).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        second_opinions: Option<String>,
    },

    /// Issues found that need fixing.
    Issues {
        /// Summary of issues.
        summary: String,

        /// Message to the agent about what to fix.
        message_to_agent: Option<String>,
    },
}

/// Record of a past decision.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DecisionRecord {
    /// The decision.
    pub decision: Decision,

    /// When the decision was made.
    pub timestamp: DateTime<Utc>,
}

/// Context about what triggered the gate (stored for roz to review).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GateTrigger {
    /// Tool name (e.g., `mcp__tissue__close_issue`).
    pub tool_name: String,

    /// Tool input (truncated if large).
    pub tool_input: TruncatedInput,

    /// When the gate was triggered.
    pub triggered_at: DateTime<Utc>,

    /// Which config pattern matched.
    pub pattern_matched: String,
}

/// Tool input with truncation for large payloads.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TruncatedInput {
    /// The input value (truncated to `max_size` if needed).
    pub value: Value,

    /// True if the original input was truncated.
    pub truncated: bool,

    /// SHA-256 hash of the original full input (for verification).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub original_hash: Option<String>,

    /// Original size in bytes (if truncated).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub original_size: Option<usize>,
}

impl TruncatedInput {
    /// Maximum size in bytes before truncation.
    const MAX_SIZE: usize = 10 * 1024; // 10KB limit

    /// Create a `TruncatedInput` from a JSON value, truncating if necessary.
    #[must_use]
    pub fn from_value(input: Value) -> Self {
        let serialized = serde_json::to_string(&input).unwrap_or_default();

        if serialized.len() <= Self::MAX_SIZE {
            return Self {
                value: input,
                truncated: false,
                original_hash: None,
                original_size: None,
            };
        }

        // Truncate: keep structure but limit string values
        let hash = sha256_hex(&serialized);
        let truncated_value = truncate_json_value(&input, Self::MAX_SIZE);

        Self {
            value: truncated_value,
            truncated: true,
            original_hash: Some(hash),
            original_size: Some(serialized.len()),
        }
    }
}

impl Default for TruncatedInput {
    fn default() -> Self {
        Self {
            value: Value::Null,
            truncated: false,
            original_hash: None,
            original_size: None,
        }
    }
}

/// Compute SHA-256 hash and return as hex string.
fn sha256_hex(data: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(data.as_bytes());
    hex::encode(hasher.finalize())
}

/// Truncate a JSON value recursively to fit within a byte budget.
fn truncate_json_value(value: &Value, budget: usize) -> Value {
    match value {
        Value::String(s) if s.len() > budget => {
            let truncate_at = budget.min(200);
            Value::String(format!(
                "{}... [truncated, {} bytes total]",
                &s[..truncate_at],
                s.len()
            ))
        }
        Value::Object(map) => {
            let mut new_map = serde_json::Map::new();
            let per_key = budget / map.len().max(1);
            for (k, v) in map {
                new_map.insert(k.clone(), truncate_json_value(v, per_key));
            }
            Value::Object(new_map)
        }
        Value::Array(arr) if arr.len() > 10 => {
            let mut new_arr: Vec<Value> = arr
                .iter()
                .take(10)
                .map(|v| truncate_json_value(v, budget / 10))
                .collect();
            new_arr.push(Value::String(format!(
                "... [{} more items]",
                arr.len() - 10
            )));
            Value::Array(new_arr)
        }
        other => other.clone(),
    }
}

/// Track each block attempt for A/B testing.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReviewAttempt {
    /// Template ID used for this attempt.
    pub template_id: String,

    /// When this attempt was made.
    pub timestamp: DateTime<Utc>,

    /// Outcome of this attempt.
    pub outcome: AttemptOutcome,
}

/// Outcome of a review attempt.
#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq, Eq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum AttemptOutcome {
    /// Still waiting for result.
    #[default]
    Pending,

    /// Roz spawned and posted decision.
    Success {
        /// The decision that was posted.
        decision_type: String,

        /// Number of blocks needed to reach this outcome.
        blocks_needed: u32,
    },

    /// Agent didn't spawn roz (circuit breaker tripped).
    NotSpawned,

    /// Roz spawned but didn't post decision.
    NoDecision,

    /// Roz spawned but `SESSION_ID` was missing/wrong.
    BadSessionId,
}

/// Trace event for debugging.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TraceEvent {
    /// Unique event identifier.
    pub id: String,

    /// When the event occurred.
    pub timestamp: DateTime<Utc>,

    /// Type of event.
    pub event_type: EventType,

    /// Event payload.
    pub payload: Value,
}

/// Types of trace events.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum EventType {
    /// Session started.
    SessionStart,
    /// User prompt received.
    PromptReceived,
    /// Pre-tool-use hook blocked a gated tool.
    GateBlocked,
    /// Pre-tool-use hook allowed a gated tool (for debugging).
    GateAllowed,
    /// Tool completed.
    ToolCompleted,
    /// Stop hook called.
    StopHookCalled,
    /// Roz decision recorded.
    RozDecision,
    /// Trace was truncated due to `max_events` limit.
    TraceCompacted,
    /// Session ended.
    SessionEnd,
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn session_state_new() {
        let state = SessionState::new("test-123");
        assert_eq!(state.session_id, "test-123");
        assert!(!state.review.enabled);
        assert_eq!(state.review.decision, Decision::Pending);
        assert!(state.trace.is_empty());
    }

    #[test]
    fn decision_serialization_pending() {
        let decision = Decision::Pending;
        let json = serde_json::to_string(&decision).unwrap();
        assert!(json.contains("pending"));
        let parsed: Decision = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, Decision::Pending);
    }

    #[test]
    fn decision_serialization_complete() {
        let decision = Decision::Complete {
            summary: "All good".to_string(),
            second_opinions: None,
        };
        let json = serde_json::to_string(&decision).unwrap();
        assert!(json.contains("complete"));
        assert!(json.contains("All good"));
        // second_opinions should be skipped when None
        assert!(!json.contains("second_opinions"));
        let parsed: Decision = serde_json::from_str(&json).unwrap();
        assert!(matches!(parsed, Decision::Complete { .. }));
    }

    #[test]
    fn decision_serialization_complete_with_second_opinions() {
        let decision = Decision::Complete {
            summary: "All good".to_string(),
            second_opinions: Some("Codex agreed".to_string()),
        };
        let json = serde_json::to_string(&decision).unwrap();
        assert!(json.contains("second_opinions"));
        assert!(json.contains("Codex agreed"));
        let parsed: Decision = serde_json::from_str(&json).unwrap();
        if let Decision::Complete {
            second_opinions, ..
        } = parsed
        {
            assert_eq!(second_opinions, Some("Codex agreed".to_string()));
        } else {
            panic!("Expected Complete decision");
        }
    }

    #[test]
    fn decision_serialization_issues() {
        let decision = Decision::Issues {
            summary: "Found bugs".to_string(),
            message_to_agent: Some("Fix the tests".to_string()),
        };
        let json = serde_json::to_string(&decision).unwrap();
        assert!(json.contains("issues"));
        let parsed: Decision = serde_json::from_str(&json).unwrap();
        assert!(matches!(parsed, Decision::Issues { .. }));
    }

    #[test]
    fn review_state_default() {
        let review = ReviewState::default();
        assert!(!review.enabled);
        assert_eq!(review.decision, Decision::Pending);
        assert!(review.user_prompts.is_empty());
        assert_eq!(review.block_count, 0);
        assert!(!review.circuit_breaker_tripped);
        assert!(review.gate_trigger.is_none());
        assert!(review.gate_approved_at.is_none());
        assert!(review.review_started_at.is_none());
        assert!(review.attempts.is_empty());
    }

    #[test]
    fn event_type_serialization() {
        let event_type = EventType::PromptReceived;
        let json = serde_json::to_string(&event_type).unwrap();
        assert_eq!(json, r#""prompt_received""#);
    }

    #[test]
    fn gate_event_types_serialization() {
        let blocked = EventType::GateBlocked;
        assert_eq!(
            serde_json::to_string(&blocked).unwrap(),
            r#""gate_blocked""#
        );

        let allowed = EventType::GateAllowed;
        assert_eq!(
            serde_json::to_string(&allowed).unwrap(),
            r#""gate_allowed""#
        );
    }

    #[test]
    fn truncated_input_small_value() {
        let value = json!({"key": "small value"});
        let truncated = TruncatedInput::from_value(value.clone());

        assert!(!truncated.truncated);
        assert!(truncated.original_hash.is_none());
        assert!(truncated.original_size.is_none());
        assert_eq!(truncated.value, value);
    }

    #[test]
    fn truncated_input_large_string() {
        // Create a string larger than 10KB
        let large_string = "x".repeat(15_000);
        let value = json!(large_string);
        let truncated = TruncatedInput::from_value(value);

        assert!(truncated.truncated);
        assert!(truncated.original_hash.is_some());
        assert_eq!(truncated.original_size, Some(15_002)); // Includes quotes
        // Truncated string should contain truncation message
        if let Value::String(s) = &truncated.value {
            assert!(s.contains("truncated"));
            assert!(s.len() < 500); // Should be much smaller than original
        } else {
            panic!("Expected string value");
        }
    }

    #[test]
    fn truncated_input_large_array() {
        // Create an array with more than 10 items
        let value = json!([1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15]);
        let truncated = TruncatedInput::from_value(value);

        // Array is small enough in bytes, but check truncation logic works
        if let Value::Array(arr) = truncated.value {
            // If within size limit, it should be unchanged
            assert_eq!(arr.len(), 15);
        }
    }

    #[test]
    fn gate_trigger_serialization() {
        let trigger = GateTrigger {
            tool_name: "mcp__tissue__close_issue".to_string(),
            tool_input: TruncatedInput::from_value(json!({"issue_id": "123"})),
            triggered_at: Utc::now(),
            pattern_matched: "mcp__tissue__close*".to_string(),
        };

        let json = serde_json::to_string(&trigger).unwrap();
        assert!(json.contains("mcp__tissue__close_issue"));
        assert!(json.contains("pattern_matched"));

        let parsed: GateTrigger = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.tool_name, trigger.tool_name);
        assert_eq!(parsed.pattern_matched, trigger.pattern_matched);
    }

    #[test]
    fn review_attempt_serialization() {
        let attempt = ReviewAttempt {
            template_id: "v1".to_string(),
            timestamp: Utc::now(),
            outcome: AttemptOutcome::Pending,
        };

        let json = serde_json::to_string(&attempt).unwrap();
        assert!(json.contains("v1"));
        assert!(json.contains("pending"));

        let parsed: ReviewAttempt = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.template_id, "v1");
        assert_eq!(parsed.outcome, AttemptOutcome::Pending);
    }

    #[test]
    fn attempt_outcome_variants() {
        let pending = AttemptOutcome::Pending;
        assert_eq!(
            serde_json::to_string(&pending).unwrap(),
            r#"{"type":"pending"}"#
        );

        let success = AttemptOutcome::Success {
            decision_type: "complete".to_string(),
            blocks_needed: 2,
        };
        let json = serde_json::to_string(&success).unwrap();
        assert!(json.contains("success"));
        assert!(json.contains("blocks_needed"));

        let not_spawned = AttemptOutcome::NotSpawned;
        assert_eq!(
            serde_json::to_string(&not_spawned).unwrap(),
            r#"{"type":"not_spawned"}"#
        );
    }
}
