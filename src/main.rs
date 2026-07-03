//! Entrypoint: parse CLI, run the timer loop, lock the screen on exhaustion.
//!
//! The timer loop is deliberately plain blocking sleep + arithmetic. Each
//! alert iteration calls into the [`UiBackend`] trait (no iced types leak
//! into the timer logic), so adding a future X11 / macOS backend means
//! adding one file under `src/ui/` and selecting it here.

use std::io::Write;
use std::process::{self, Command};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::Duration;

use clap::Parser;

use nudge::duration as durparse;
use nudge::plan::{default_runway, humanize, plan};
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
///
/// Anchored to a wall-clock deadline instead of counting fixed slices:
/// `thread::sleep` may oversleep each slice, and over a multi-hour timer
/// those errors add up to seconds of drift. Re-deriving the remaining time
/// from `Instant::now()` each iteration keeps the total wait accurate.
fn sleep_cancellable(dur: Duration, cancel: &AtomicBool) {
    let slice = Duration::from_millis(100);
    let deadline = std::time::Instant::now() + dur;
    while !cancel.load(Ordering::Relaxed) {
        let remaining = deadline.saturating_duration_since(std::time::Instant::now());
        if remaining.is_zero() {
            break;
        }
        thread::sleep(remaining.min(slice));
    }
}

fn beep() {
    let mut out = std::io::stdout().lock();
    let _ = out.write_all(b"\x07");
    let _ = out.flush();
}

/// Return true if `program` exists somewhere on PATH.
fn program_exists(program: &str) -> bool {
    std::env::var_os("PATH")
        .map(|path| std::env::split_paths(&path).any(|dir| dir.join(program).is_file()))
        .unwrap_or(false)
}

/// Spawn `argv[0]` with the remaining elements as arguments and leave it
/// running detached. Returns Ok(()) if the process was successfully spawned.
///
/// Use for lockers that block until the user unlocks (hyprlock, swaylock,
/// waylock) — waiting on them would hang nudge forever.
fn spawn_detached(argv: &[&str]) -> Result<(), ()> {
    if argv.is_empty() || !program_exists(argv[0]) {
        return Err(());
    }
    Command::new(argv[0])
        .args(&argv[1..])
        .spawn()
        .map(|_| ())
        .map_err(|_| ())
}

/// Run `argv` to completion and report whether it exited successfully.
///
/// Use for one-shot lock commands (loginctl, dbus calls, xdg-screensaver):
/// merely spawning them proves nothing — e.g. `loginctl` exists on every
/// systemd machine and spawns fine even when no locker is registered, in
/// which case it exits non-zero and the next strategy must be tried.
fn run_ok(argv: &[&str]) -> bool {
    if argv.is_empty() || !program_exists(argv[0]) {
        return false;
    }
    Command::new(argv[0])
        .args(&argv[1..])
        .status()
        .map(|st| st.success())
        .unwrap_or(false)
}

/// Lock the screen using the best available method.
///
/// Strategy (first success wins):
///
/// 1. `loginctl lock-session` — works on GNOME (Wayland/X11), KDE Plasma
///    (Wayland), niri, sway, and any DE/WM that registers a locker with
///    logind via the `Lock` D-Bus signal. This is the most portable option.
///
/// 2. DE/WM-specific binaries discovered from `XDG_CURRENT_DESKTOP` and
///    `XDG_SESSION_DESKTOP`:
///    - Hyprland → `hyprlock`
///    - KDE → `loginctl lock-session` already covers it; `qdbus` fallback
///    - GNOME → `gnome-screensaver-command --lock`
///
/// 3. Generic Wayland screen-locker binaries found on PATH (in priority
///    order): `hyprlock`, `swaylock`, `waylock`.
///
/// 4. `xdg-screensaver lock` — legacy X11/mixed fallback.
///
/// If every attempt fails the error from the last attempt is returned.
fn spawn_locker() -> Result<(), String> {
    // 1. loginctl — portable across most modern DEs on systemd. One-shot:
    //    wait for it and check the exit status, because it spawns fine on
    //    every systemd machine even when locking is impossible.
    if run_ok(&["loginctl", "lock-session"]) {
        return Ok(());
    }

    // 2. DE-specific, derived from environment.
    let current_desktop = std::env::var("XDG_CURRENT_DESKTOP")
        .unwrap_or_default()
        .to_ascii_lowercase();
    let session_desktop = std::env::var("XDG_SESSION_DESKTOP")
        .unwrap_or_default()
        .to_ascii_lowercase();
    let desktop = format!("{current_desktop}:{session_desktop}");

    if desktop.contains("hyprland") && spawn_detached(&["hyprlock"]).is_ok() {
        return Ok(());
    }
    if desktop.contains("kde") || desktop.contains("plasma") {
        // D-Bus paths used by KDE when loginctl didn't work. Both one-shot.
        if run_ok(&[
            "qdbus",
            "org.freedesktop.ScreenSaver",
            "/ScreenSaver",
            "Lock",
        ]) {
            return Ok(());
        }
        if run_ok(&[
            "dbus-send",
            "--session",
            "--dest=org.freedesktop.ScreenSaver",
            "--type=method_call",
            "/ScreenSaver",
            "org.freedesktop.ScreenSaver.Lock",
        ]) {
            return Ok(());
        }
    }
    if (desktop.contains("gnome") || desktop.contains("unity"))
        && run_ok(&["gnome-screensaver-command", "--lock"])
    {
        return Ok(());
    }

    // 3. Generic Wayland lockers present on PATH. These block until the
    //    user unlocks, so they must be spawned detached, not waited on.
    for binary in &["hyprlock", "swaylock", "waylock"] {
        if spawn_detached(&[binary]).is_ok() {
            return Ok(());
        }
    }

    // 4. Legacy xdg-screensaver. One-shot.
    if run_ok(&["xdg-screensaver", "lock"]) {
        return Ok(());
    }

    Err("could not find a screen locker; tried loginctl, hyprlock, swaylock, waylock, gnome-screensaver-command, xdg-screensaver. Install one or ensure loginctl lock-session works.".into())
}

fn run(cfg: Config, cancel: Arc<AtomicBool>) -> Result<(), String> {
    let ui = IcedLayerShellUi::new();
    let plan = plan(cfg.initial, cfg.runway, cfg.decay, cfg.floor);

    // Walk the plan: for each scheduled wait, sleep then alert. The last
    // alert in the plan is the final one — after it, lock and exit.
    let last_idx = plan.waits.len().saturating_sub(1);
    for (idx, wait) in plan.waits.iter().enumerate() {
        sleep_cancellable(*wait, &cancel);
        if cancel.load(Ordering::Relaxed) {
            return Ok(());
        }

        let is_final = idx == last_idx;
        // Subtitle for the alert about to fire: either how long until the
        // *next* alert, or "locking screen" if this is the last one.
        let subtitle: String = if is_final {
            "locking screen".into()
        } else {
            // The next sleep's duration tells the user when to expect the
            // next nudge. plan.waits[idx + 1] is the post-alert sleep.
            format!("next nudge in {}", humanize(plan.waits[idx + 1]))
        };

        if cfg.beep {
            beep();
        }
        ui.alert(&cfg.message, Some(&subtitle), cfg.alert_duration)
            .map_err(|e| format!("alert failed: {e}"))?;
        if cancel.load(Ordering::Relaxed) {
            return Ok(());
        }

        if is_final {
            spawn_locker()?;
            return Ok(());
        }
    }
    // Unreachable in practice: plan() always produces at least one wait
    // (the initial duration), and the loop returns on the final alert.
    // Treat reaching here as a no-op rather than panicking.
    Ok(())
}

/// Install SIGINT/SIGTERM handlers.
///
/// Returns (cancel, signal): `cancel` is polled by the timer loop;
/// `signal` records which signal arrived so main() can exit with the
/// conventional 128+N code (130 for SIGINT, 143 for SIGTERM), as the
/// spec in the README promises.
fn install_signal_handlers() -> (Arc<AtomicBool>, Arc<AtomicUsize>) {
    use signal_hook::consts::{SIGINT, SIGTERM};
    use signal_hook::flag;
    let cancel = Arc::new(AtomicBool::new(false));
    let signal = Arc::new(AtomicUsize::new(0));
    // register_usize stores the signal number; register flips the cancel
    // flag. Order matters: record the signal before cancel becomes visible.
    flag::register_usize(SIGINT, Arc::clone(&signal), SIGINT as usize).expect("register SIGINT");
    flag::register_usize(SIGTERM, Arc::clone(&signal), SIGTERM as usize).expect("register SIGTERM");
    flag::register(SIGINT, Arc::clone(&cancel)).expect("register SIGINT");
    flag::register(SIGTERM, Arc::clone(&cancel)).expect("register SIGTERM");
    (cancel, signal)
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

    let (cancel, signal) = install_signal_handlers();

    match run(cfg, Arc::clone(&cancel)) {
        Ok(()) => {
            if cancel.load(Ordering::Relaxed) {
                // Conventional 128+N; falls back to 143 if the signal
                // number was somehow not recorded.
                let sig = signal.load(Ordering::Relaxed);
                process::exit(if sig == 0 {
                    EXIT_SIGTERM
                } else {
                    128 + sig as i32
                });
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

    #[test]
    fn program_exists_finds_sh() {
        // `sh` is present on every Unix system; use it as a canary.
        assert!(program_exists("sh"));
    }

    #[test]
    fn program_exists_rejects_nonexistent() {
        assert!(!program_exists("__nudge_nonexistent_binary_xyz__"));
    }

    #[test]
    fn spawn_detached_fails_for_nonexistent() {
        // A binary that definitely doesn't exist must return Err without
        // panicking.
        assert!(spawn_detached(&["__nudge_nonexistent_binary_xyz__"]).is_err());
    }

    #[test]
    fn spawn_detached_empty_argv_is_err() {
        assert!(spawn_detached(&[]).is_err());
    }

    #[test]
    fn run_ok_reports_exit_status() {
        // `true` exits 0, `false` exits 1 — run_ok must distinguish them,
        // not just report "spawned fine".
        assert!(run_ok(&["true"]));
        assert!(!run_ok(&["false"]));
    }

    #[test]
    fn run_ok_nonexistent_and_empty() {
        assert!(!run_ok(&["__nudge_nonexistent_binary_xyz__"]));
        assert!(!run_ok(&[]));
    }
}
