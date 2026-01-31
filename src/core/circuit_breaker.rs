//! Circuit breaker logic to prevent infinite blocking loops.
//!
//! The circuit breaker trips when the block count exceeds `max_blocks`,
//! forcing an approve and logging a warning.

use crate::config::CircuitBreakerConfig;
use crate::core::state::SessionState;

/// Check if the circuit breaker should trip.
///
/// Returns `true` if the circuit breaker has tripped or should trip now.
#[must_use]
pub fn should_trip(state: &SessionState, config: &CircuitBreakerConfig) -> bool {
    // Already tripped
    if state.review.circuit_breaker_tripped {
        return true;
    }

    // Check if we've hit the limit
    state.review.block_count >= config.max_blocks
}

/// Trip the circuit breaker, updating the session state.
///
/// This forces approval and logs a warning. The breaker remains tripped
/// until the cooldown expires or a new session starts.
pub fn trip(state: &mut SessionState) {
    state.review.circuit_breaker_tripped = true;
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
    fn should_trip_already_tripped() {
        let mut state = SessionState::new("test-123");
        state.review.circuit_breaker_tripped = true;
        state.review.block_count = 1; // Below limit, but already tripped

        let config = CircuitBreakerConfig {
            max_blocks: 3,
            cooldown_seconds: 300,
        };

        assert!(should_trip(&state, &config));
    }

    #[test]
    fn trip_sets_flags() {
        let mut state = SessionState::new("test-123");
        state.review.enabled = true;
        state.review.block_count = 3;

        trip(&mut state);

        assert!(state.review.circuit_breaker_tripped);
        assert!(!state.review.enabled);
    }
}
