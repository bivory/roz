//! `roz clean` command implementation.

use crate::core::state::Decision;
use crate::error::Result;
use crate::storage::MessageStore;
use crate::storage::file::{FileBackend, get_roz_home};
use chrono::{Duration, Utc};

/// Run the clean command.
///
/// Removes sessions older than the specified duration.
///
/// # Errors
///
/// Returns an error if the storage backend fails.
pub fn run(before: &str, all: bool) -> Result<()> {
    let store = FileBackend::new(get_roz_home())?;

    let duration = if all {
        Duration::zero() // Clean everything
    } else {
        parse_duration(before)?
    };

    let removed = clean_sessions(&store, duration)?;

    if removed == 0 {
        println!("No sessions to clean.");
    } else {
        println!("Cleaned {removed} session(s).");
    }

    Ok(())
}

/// Parse a duration string like "7d", "30d", "24h".
///
/// # Errors
///
/// Returns an error if the duration format is invalid.
fn parse_duration(s: &str) -> Result<Duration> {
    let s = s.trim();

    if s.is_empty() {
        return Ok(Duration::days(7)); // Default
    }

    let parse_err = |_| crate::error::Error::InvalidState(format!("Invalid duration: {s}"));

    if let Some(stripped) = s.strip_suffix('d') {
        let num: i64 = stripped.parse().map_err(parse_err)?;
        Ok(Duration::days(num))
    } else if let Some(stripped) = s.strip_suffix('h') {
        let num: i64 = stripped.parse().map_err(parse_err)?;
        Ok(Duration::hours(num))
    } else if let Some(stripped) = s.strip_suffix('m') {
        let num: i64 = stripped.parse().map_err(parse_err)?;
        Ok(Duration::minutes(num))
    } else {
        // Default to days if no unit
        let num: i64 = s.parse().map_err(parse_err)?;
        Ok(Duration::days(num))
    }
}

/// Clean sessions older than the given duration.
fn clean_sessions(store: &dyn MessageStore, before: Duration) -> Result<usize> {
    let cutoff = Utc::now() - before;
    let sessions = store.list_sessions(10000)?;
    let mut removed = 0;

    for summary in sessions {
        if summary.created_at >= cutoff {
            continue; // Too recent
        }

        // Don't delete active sessions (review still pending)
        if let Some(session) = store.get_session(&summary.session_id)? {
            if session.review.enabled && matches!(session.review.decision, Decision::Pending) {
                continue; // Still active, skip
            }
        }

        store.delete_session(&summary.session_id)?;
        removed += 1;
    }

    Ok(removed)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::SessionState;
    use crate::storage::MemoryBackend;
    use chrono::Duration as ChronoDuration;

    #[test]
    fn parse_duration_days() {
        let d = parse_duration("7d").unwrap();
        assert_eq!(d, ChronoDuration::days(7));
    }

    #[test]
    fn parse_duration_hours() {
        let d = parse_duration("24h").unwrap();
        assert_eq!(d, ChronoDuration::hours(24));
    }

    #[test]
    fn parse_duration_minutes() {
        let d = parse_duration("30m").unwrap();
        assert_eq!(d, ChronoDuration::minutes(30));
    }

    #[test]
    fn parse_duration_no_unit_defaults_to_days() {
        let d = parse_duration("14").unwrap();
        assert_eq!(d, ChronoDuration::days(14));
    }

    #[test]
    fn parse_duration_empty_defaults_to_7d() {
        let d = parse_duration("").unwrap();
        assert_eq!(d, ChronoDuration::days(7));
    }

    #[test]
    fn clean_removes_old_sessions() {
        let store = MemoryBackend::new();

        // Create an old session
        let mut old_state = SessionState::new("old-session");
        old_state.created_at = Utc::now() - ChronoDuration::days(10);
        old_state.review.decision = Decision::Complete {
            summary: "Done".to_string(),
            second_opinions: None,
        };
        store.put_session(&old_state).unwrap();

        // Create a recent session
        let recent_state = SessionState::new("recent-session");
        store.put_session(&recent_state).unwrap();

        // Clean sessions older than 7 days
        let removed = clean_sessions(&store, ChronoDuration::days(7)).unwrap();

        assert_eq!(removed, 1);
        assert!(store.get_session("old-session").unwrap().is_none());
        assert!(store.get_session("recent-session").unwrap().is_some());
    }

    #[test]
    fn clean_skips_active_sessions() {
        let store = MemoryBackend::new();

        // Create an old but active session (review pending)
        let mut active_state = SessionState::new("active-old");
        active_state.created_at = Utc::now() - ChronoDuration::days(10);
        active_state.review.enabled = true;
        active_state.review.decision = Decision::Pending;
        store.put_session(&active_state).unwrap();

        // Clean sessions older than 7 days
        let removed = clean_sessions(&store, ChronoDuration::days(7)).unwrap();

        assert_eq!(removed, 0); // Should not remove active session
        assert!(store.get_session("active-old").unwrap().is_some());
    }
}
