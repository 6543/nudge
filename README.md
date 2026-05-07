# nudge

ADHD-friendly self-nudge timer for Wayland. You give it a duration. After that duration it flashes a red full-screen overlay with a message. Then it waits a shorter time and does it again. Each cycle the wait shrinks (exponential decay). When the wait would drop below a configurable floor, it locks the screen and exits — forcing you away from the PC.

Inspired by the simple observation that "stop and stand up" reminders work better when they're hard to ignore.

## Status

Pre-alpha. Spec frozen, scaffolding in progress. See [Spec](#spec) below.

## Why

Built for personal use. ADHD-affected users can lose track of time on the PC for hours. A passive notification is too easy to dismiss; a full-screen red flash is harder to ignore but still non-blocking (visual only, no input grab — see [Design decisions](#design-decisions)). The escalating cadence mirrors how a real person would nag you: "in 30 minutes" → "in 15" → "ok seriously now" → forces lock.

## Install

### Nix flake

```sh
nix run github:YOUR_USER/nudge -- 1h30m
```

Or add to your system / home-manager config via `inputs.nudge.url = "github:YOUR_USER/nudge";` and pull `nudge.packages.${system}.default`.

### From source (NixOS dev shell)

```sh
git clone https://github.com/YOUR_USER/nudge
cd nudge
nix develop
cargo build --release
./target/release/nudge 5m
```

## Usage

```
nudge <duration> [flags]
```

### Duration format

Compound units, integer values:

| Example  | Meaning            |
| -------- | ------------------ |
| `30s`    | 30 seconds         |
| `5m`     | 5 minutes          |
| `2h`     | 2 hours            |
| `1h30m`  | 1 hour 30 minutes  |
| `2m30s`  | 2 minutes 30 sec   |
| `1h30m45s` | full house       |

Order must be `h → m → s`, each unit at most once.

### Flags

| Flag                          | Default          | Meaning                                         |
| ----------------------------- | ---------------- | ----------------------------------------------- |
| `-d, --decay <FLOAT>`         | `0.5`            | Decay factor. Each cycle the wait is multiplied by this. Must be `0 < x < 1`. |
| `-f, --floor <DURATION>`      | `5s`             | When the next wait would be ≤ this, stop and lock. |
| `-m, --message <STRING>`      | `"time expired"` | Text shown on the red overlay.                  |
| `-D, --alert-duration <DUR>`  | `1s`             | How long the red overlay is visible per alert.  |
| `-b, --beep`                  | off              | Print BEL (`\x07`) on each alert. Your terminal/WM decides what that does. |
| `-h, --help`                  | —                | Print help.                                     |
| `-V, --version`               | —                | Print version.                                  |

### Example

```sh
nudge 5m -d 0.5 -f 5s -m "stand up dummy"
```

What happens:

```
t = 0       │ start, sleep 5m
t = 5m      │ ▓▓▓ red flash 1s ▓▓▓  "stand up dummy"
t = 5m1s    │ sleep 2m30s
t = 7m31s   │ ▓▓▓ red flash 1s ▓▓▓
t = 7m32s   │ sleep 1m15s
              ...
              next wait = 4.5s, ≤ floor → hyprlock → exit 0
```

Total ≈ 10 minutes for a 5-minute initial timer with default decay.

### Tip: pair with a shell alias

```sh
alias work='nudge 25m -m "POMODORO DONE"'
alias deep='nudge 1h30m -d 0.6 -m "stand up. drink water."'
```

## Design decisions

These are deliberate. Open an issue if you disagree, but know the tradeoffs were considered.

- **No input grab during alert.** The overlay is purely visual. You can keep typing if you really need to. Grabbing keyboard/mouse for a full second every cycle would break flow for anyone trying to finish a sentence; the flash itself is the nudge, not coercion.
- **Beep off by default.** Audio is intrusive in shared spaces. Opt-in via `-b`.
- **No daemon, no IPC, no config file.** It's a CLI tool. You launch it, it runs, it dies. State lives in the process. Reboot mid-timer = timer dies. KISS.
- **Multiple concurrent instances allowed.** If you launch `nudge` twice, you get two independent alert schedules. This is your decision to make.
- **Hard-coded `hyprlock` for v1.** A locker abstraction would be premature. If/when someone wants `swaylock` or `loginctl lock-session`, we'll add a flag.
- **Wayland layer-shell only for v1.** The UI is split behind a `UiBackend` trait, so an X11 or macOS backend can be added without touching the timer core. Not done now because it's not needed now.

## Architecture

```
src/
  main.rs        cli parse, timer loop, signal handling, lock spawn
  duration.rs    "1h30m" → std::time::Duration parser + tests
  ui/
    mod.rs       UiBackend trait
    iced.rs      IcedLayerShellUi impl (red overlay via iced_layershell)
  alert.rs       AlertApp — minimal iced Application showing one alert, then exits
```

The timer core is plain blocking sleep + arithmetic. Each alert spins up `AlertApp::run(...)`, which blocks until the alert window closes itself after `alert-duration`, then returns. This keeps the iced runtime out of the main loop and makes the `UiBackend` trait genuinely portable.

## Build / dev

```sh
nix develop                          # enter dev shell
cargo run -- 10s -d 0.5 -f 2s        # quick e2e — fires after 10s, then 5s, then locks
cargo test                           # unit tests for duration parser & decay math
cargo clippy -- -D warnings
nix flake check                      # fmt + clippy in CI
```

For the tightest dev loop:

```sh
bacon
```

## Spec

Below is the locked v1 specification. PRs that contradict this need to amend the spec first.

### Invocation
```
nudge <duration> [flags]
```

### Behavior
1. Parse args; validate. Bad input → stderr + exit 2.
2. Sleep `<duration>`.
3. Show red overlay covering all outputs for `<alert-duration>`. Center `<message>` text. (Beep if `-b`.)
4. Compute `next_wait = previous_wait * decay`.
5. If `next_wait ≤ floor` → spawn locker, detach, exit 0.
6. Else sleep `next_wait`, goto 3.

### Overlay requirements
- Covers all monitors.
- Above all windows including fullscreen.
- Pure visual — no input grab, no focus steal.
- Solid red `#ff0000` background, large centered text.

### Error handling
- Parse error → exit 2.
- Overlay create failure → stderr + exit 1.
- Lock spawn failure → stderr + exit 1.
- SIGINT / SIGTERM → tear down overlay if visible → exit 130 / 143.

### Non-goals (v1)
- ❌ Daemon mode
- ❌ Multi-instance coordination
- ❌ Persistence across reboot
- ❌ Audio beyond BEL
- ❌ Notification daemon integration
- ❌ Config file (use shell aliases)
- ❌ Input grab

## License

TBD — likely MIT or Apache-2.0.

## Acknowledgements

Flake structure adapted from [animolauncher](https://github.com/) (same author's launcher project).
