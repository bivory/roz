//! Circuit breaker logic to prevent infinite blocking loops.
//!
//! The circuit breaker trips when the block count exceeds `max_blocks`,
//! forcing an approve and logging a warning. After `cooldown_seconds`,
//! the circuit breaker resets and blocking can resume.

use crate::config::CircuitBreakerConfig;
use crate::core::state::SessionState;
use chrono::{Duration, Utc};

/// Check if the circuit breaker should trip.
///
/// Returns `true` if the circuit breaker should trip (block count exceeded
/// and cooldown hasn't elapsed since last trip).
#[must_use]
pub fn should_trip(state: &SessionState, config: &CircuitBreakerConfig) -> bool {
    // If previously tripped, check if cooldown has elapsed
    if state.review.circuit_breaker_tripped {
        if let Some(tripped_at) = state.review.circuit_breaker_tripped_at {
            // Safe conversion: cap at i64::MAX for very large values
            let cooldown_secs = i64::try_from(config.cooldown_seconds).unwrap_or(i64::MAX);
            let cooldown = Duration::seconds(cooldown_secs);
            let now = Utc::now();
            // If cooldown has elapsed, the breaker should NOT trip (allow retry)
            if now >= tripped_at + cooldown {
                return false;
            }
        }
        // Still in cooldown or no timestamp (legacy state) - stay tripped
        return true;
    }

    // Check if we've hit the limit
    state.review.block_count >= config.max_blocks
}

/// Reset the circuit breaker after cooldown has elapsed.
///
/// Call this when `should_trip` returns false but `circuit_breaker_tripped` is true,
/// to reset the state for a new blocking cycle.
pub fn reset(state: &mut SessionState) {
    state.review.circuit_breaker_tripped = false;
    state.review.circuit_breaker_tripped_at = None;
    state.review.block_count = 0;
    state.review.enabled = true;
    state.review.decision = crate::core::state::Decision::Pending;

    eprintln!(
        "roz: info: circuit breaker reset after cooldown for session {}",
        state.session_id
    );
}

/// Trip the circuit breaker, updating the session state.
///
/// This forces approval and logs a warning. The breaker remains tripped
/// until the cooldown expires or a new session starts.
pub fn trip(state: &mut SessionState) {
    state.review.circuit_breaker_tripped = true;
    state.review.circuit_breaker_tripped_at = Some(Utc::now());
    state.review.enabled = false;

    eprintln!(
        "roz: warning: circuit breaker tripped after {} blocks for session {}",
        state.review.block_count, state.session_id
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn should_trip_below_limit() {
        let mut state = SessionState::new("test-123");
        state.review.block_count = 2;

        let config = CircuitBreakerConfig {
            max_blocks: 3,
            cooldown_seconds: 300,
        };

        assert!(!should_trip(&state, &config));
    }

    #[test]
    fn should_trip_at_limit() {
        let mut state = SessionState::new("test-123");
        state.review.block_count = 3;

        let config = CircuitBreakerConfig {
            max_blocks: 3,
            cooldown_seconds: 300,
        };

        assert!(should_trip(&state, &config));
    }

    #[test]
    fn should_trip_above_limit() {
        let mut state = SessionState::new("test-123");
        state.review.block_count = 5;

        let config = CircuitBreakerConfig {
            max_blocks: 3,
            cooldown_seconds: 300,
        };

        assert!(should_trip(&state, &config));
    }

    #[test]
    fn should_trip_already_tripped_within_cooldown() {
        let mut state = SessionState::new("test-123");
        state.review.circuit_breaker_tripped = true;
        state.review.circuit_breaker_tripped_at = Some(Utc::now()); // Just tripped
        state.review.block_count = 1; // Below limit, but already tripped

        let config = CircuitBreakerConfig {
            max_blocks: 3,
            cooldown_seconds: 300, // 5 minutes cooldown
        };

        // Should still trip because cooldown hasn't elapsed
        assert!(should_trip(&state, &config));
    }

    #[test]
    fn should_not_trip_after_cooldown() {
        let mut state = SessionState::new("test-123");
        state.review.circuit_breaker_tripped = true;
        // Tripped 10 minutes ago
        state.review.circuit_breaker_tripped_at = Some(Utc::now() - Duration::minutes(10));
        state.review.block_count = 1;

        let config = CircuitBreakerConfig {
            max_blocks: 3,
            cooldown_seconds: 300, // 5 minutes cooldown (already elapsed)
        };

        // Should NOT trip because cooldown has elapsed
        assert!(!should_trip(&state, &config));
    }

    #[test]
    fn should_trip_legacy_state_without_timestamp() {
        let mut state = SessionState::new("test-123");
        state.review.circuit_breaker_tripped = true;
        state.review.circuit_breaker_tripped_at = None; // Legacy state without timestamp

        let config = CircuitBreakerConfig {
            max_blocks: 3,
            cooldown_seconds: 300,
        };

        // Should still trip for legacy states (conservative behavior)
        assert!(should_trip(&state, &config));
    }

    #[test]
    fn trip_sets_flags_and_timestamp() {
        let mut state = SessionState::new("test-123");
        state.review.enabled = true;
        state.review.block_count = 3;

        trip(&mut state);

        assert!(state.review.circuit_breaker_tripped);
        assert!(state.review.circuit_breaker_tripped_at.is_some());
        assert!(!state.review.enabled);
    }

    #[test]
    fn reset_clears_state() {
        let mut state = SessionState::new("test-123");
        state.review.circuit_breaker_tripped = true;
        state.review.circuit_breaker_tripped_at = Some(Utc::now() - Duration::hours(1));
        state.review.block_count = 5;
        state.review.enabled = false;

        reset(&mut state);

        assert!(!state.review.circuit_breaker_tripped);
        assert!(state.review.circuit_breaker_tripped_at.is_none());
        assert_eq!(state.review.block_count, 0);
        assert!(state.review.enabled);
    }
}
