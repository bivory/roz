//! Core types and hook logic.

pub mod circuit_breaker;
pub mod hooks;
pub mod state;

pub use hooks::{
    handle_pre_tool_use, handle_session_end, handle_session_start, handle_stop,
    handle_stop_with_config, handle_subagent_stop, handle_user_prompt,
    handle_user_prompt_with_config,
};
pub use state::{
    AttemptOutcome, Decision, DecisionRecord, EventType, GateTrigger, ReviewAttempt, ReviewState,
    SessionState, TraceEvent, TruncatedInput,
};
