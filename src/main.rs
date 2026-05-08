//! Entrypoint: parse CLI, run the timer loop, lock the screen on exhaustion.
//!
//! The timer loop is deliberately plain blocking sleep + arithmetic. Each
//! alert iteration calls into the [`UiBackend`] trait (no iced types leak
//! into the timer logic), so adding a future X11 / macOS backend means
//! adding one file under `src/ui/` and selecting it here.

use std::io::Write;
use std::process::{self, Command};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::Duration;

use clap::Parser;

use nudge::duration as durparse;
use nudge::ui::iced::IcedLayerShellUi;
use nudge::ui::UiBackend;

const EXIT_USAGE: i32 = 2;
const EXIT_RUNTIME: i32 = 1;
const EXIT_SIGTERM: i32 = 143;

/// ADHD-friendly self-nudge timer for Wayland.
///
/// Sleeps for DURATION, flashes a red full-screen overlay, then waits a
/// shorter time and flashes again. Each cycle the wait is multiplied by
/// --decay. When the next wait would be ≤ --floor, the screen is locked
/// and the program exits.
#[derive(Parser, Debug)]
#[command(name = "nudge", version, about, long_about = None)]
struct Cli {
    /// Initial duration before the first alert. Format: 1h30m, 5m, 30s, ...
    #[arg(value_name = "DURATION")]
    duration: String,

    /// Optional second positional argument: total nudging "runway" after the
    /// first alert. The first post-alert wait is set to runway/2, then halved
    /// each cycle, so the geometric tail sums to ~runway. If omitted, defaults
    /// to 20m for initial durations >= 2h, otherwise 10m.
    ///
    /// Example: `nudge 2h 15m` -> alert at 2h, then ~15m of decaying nudges
    /// before the screen locks. Useful for "don't miss the 14:15 train" type
    /// reminders where you want a fixed warning window before the lock.
    #[arg(value_name = "RUNWAY")]
    runway: Option<String>,

    /// Decay factor applied to the wait between alerts. Must be 0 < x < 1.
    #[arg(short, long, default_value_t = 0.5)]
    decay: f64,

    /// Floor for the wait between alerts. When next wait ≤ floor, lock and exit.
    #[arg(short, long, default_value = "5s")]
    floor: String,

    /// Message displayed on the red overlay.
    #[arg(short, long, default_value = "time expired")]
    message: String,

    /// How long the red overlay stays visible per alert.
    #[arg(short = 'D', long, default_value = "2s")]
    alert_duration: String,

    /// Print BEL (\x07) on each alert. Terminal/WM decides what that does.
    #[arg(short, long)]
    beep: bool,
}

struct Config {
    initial: Duration,
    runway: Duration,
    decay: f64,
    floor: Duration,
    message: String,
    alert_duration: Duration,
    beep: bool,
}

/// Default runway when no second positional arg is given.
/// >= 2h initial -> 20m runway; otherwise 10m.
fn default_runway(initial: Duration) -> Duration {
    const TWO_HOURS: Duration = Duration::from_secs(2 * 3600);
    if initial >= TWO_HOURS {
        Duration::from_secs(20 * 60)
    } else {
        Duration::from_secs(10 * 60)
    }
}

impl Config {
    fn from_cli(cli: Cli) -> Result<Self, String> {
        if !(cli.decay > 0.0 && cli.decay < 1.0) {
            return Err(format!("--decay must be in (0, 1), got {}", cli.decay));
        }
        let initial =
            durparse::parse(&cli.duration).map_err(|e| format!("invalid <DURATION>: {e}"))?;
        let floor = durparse::parse(&cli.floor).map_err(|e| format!("invalid --floor: {e}"))?;
        let alert_duration = durparse::parse(&cli.alert_duration)
            .map_err(|e| format!("invalid --alert-duration: {e}"))?;
        let runway = match cli.runway.as_deref() {
            Some(s) => durparse::parse(s).map_err(|e| format!("invalid <RUNWAY>: {e}"))?,
            None => default_runway(initial),
        };
        if initial <= floor {
            return Err(format!(
                "<DURATION> ({:?}) must be greater than --floor ({:?})",
                initial, floor
            ));
        }
        if runway <= floor {
            return Err(format!(
                "<RUNWAY> ({:?}) must be greater than --floor ({:?})",
                runway, floor
            ));
        }
        Ok(Self {
            initial,
            runway,
            decay: cli.decay,
            floor,
            message: cli.message,
            alert_duration,
            beep: cli.beep,
        })
    }
}

/// Cancellable sleep. Wakes early if `cancel` is set.
fn sleep_cancellable(dur: Duration, cancel: &AtomicBool) {
    // Poll in 100ms slices. Coarse but fine for a tool that sleeps minutes.
    let slice = Duration::from_millis(100);
    let mut remaining = dur;
    while !cancel.load(Ordering::Relaxed) && !remaining.is_zero() {
        let step = remaining.min(slice);
        thread::sleep(step);
        remaining = remaining.saturating_sub(step);
    }
}

/// Reduce a Duration by a factor in (0,1). Saturates at zero.
fn decay(prev: Duration, factor: f64) -> Duration {
    let secs = prev.as_secs_f64() * factor;
    if secs.is_finite() && secs >= 0.0 {
        Duration::from_secs_f64(secs)
    } else {
        Duration::ZERO
    }
}

fn beep() {
    let mut out = std::io::stdout().lock();
    let _ = out.write_all(b"\x07");
    let _ = out.flush();
}

fn spawn_locker() -> Result<(), String> {
    // Hardcoded for v1, see README "Design decisions". Spawn detached and
    // return immediately so the parent can exit.
    Command::new("hyprlock")
        .spawn()
        .map(|_child| ())
        .map_err(|e| format!("failed to spawn hyprlock: {e}"))
}

/// Format a Duration as a short human string: "2m 30s", "45s", "1h 5m".
/// Seconds are dropped above 1 hour (granularity isn't useful at that range).
fn humanize(d: Duration) -> String {
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

fn run(cfg: Config, cancel: Arc<AtomicBool>) -> Result<(), String> {
    let ui = IcedLayerShellUi::new();

    // Phase 1: initial wait.
    sleep_cancellable(cfg.initial, &cancel);
    if cancel.load(Ordering::Relaxed) {
        return Ok(());
    }

    // The decay loop starts from `runway`, not `initial`. This makes the
    // first post-alert wait `runway * decay` (= runway/2 with default decay),
    // which gives a geometric tail that sums to ~runway. Decoupling the
    // initial wait from the tail length is the whole point of the feature:
    // a 2h timer no longer means a 1h gap until the second nudge.
    let mut wait = cfg.runway;
    loop {
        // Compute what comes next BEFORE showing the alert, so the alert
        // can display it as a subtitle.
        let next = decay(wait, cfg.decay);
        let is_final = next <= cfg.floor;
        let subtitle: String = if is_final {
            "locking screen".into()
        } else {
            format!("next nudge in {}", humanize(next))
        };

        // Phase 2: alert.
        if cfg.beep {
            beep();
        }
        ui.alert(&cfg.message, Some(&subtitle), cfg.alert_duration)
            .map_err(|e| format!("alert failed: {e}"))?;
        if cancel.load(Ordering::Relaxed) {
            return Ok(());
        }

        // Phase 3 / 4: lock + exit, or decay + sleep again.
        if is_final {
            spawn_locker()?;
            return Ok(());
        }
        wait = next;
        sleep_cancellable(wait, &cancel);
        if cancel.load(Ordering::Relaxed) {
            return Ok(());
        }
    }
}

fn install_signal_handlers() -> Arc<AtomicBool> {
    let cancel = Arc::new(AtomicBool::new(false));
    // Both SIGINT and SIGTERM flip the same flag; the timer loop polls it
    // in sleep_cancellable. We don't distinguish which signal arrived, so
    // the exit code in main() is always 143 for any cancelled run. The
    // shell's "$?" reporting 130 vs 143 only matters when the parent
    // process is monitoring it — for a personal CLI, KISS wins.
    use signal_hook::consts::{SIGINT, SIGTERM};
    use signal_hook::flag;
    flag::register(SIGINT, Arc::clone(&cancel)).expect("register SIGINT");
    flag::register(SIGTERM, Arc::clone(&cancel)).expect("register SIGTERM");
    cancel
}

fn main() {
    let cli = Cli::parse();
    let cfg = match Config::from_cli(cli) {
        Ok(c) => c,
        Err(msg) => {
            eprintln!("nudge: {msg}");
            process::exit(EXIT_USAGE);
        }
    };

    let cancel = install_signal_handlers();

    match run(cfg, Arc::clone(&cancel)) {
        Ok(()) => {
            if cancel.load(Ordering::Relaxed) {
                process::exit(EXIT_SIGTERM);
            }
            process::exit(0);
        }
        Err(msg) => {
            eprintln!("nudge: {msg}");
            process::exit(EXIT_RUNTIME);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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

    // ── Config::from_cli validation ───────────────────────────────────────

    /// Build a Cli with defaults for every field except those overridden,
    /// to keep test bodies focused on the field being exercised.
    fn cli(duration: &str) -> Cli {
        Cli {
            duration: duration.into(),
            runway: None,
            decay: 0.5,
            floor: "5s".into(),
            message: "test".into(),
            alert_duration: "2s".into(),
            beep: false,
        }
    }

    #[test]
    fn config_accepts_canonical_examples() {
        // The four examples from the README.
        let mut c = cli("25m");
        assert!(Config::from_cli(c).is_ok());
        c = cli("2h");
        c.runway = Some("15m".into());
        assert!(Config::from_cli(c).is_ok());
        c = cli("90m");
        assert!(Config::from_cli(c).is_ok());
        c = cli("30s");
        c.runway = Some("10s".into());
        assert!(Config::from_cli(c).is_ok());
    }

    #[test]
    fn config_rejects_decay_out_of_range() {
        let mut c = cli("5m");
        c.decay = 0.0;
        assert!(Config::from_cli(c).is_err());
        let mut c = cli("5m");
        c.decay = 1.0;
        assert!(Config::from_cli(c).is_err());
        let mut c = cli("5m");
        c.decay = 1.5;
        assert!(Config::from_cli(c).is_err());
        let mut c = cli("5m");
        c.decay = -0.5;
        assert!(Config::from_cli(c).is_err());
    }

    #[test]
    fn config_rejects_initial_at_or_below_floor() {
        // initial == floor is invalid (would alert and lock instantly).
        let mut c = cli("5s");
        c.floor = "5s".into();
        assert!(Config::from_cli(c).is_err());
        let mut c = cli("3s");
        c.floor = "5s".into();
        assert!(Config::from_cli(c).is_err());
    }

    #[test]
    fn config_rejects_runway_at_or_below_floor() {
        // runway must be strictly above floor or the loop locks immediately
        // after the first alert.
        let mut c = cli("10m");
        c.runway = Some("5s".into());
        c.floor = "5s".into();
        assert!(Config::from_cli(c).is_err());
        let mut c = cli("10m");
        c.runway = Some("3s".into());
        c.floor = "5s".into();
        assert!(Config::from_cli(c).is_err());
    }

    #[test]
    fn config_allows_runway_greater_than_initial() {
        // Documented as allowed: "their problem". Exercise it.
        let mut c = cli("5m");
        c.runway = Some("30m".into());
        let cfg = Config::from_cli(c).expect("should be allowed");
        assert!(cfg.runway > cfg.initial);
    }

    #[test]
    fn config_rejects_unparseable_durations() {
        // Bad initial.
        let c = cli("not-a-duration");
        assert!(Config::from_cli(c).is_err());
        // Bad runway.
        let mut c = cli("5m");
        c.runway = Some("garbage".into());
        assert!(Config::from_cli(c).is_err());
        // Bad floor.
        let mut c = cli("5m");
        c.floor = "?".into();
        assert!(Config::from_cli(c).is_err());
        // Bad alert duration.
        let mut c = cli("5m");
        c.alert_duration = "soon".into();
        assert!(Config::from_cli(c).is_err());
    }

    #[test]
    fn config_uses_default_runway_when_omitted() {
        // <2h initial -> 10m default
        let cfg = Config::from_cli(cli("30m")).unwrap();
        assert_eq!(cfg.runway, Duration::from_secs(10 * 60));

        // >=2h initial -> 20m default
        let cfg = Config::from_cli(cli("3h")).unwrap();
        assert_eq!(cfg.runway, Duration::from_secs(20 * 60));
    }
}
