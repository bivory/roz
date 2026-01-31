//! Stats command for template A/B test performance.

use crate::core::state::AttemptOutcome;
use crate::error::Result;
use crate::storage::MessageStore;
use crate::storage::file::{FileBackend, get_roz_home};
use chrono::{Duration, Utc};
use std::collections::HashMap;

/// Template statistics for A/B testing analysis.
#[derive(Debug, Default)]
struct TemplateStats {
    /// Number of successful reviews (decision posted).
    success_count: u32,
    /// Total blocks needed across successful reviews.
    total_blocks: u32,
    /// Failures by type.
    not_spawned: u32,
    no_decision: u32,
    bad_session_id: u32,
    /// Still pending (not yet resolved).
    pending: u32,
}

impl TemplateStats {
    /// Record an attempt outcome.
    fn record(&mut self, outcome: &AttemptOutcome) {
        match outcome {
            AttemptOutcome::Pending => {
                self.pending += 1;
            }
            AttemptOutcome::Success { blocks_needed, .. } => {
                self.success_count += 1;
                self.total_blocks += blocks_needed;
            }
            AttemptOutcome::NotSpawned => {
                self.not_spawned += 1;
            }
            AttemptOutcome::NoDecision => {
                self.no_decision += 1;
            }
            AttemptOutcome::BadSessionId => {
                self.bad_session_id += 1;
            }
        }
    }

    /// Get total failure count.
    fn failure_count(&self) -> u32 {
        self.not_spawned + self.no_decision + self.bad_session_id
    }

    /// Get success rate as a percentage.
    fn success_rate(&self) -> f64 {
        let total = self.success_count + self.failure_count();
        if total == 0 {
            0.0
        } else {
            f64::from(self.success_count) / f64::from(total) * 100.0
        }
    }

    /// Get average blocks needed for successful reviews.
    fn avg_blocks(&self) -> f64 {
        if self.success_count == 0 {
            0.0
        } else {
            f64::from(self.total_blocks) / f64::from(self.success_count)
        }
    }
}

/// Run the stats command.
///
/// # Arguments
///
/// * `days` - Number of days to look back (default 30).
///
/// # Errors
///
/// Returns an error if storage operations fail.
pub fn run(days: u32) -> Result<()> {
    let store = FileBackend::new(get_roz_home())?;
    let cutoff = Utc::now() - Duration::days(i64::from(days));
    let sessions = store.list_sessions(10000)?;

    let mut stats: HashMap<String, TemplateStats> = HashMap::new();
    let mut total_sessions = 0;
    let mut sessions_with_attempts = 0;

    for summary in sessions {
        if summary.created_at < cutoff {
            continue;
        }

        total_sessions += 1;

        if let Ok(Some(session)) = store.get_session(&summary.session_id) {
            if !session.review.attempts.is_empty() {
                sessions_with_attempts += 1;
            }
            for attempt in &session.review.attempts {
                let entry = stats.entry(attempt.template_id.clone()).or_default();
                entry.record(&attempt.outcome);
            }
        }
    }

    if stats.is_empty() {
        println!("No template statistics available for the last {days} days.");
        println!("\nSessions analyzed: {total_sessions}");
        println!("Sessions with review attempts: {sessions_with_attempts}");
        return Ok(());
    }

    render_stats_table(&stats, days);
    render_failure_breakdown(&stats);

    println!("\nSessions analyzed: {total_sessions}");
    println!("Sessions with review attempts: {sessions_with_attempts}");

    Ok(())
}

/// Render the stats table.
fn render_stats_table(stats: &HashMap<String, TemplateStats>, days: u32) {
    println!("Template Performance (last {days} days):");
    println!("{}", "─".repeat(70));
    println!(
        "{:<12} {:>10} {:>10} {:>12} {:>14}",
        "Template", "Success", "Failure", "Avg Blocks", "Success Rate"
    );
    println!("{}", "─".repeat(70));

    // Sort by template name for consistent output
    let mut template_ids: Vec<_> = stats.keys().collect();
    template_ids.sort();

    for template_id in template_ids {
        let stat = &stats[template_id];
        println!(
            "{:<12} {:>10} {:>10} {:>12.1} {:>13.1}%",
            template_id,
            stat.success_count,
            stat.failure_count(),
            stat.avg_blocks(),
            stat.success_rate()
        );
    }
    println!("{}", "─".repeat(70));
}

/// Render failure breakdown.
fn render_failure_breakdown(stats: &HashMap<String, TemplateStats>) {
    let total_not_spawned: u32 = stats.values().map(|s| s.not_spawned).sum();
    let total_no_decision: u32 = stats.values().map(|s| s.no_decision).sum();
    let total_bad_session_id: u32 = stats.values().map(|s| s.bad_session_id).sum();
    let total_pending: u32 = stats.values().map(|s| s.pending).sum();

    let total_failures = total_not_spawned + total_no_decision + total_bad_session_id;

    if total_failures == 0 && total_pending == 0 {
        return;
    }

    println!("\nFailure Breakdown:");

    if total_failures > 0 {
        let pct = |n: u32| -> f64 {
            if total_failures == 0 {
                0.0
            } else {
                f64::from(n) / f64::from(total_failures) * 100.0
            }
        };

        if total_not_spawned > 0 {
            println!(
                "  NotSpawned:   {:>4} ({:>5.1}%)",
                total_not_spawned,
                pct(total_not_spawned)
            );
        }
        if total_no_decision > 0 {
            println!(
                "  NoDecision:   {:>4} ({:>5.1}%)",
                total_no_decision,
                pct(total_no_decision)
            );
        }
        if total_bad_session_id > 0 {
            println!(
                "  BadSessionId: {:>4} ({:>5.1}%)",
                total_bad_session_id,
                pct(total_bad_session_id)
            );
        }
    }

    if total_pending > 0 {
        println!("  Pending:      {total_pending:>4}");
    }
}

#[cfg(test)]
#[allow(clippy::float_cmp)] // Exact float comparisons are safe for these test values
mod tests {
    use super::*;

    #[test]
    fn template_stats_default() {
        let stats = TemplateStats::default();
        assert_eq!(stats.success_count, 0);
        assert_eq!(stats.failure_count(), 0);
        assert_eq!(stats.success_rate(), 0.0);
        assert_eq!(stats.avg_blocks(), 0.0);
    }

    #[test]
    fn template_stats_record_success() {
        let mut stats = TemplateStats::default();
        stats.record(&AttemptOutcome::Success {
            decision_type: "complete".to_string(),
            blocks_needed: 2,
        });

        assert_eq!(stats.success_count, 1);
        assert_eq!(stats.total_blocks, 2);
        assert_eq!(stats.avg_blocks(), 2.0);
        assert_eq!(stats.success_rate(), 100.0);
    }

    #[test]
    fn template_stats_record_failures() {
        let mut stats = TemplateStats::default();
        stats.record(&AttemptOutcome::NotSpawned);
        stats.record(&AttemptOutcome::NoDecision);
        stats.record(&AttemptOutcome::BadSessionId);

        assert_eq!(stats.not_spawned, 1);
        assert_eq!(stats.no_decision, 1);
        assert_eq!(stats.bad_session_id, 1);
        assert_eq!(stats.failure_count(), 3);
    }

    #[test]
    fn template_stats_mixed() {
        let mut stats = TemplateStats::default();

        // 3 successes
        for _ in 0..3 {
            stats.record(&AttemptOutcome::Success {
                decision_type: "complete".to_string(),
                blocks_needed: 1,
            });
        }

        // 1 failure
        stats.record(&AttemptOutcome::NotSpawned);

        assert_eq!(stats.success_count, 3);
        assert_eq!(stats.failure_count(), 1);
        assert_eq!(stats.success_rate(), 75.0); // 3/4 = 75%
        assert_eq!(stats.avg_blocks(), 1.0); // 3 blocks / 3 successes
    }

    #[test]
    fn template_stats_pending_not_counted_in_rate() {
        let mut stats = TemplateStats::default();
        stats.record(&AttemptOutcome::Pending);
        stats.record(&AttemptOutcome::Success {
            decision_type: "complete".to_string(),
            blocks_needed: 1,
        });

        // Pending should not affect success rate calculation
        assert_eq!(stats.pending, 1);
        assert_eq!(stats.success_count, 1);
        assert_eq!(stats.success_rate(), 100.0); // Only counts resolved attempts
    }
}
