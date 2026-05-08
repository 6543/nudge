# nudge

A red-screen kitchen timer for people who lose hours to their PC. You set a duration, you keep working, and when the time is up nudge starts gently shouting at you until it locks the screen.

Built for ADHD self-management on Wayland (Hyprland and friends). It is harder to ignore than a notification but never grabs your input — you can finish the sentence you're typing, then walk away.

## What it looks like

```sh
nudge 25m -m "POMODORO DONE"
```

Wait 25 minutes. Red flash with "POMODORO DONE". Sleep 5 minutes. Red flash. Sleep 2½ minutes. Red flash. Each gap halves until it gets short enough that the screen just locks. About 35 minutes from start to lock.

```sh
nudge 2h 15m -m "leave for the train"
```

Wait 2 hours. First red flash. Then ~15 minutes of escalating nudges before the screen locks. Use this when you have a hard deadline and want a fixed warning window.

```sh
nudge 90m
```

Wait 90 minutes, then the default 10-minute tail of nudges, then lock. Good catch-all for "I want to stop working in roughly an hour and a half."

```sh
nudge 30s 10s
```

The fastest e2e test you can run. Wait 30s, get a couple of quick nudges, lock. Use this to confirm everything works before relying on it.

## Why

Passive notifications are too easy to dismiss when you're in flow. A full-screen red flash is hard to miss but doesn't break what you're doing — your keyboard still works, your mouse still works, you finish what you were doing and stand up.

The escalating cadence mirrors how a real person would nag you: a polite check-in, then a firmer one, then "seriously, leave," then it makes the decision for you and locks the screen.

## Install

### Nix flake

```sh
nix run github:6543/nudge -- 25m
```

Or in your system / home-manager config:

```nix
inputs.nudge.url = "github:6543/nudge";
# then pull `nudge.packages.${system}.default`
```

### From source

```sh
git clone https://github.com/6543/nudge
cd nudge
nix develop
cargo build --release
./target/release/nudge 5m
```

You'll need a screen locker available on `PATH`. `nudge` auto-detects in order: `loginctl lock-session` (GNOME, KDE, niri, sway, …), then `hyprlock`, `swaylock`, `waylock`, `gnome-screensaver-command`, `xdg-screensaver`.

## Usage

```
nudge <duration> [runway] [flags]
```

`<duration>` is how long until the first alert. `[runway]` is optional and controls how much nudging happens between the first alert and the lock — see [Runway](#runway).

### Duration format

Compound units, integer values, ordered `h → m → s`:

| Example     | Meaning            |
| ----------- | ------------------ |
| `30s`       | 30 seconds         |
| `5m`        | 5 minutes          |
| `2h`        | 2 hours            |
| `1h30m`     | 1 hour 30 minutes  |
| `2m30s`     | 2 minutes 30 sec   |
| `1h30m45s`  | full house         |

### Runway

The runway is the total time the nudging tail lasts after the first alert. Whatever you put there is roughly how long you have left before the lock kicks in.

```sh
nudge 2h         # alert at 2h, then ~20m of decaying nudges, then lock
nudge 2h 15m     # alert at 2h, then ~15m of decaying nudges, then lock
nudge 30m 5m     # alert at 30m, then ~5m of decaying nudges, then lock
```

If you don't pass a runway, the default depends on the initial duration:

| Initial duration | Default runway |
| ---------------- | -------------- |
| `< 2h`           | `10m`          |
| `≥ 2h`           | `20m`          |

A runway longer than the initial duration is allowed (`nudge 5m 30m`) but probably not what you want.

### Flags

| Flag                          | Default          | Meaning                                            |
| ----------------------------- | ---------------- | -------------------------------------------------- |
| `-d, --decay <FLOAT>`         | `0.5`            | Each gap is multiplied by this. `0 < x < 1`.       |
| `-f, --floor <DURATION>`      | `5s`             | When the next gap would be ≤ this, lock and exit.  |
| `-m, --message <STRING>`      | `"time expired"` | Big text on the red overlay.                       |
| `-D, --alert-duration <DUR>`  | `2s`             | How long each red flash stays visible.             |
| `-b, --beep`                  | off              | Print BEL (`\x07`) on each alert.                  |
| `-h, --help`                  | —                | Print help.                                        |
| `-V, --version`               | —                | Print version.                                     |

### Shell aliases

The whole tool is meant to be aliased:

```sh
alias work='nudge 25m -m "POMODORO DONE"'
alias deep='nudge 1h30m -m "stand up. drink water."'
alias train='nudge 2h 15m -m "LEAVE FOR THE TRAIN"'
```

## Design decisions

- **No input grab.** The overlay is purely visual. You can keep typing. The flash itself is the nudge, not coercion.
- **Beep off by default.** Audio is intrusive in shared spaces. Opt-in via `-b`.
- **No daemon, no IPC, no config file.** It's a CLI tool. You launch it, it runs, it dies.
- **Multiple instances allowed.** Run two `nudge`s, get two independent schedules. Your decision.
- **Wayland layer-shell only.** The UI is split behind a backend trait, so X11 / macOS support is a "someone implements one trait" away, but it's not done now because it's not needed now.
- **Locker auto-detection.** `nudge` tries lockers in this order: `loginctl lock-session` (works on GNOME, KDE Plasma, niri, sway, and any DE that registers a locker with logind), then DE-specific binaries derived from `XDG_CURRENT_DESKTOP`/`XDG_SESSION_DESKTOP` (hyprlock for Hyprland, qdbus/dbus-send for KDE, gnome-screensaver-command for GNOME), then generic Wayland lockers found on PATH (`hyprlock`, `swaylock`, `waylock`), and finally `xdg-screensaver lock` as a legacy fallback. The first that succeeds is used.

## For the technically interested

<details>
<summary>Architecture</summary>

```
src/
  main.rs        cli parse, timer loop, signal handling, lock spawn
  duration.rs    "1h30m" → std::time::Duration parser
  ui/
    mod.rs       UiBackend trait
    iced.rs      Wayland layer-shell impl using iced + iced_layershell
  alert.rs       AlertApp — minimal iced Application showing one alert, then exits
```

The timer core is plain blocking sleep + arithmetic. Each alert spawns a fresh iced runtime that blocks until the alert duration elapses, then returns. The iced runtime is not kept alive between alerts; the `UiBackend` trait method is synchronous from the timer loop's perspective. This keeps the abstraction genuinely portable for future X11 / macOS backends.

</details>

<details>
<summary>Spec</summary>

### Invocation
```
nudge <duration> [runway] [flags]
```

### Behavior
1. Parse args; validate. Bad input → stderr + exit 2.
2. Sleep `<duration>`.
3. Show red overlay covering all outputs for `<alert-duration>`. Centered `<message>` and a smaller subtitle showing the next wait, or `"locking screen"` on the final alert. (Beep if `-b`.)
4. Set `wait = runway` after the first alert; thereafter `wait = previous_wait * decay`.
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
- Daemon mode
- Multi-instance coordination
- Persistence across reboot
- Audio beyond BEL
- Notification daemon integration
- Config file (use shell aliases)
- Input grab

</details>

<details>
<summary>Build / dev</summary>

```sh
nix develop                          # enter dev shell
cargo run -- 30s 10s                 # quickest e2e test
cargo test                           # unit tests
cargo clippy -- -D warnings
nix flake check                      # fmt + clippy in CI
```

CI runs on Woodpecker — see `.woodpecker.yaml`.

</details>

## License

[MIT](LICENSE)
