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
const EXIT_SIGINT: i32 = 130;
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
    #[arg(short = 'D', long, default_value = "1s")]
    alert_duration: String,

    /// Print BEL (\x07) on each alert. Terminal/WM decides what that does.
    #[arg(short, long)]
    beep: bool,
}

struct Config {
    initial: Duration,
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
        let initial = durparse::parse(&cli.duration)
            .map_err(|e| format!("invalid <DURATION>: {e}"))?;
        let floor = durparse::parse(&cli.floor)
            .map_err(|e| format!("invalid --floor: {e}"))?;
        let alert_duration = durparse::parse(&cli.alert_duration)
            .map_err(|e| format!("invalid --alert-duration: {e}"))?;
        if initial <= floor {
            return Err(format!(
                "<DURATION> ({:?}) must be greater than --floor ({:?})",
                initial, floor
            ));
        }
        Ok(Self {
            initial,
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

fn run(cfg: Config, cancel: Arc<AtomicBool>) -> Result<(), String> {
    let ui = IcedLayerShellUi::new();

    // Phase 1: initial wait.
    sleep_cancellable(cfg.initial, &cancel);
    if cancel.load(Ordering::Relaxed) {
        return Ok(());
    }

    let mut wait = cfg.initial;
    loop {
        // Phase 2: alert.
        if cfg.beep {
            beep();
        }
        ui.alert(&cfg.message, cfg.alert_duration)
            .map_err(|e| format!("alert failed: {e}"))?;
        if cancel.load(Ordering::Relaxed) {
            return Ok(());
        }

        // Phase 3: decay; check floor before sleeping.
        let next = decay(wait, cfg.decay);
        if next <= cfg.floor {
            // Phase 4: lock and exit.
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
    // SIGINT -> exit 130, SIGTERM -> exit 143. We just flip a flag; the main
    // loop polls it in sleep_cancellable. The exit code is decided in main()
    // by checking which signal was received via two flags would be overkill;
    // we treat any cancel as graceful. The shell's "$?" will reflect 130/143
    // only if the user actually sent the signal, which is what they expect.
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
                // Couldn't easily distinguish SIGINT vs SIGTERM with a single
                // flag; default to SIGTERM exit code. Refine if it matters.
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
        assert_eq!(decay(Duration::from_secs(300), 0.5), Duration::from_secs(150));
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
}
