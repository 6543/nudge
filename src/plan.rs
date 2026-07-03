//! Pure timer arithmetic: the wait schedule ("plan"), decay, defaults,
//! and human-readable duration formatting.
//!
//! Nothing in here sleeps, draws, or spawns processes — it is plain data
//! and math, which is what makes the timer behaviour fully unit-testable.
//! `main.rs` walks the plan performing real sleeps and alerts.

use std::time::Duration;

/// Default runway when no second positional arg is given.
/// >= 2h initial -> 20m runway; otherwise 10m.
pub fn default_runway(initial: Duration) -> Duration {
    const TWO_HOURS: Duration = Duration::from_secs(2 * 3600);
    if initial >= TWO_HOURS {
        Duration::from_secs(20 * 60)
    } else {
        Duration::from_secs(10 * 60)
    }
}

/// Reduce a Duration by a factor in (0,1). Saturates at zero.
pub fn decay(prev: Duration, factor: f64) -> Duration {
    let secs = prev.as_secs_f64() * factor;
    if secs.is_finite() && secs >= 0.0 {
        Duration::from_secs_f64(secs)
    } else {
        Duration::ZERO
    }
}

/// Format a Duration as a short human string: "2m 30s", "45s", "1h 5m".
/// Seconds are dropped above 1 hour (granularity isn't useful at that range).
pub fn humanize(d: Duration) -> String {
    let total = d.as_secs();
    let h = total / 3600;
    let m = (total % 3600) / 60;
    let s = total % 60;
    match (h, m, s) {
        (0, 0, s) => format!("{s}s"),
        (0, m, 0) => format!("{m}m"),
        (0, m, s) => format!("{m}m {s}s"),
        (h, 0, _) => format!("{h}h"),
        (h, m, _) => format!("{h}h {m}m"),
    }
}

/// The algorithm's output as a pure data structure: the sequence of waits
/// the program will perform before each alert. The number of alerts equals
/// `waits.len()`. After the final alert the screen is locked.
///
/// `waits[0]` is always the initial duration (sleep before alert 1).
/// `waits[1..]` are post-alert sleeps, each one decayed from the previous.
/// The loop stops generating waits when the next decayed value would be
/// ≤ floor — that wait is omitted (we lock instead).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Plan {
    pub waits: Vec<Duration>,
}

impl Plan {
    /// Total wall-clock time from `nudge` invocation until the screen lock,
    /// not counting alert overlay duration (typically ~2s × alert count,
    /// which is negligible against the multi-minute waits).
    pub fn total(&self) -> Duration {
        self.waits.iter().sum()
    }

    /// Number of alerts the user will see before the lock.
    pub fn alert_count(&self) -> usize {
        self.waits.len()
    }
}

/// Compute the full plan up-front, given the algorithm parameters.
///
/// This is the single source of truth for the timer behaviour. `run()` in
/// main.rs walks the same plan executing real sleeps and alerts; tests walk
/// it as data.
///
/// Semantics:
///   - `waits[0]` is the initial wait (before alert 1).
///   - For each subsequent alert N, `waits[N-1]` is `decay(prev_wait)` where
///     `prev_wait` starts at `runway` and is replaced by `decay(prev_wait)`
///     each iteration.
///   - The loop terminates when `decay(prev_wait) <= floor`; the final alert
///     is the one with `next <= floor`, after which the screen locks.
pub fn plan(initial: Duration, runway: Duration, decay_factor: f64, floor: Duration) -> Plan {
    let mut waits = vec![initial];
    let mut wait = runway;
    loop {
        let next = decay(wait, decay_factor);
        if next <= floor {
            // The alert about to fire is the final one; lock follows.
            // No further wait is scheduled.
            break;
        }
        // `next` becomes the actual sleep before the *following* alert.
        waits.push(next);
        wait = next;
    }
    Plan { waits }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::duration as durparse;

    #[test]
    fn decay_halves() {
        assert_eq!(
            decay(Duration::from_secs(300), 0.5),
            Duration::from_secs(150)
        );
    }

    #[test]
    fn decay_saturates_at_zero() {
        assert_eq!(decay(Duration::ZERO, 0.5), Duration::ZERO);
    }

    #[test]
    fn decay_reaches_floor() {
        // 300s * 0.5^N: 300, 150, 75, 37.5, 18.75, 9.375, 4.6875
        // floor=5s should stop loop after the 9.375 alert, when next=4.6875 <= 5
        let mut w = Duration::from_secs(300);
        let floor = Duration::from_secs(5);
        let mut n = 0;
        loop {
            let next = decay(w, 0.5);
            if next <= floor {
                break;
            }
            w = next;
            n += 1;
            assert!(n < 100, "decay loop never terminated");
        }
        assert_eq!(n, 5);
    }

    #[test]
    fn humanize_seconds_only() {
        assert_eq!(humanize(Duration::from_secs(5)), "5s");
        assert_eq!(humanize(Duration::from_secs(45)), "45s");
    }

    #[test]
    fn humanize_minutes_only() {
        assert_eq!(humanize(Duration::from_secs(60)), "1m");
        assert_eq!(humanize(Duration::from_secs(300)), "5m");
    }

    #[test]
    fn humanize_minutes_and_seconds() {
        assert_eq!(humanize(Duration::from_secs(150)), "2m 30s");
        assert_eq!(humanize(Duration::from_secs(75)), "1m 15s");
    }

    #[test]
    fn humanize_hours() {
        assert_eq!(humanize(Duration::from_secs(3600)), "1h");
        assert_eq!(humanize(Duration::from_secs(5400)), "1h 30m");
        assert_eq!(humanize(Duration::from_secs(7290)), "2h 1m");
    }

    #[test]
    fn default_runway_short_initial() {
        // Anything below 2h gets the short default.
        assert_eq!(
            default_runway(Duration::from_secs(60)),
            Duration::from_secs(600)
        );
        assert_eq!(
            default_runway(Duration::from_secs(3600)),
            Duration::from_secs(600)
        );
        assert_eq!(
            default_runway(Duration::from_secs(2 * 3600 - 1)),
            Duration::from_secs(600),
        );
    }

    #[test]
    fn default_runway_long_initial() {
        // 2h on the dot or longer gets the long default.
        assert_eq!(
            default_runway(Duration::from_secs(2 * 3600)),
            Duration::from_secs(20 * 60),
        );
        assert_eq!(
            default_runway(Duration::from_secs(4 * 3600)),
            Duration::from_secs(20 * 60),
        );
    }

    // ── decay() edge cases ────────────────────────────────────────────────

    #[test]
    fn decay_factor_near_one_barely_shrinks() {
        // factor=0.99 against 100s -> ~99s (within 1ms tolerance of float math).
        let got = decay(Duration::from_secs(100), 0.99);
        let want = Duration::from_secs(99);
        let diff = if got > want { got - want } else { want - got };
        assert!(
            diff < Duration::from_millis(1),
            "decay(100s, 0.99) = {got:?}, expected ~99s",
        );
    }

    #[test]
    fn decay_factor_near_zero_collapses() {
        // factor=0.01 against 100s -> ~1s (within 1ms).
        let got = decay(Duration::from_secs(100), 0.01);
        let want = Duration::from_secs(1);
        let diff = if got > want { got - want } else { want - got };
        assert!(
            diff < Duration::from_millis(1),
            "decay(100s, 0.01) = {got:?}, expected ~1s",
        );
    }

    #[test]
    fn decay_preserves_subsecond_resolution() {
        // 1s × 0.5 should give 500ms, not be rounded to 0.
        assert_eq!(
            decay(Duration::from_secs(1), 0.5),
            Duration::from_millis(500),
        );
    }

    #[test]
    fn decay_handles_long_durations() {
        // 8 hours × 0.5 should give exactly 4 hours, no precision drift.
        assert_eq!(
            decay(Duration::from_secs(8 * 3600), 0.5),
            Duration::from_secs(4 * 3600),
        );
    }

    // ── humanize() edge cases ─────────────────────────────────────────────

    #[test]
    fn humanize_zero() {
        assert_eq!(humanize(Duration::ZERO), "0s");
    }

    #[test]
    fn humanize_exactly_one_minute() {
        // 60s should render as "1m", not "60s" — minute boundary takes priority.
        assert_eq!(humanize(Duration::from_secs(60)), "1m");
    }

    #[test]
    fn humanize_exactly_one_hour() {
        // 3600s should render as "1h", not "60m".
        assert_eq!(humanize(Duration::from_secs(3600)), "1h");
    }

    #[test]
    fn humanize_drops_seconds_above_one_hour() {
        // Above 1h, sub-minute precision is intentionally dropped.
        // 1h 0m 30s -> "1h", not "1h 0m 30s".
        assert_eq!(humanize(Duration::from_secs(3630)), "1h");
        // 1h 5m 45s -> "1h 5m" (seconds dropped).
        assert_eq!(humanize(Duration::from_secs(3945)), "1h 5m");
    }

    #[test]
    fn humanize_just_under_one_minute() {
        // 59s stays in seconds.
        assert_eq!(humanize(Duration::from_secs(59)), "59s");
    }

    #[test]
    fn humanize_just_under_one_hour() {
        // 59m 59s stays as "59m 59s" — no rounding up to "1h".
        assert_eq!(humanize(Duration::from_secs(3599)), "59m 59s");
    }

    // ── plan() — full algorithm simulation ────────────────────────────────

    /// Helper: build a Plan from human-readable durations.
    fn p(initial: &str, runway: &str, decay: f64, floor: &str) -> Plan {
        plan(
            durparse::parse(initial).unwrap(),
            durparse::parse(runway).unwrap(),
            decay,
            durparse::parse(floor).unwrap(),
        )
    }

    #[test]
    fn plan_single_alert_when_runway_decays_to_floor_immediately() {
        // nudge 30s 10s -> first decay 10s × 0.5 = 5s = floor -> stop.
        // Only the initial wait is scheduled; the alert it precedes is final.
        let plan = p("30s", "10s", 0.5, "5s");
        assert_eq!(plan.alert_count(), 1);
        assert_eq!(plan.waits, vec![Duration::from_secs(30)]);
    }

    #[test]
    fn plan_25m_with_default_runway_matches_readme() {
        // README claim: nudge 25m -> ~35 minutes total, ending in lock.
        let plan = p("25m", "10m", 0.5, "5s");
        assert_eq!(plan.alert_count(), 7);

        // First wait is the initial duration.
        assert_eq!(plan.waits[0], Duration::from_secs(25 * 60));

        // Tail values: 5m, 2m30s, 1m15s, 37.5s, 18.75s, 9.375s.
        let tail_secs: Vec<f64> = plan.waits[1..].iter().map(|d| d.as_secs_f64()).collect();
        assert_eq!(tail_secs, vec![300.0, 150.0, 75.0, 37.5, 18.75, 9.375]);

        // Total ~ 25m + 9m50s ~ 34m50s.
        let total = plan.total();
        assert!(
            total >= Duration::from_secs(34 * 60 + 45) && total <= Duration::from_secs(35 * 60 + 5),
            "expected total ~35min, got {total:?}",
        );
    }

    #[test]
    fn plan_2h_15m_train_example() {
        // README claim: nudge 2h 15m -> alert at 2h, then ~15m of nudges.
        let plan = p("2h", "15m", 0.5, "5s");
        assert_eq!(plan.waits[0], Duration::from_secs(2 * 3600));

        // Tail (excluding initial) should sum to roughly the runway value.
        // Geometric series: 15m × Σ(0.5^k for k=1..) ≈ 15m as floor approaches 0.
        // With a 5s floor we lose the smallest terms, so it's a bit less.
        let tail: Duration = plan.waits[1..].iter().sum();
        let runway = Duration::from_secs(15 * 60);
        // Tail is ~runway × (1 - epsilon). Allow generous bounds.
        assert!(
            tail >= runway / 2 && tail <= runway,
            "expected tail in (runway/2, runway], got {tail:?} vs runway={runway:?}",
        );

        // Total should land within ~30s of (initial + runway).
        let total = plan.total();
        let expected = Duration::from_secs(2 * 3600 + 15 * 60);
        let diff = if total > expected {
            total - expected
        } else {
            expected - total
        };
        assert!(
            diff < Duration::from_secs(30),
            "total {total:?} expected ~{expected:?} (diff {diff:?})",
        );
    }

    #[test]
    fn plan_first_alert_always_at_initial_duration() {
        // No matter what the runway is, the very first wait equals initial.
        for (init, run) in [("1m", "30s"), ("2h", "20m"), ("5m", "20m"), ("30s", "10s")] {
            let plan = p(init, run, 0.5, "5s");
            assert_eq!(plan.waits[0], durparse::parse(init).unwrap());
        }
    }

    #[test]
    fn plan_strictly_decreasing_after_initial() {
        // Each post-initial wait must be shorter than the one before it
        // (decay factor < 1).
        let plan = p("1h", "30m", 0.5, "5s");
        for window in plan.waits[1..].windows(2) {
            assert!(window[0] > window[1], "tail not decreasing: {window:?}");
        }
    }

    #[test]
    fn plan_terminates_for_high_decay_factor() {
        // factor=0.99 means tail shrinks slowly. Must still terminate before
        // the test times out (5s floor is well above 0).
        let plan = p("10m", "1m", 0.99, "5s");
        assert!(plan.alert_count() < 1000, "plan grew too large");
        assert!(plan.alert_count() > 1);
    }

    #[test]
    fn plan_floor_at_or_above_runway_yields_one_alert() {
        // If floor is so high that the first decay step is already below it,
        // we get a single alert (the initial one) and lock.
        let plan = p("5m", "10s", 0.5, "10s");
        assert_eq!(plan.alert_count(), 1);
        assert_eq!(plan.waits, vec![Duration::from_secs(300)]);
    }

    #[test]
    fn plan_subtitle_humanization_is_nonempty() {
        // For every scheduled wait, the humanized form must be non-empty —
        // it's what gets shown to the user as the alert subtitle.
        let plan = p("1h", "20m", 0.5, "5s");
        for w in &plan.waits {
            assert!(!humanize(*w).is_empty(), "empty subtitle for {w:?}");
        }
    }

    #[test]
    fn plan_long_initial_short_runway_useful_pattern() {
        // The "don't miss the train" pattern: long initial, short runway.
        // Should produce one alert at the deadline, then a couple of close-
        // together nudges, then lock.
        let plan = p("2h", "1m", 0.5, "5s");
        assert!(plan.alert_count() >= 2);
        assert_eq!(plan.waits[0], Duration::from_secs(2 * 3600));

        // Tail should sum to less than runway (it's a partial geometric
        // series cut off at the floor).
        let tail: Duration = plan.waits[1..].iter().sum();
        assert!(tail <= Duration::from_secs(60));
    }
}
