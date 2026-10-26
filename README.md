# opencode-voice

A tiny, fully offline wake-word launcher for Linux. Say **"Hey, OpenCode"** and
it shows a five-second listening notification. Say a command during the
countdown, or do nothing and OpenCode opens normally.

Voice commands:

| Phrase | Action |
| --- | --- |
| “Hey, OpenCode” | Show the listening notification; open OpenCode after 5 seconds if no command is heard |
| “Hey, OpenCode, open” | Open OpenCode immediately, skipping the countdown |
| “Hey, OpenCode, continue” | Open a terminal and resume the last session with `opencode --continue` |
| “Hey, OpenCode, stop” | Interrupt the active OpenCode response immediately |
| “Hey, OpenCode, help” | Open a new terminal window with the voice command help |
| “Hey, OpenCode, never mind” | Cancel the countdown and do nothing |

Commands can be spoken during the countdown, or immediately after the wake
phrase. The notification updates every second so you can see how much time is
left. Because the wake phrase itself contains “open”, an OPEN detected while the
wake phrase is still being decoded is ignored; an “open” spoken later in the
window still opens immediately.

Built for a low-power ThinkPad T480:

- **Offline** — no cloud, no API keys, microphone audio stays on the machine.
- **Lightweight** — 3.3M-parameter sherpa-onnx keyword spotter (~20 MB model).
- **Efficient** — one worker thread, one shared 3.3M-parameter model, 16 kHz
  mono where PipeWire can provide it, and only one keyword stream active at a
  time. A tiny RMS gate skips wake-word decoding while the room is quiet, so
  idle CPU is near zero; active decoding costs roughly 4% of one CPU core
  (RTF ~0.04 on an i5-8250U). The command stream runs only during the
  five-second window.
- **Simple** — Rust binary + user systemd service. No Electron, no Python, no
  background daemon framework.

## Requirements

- Linux desktop with PipeWire, PulseAudio, or ALSA
- A supported terminal emulator: kitty, Ghostty, Alacritty, Konsole, GNOME
  Terminal, WezTerm, Ptyxis, foot, XFCE4 Terminal, Tilix, Terminator, xterm,
  or `x-terminal-emulator`. `xdg-terminal-exec` is used automatically when
  available.
- `libnotify` (`notify-send`) for the listening countdown
- OpenCode in `PATH`
- A working microphone

## Install

The release binary is built by GitHub Actions. To install the latest release,
the wake-word model, and the systemd user service:

```bash
curl -fsSL https://raw.githubusercontent.com/Peter5235252/opencode-voice/main/install.sh | bash
```

Everything is installed under your home directory; **no sudo is required**.
The script installs:

| Path | Purpose |
| --- | --- |
| `~/.local/bin/opencode-voice` | The binary |
| `~/.local/bin/opencode-voice-help` | Voice command help shown by “help” |
| `~/.local/share/opencode-voice/models/...` | Offline KWS model |
| `~/.config/systemd/user/opencode-voice.service` | Autostart user service |

Start/stop/check it with:

```bash
systemctl --user status opencode-voice
systemctl --user restart opencode-voice
journalctl --user -u opencode-voice -f
```

## Usage

```text
opencode-voice [OPTIONS]

  --model-dir <DIR>   Directory containing the sherpa-onnx KWS model
  --device <NAME>     Input device name (case-insensitive substring)
  --terminal <NAME>   Terminal emulator to use (default: auto-detect)
  --threshold <0-1>   Keyword confidence threshold (default: 0.15)
  --energy-gate <RMS> Skip wake decoding below this RMS (0 disables, default: 0.003)
  --cooldown <SECS>   Ignore repeated launches for this long (default: 4)
  --wav <FILE>        Test a WAV file instead of listening to the microphone
  --dry-run           Detect and log, but do not launch a terminal or stop OpenCode
  --list-devices      List available audio input devices and exit
  -h, --help          Print help
  -V, --version       Print version
```

Examples:

```bash
# See what the microphone options are called
opencode-voice --list-devices

# Listen on a specific microphone, without opening a terminal or stopping OpenCode
opencode-voice --device "USB" --dry-run

# Use a specific terminal emulator for this run
opencode-voice --terminal alacritty

# Check detection against a WAV file
opencode-voice --wav sample.wav
```

The WAV test also understands the listening window, so
`opencode-voice --wav hey-opencode-continue.wav` prints the wake detection and
then `CONTINUE (continue)` instead of stopping at the wake phrase.

If the default source is not the microphone you want, set it in PipeWire or
pass `--device` to the service. A substring is enough, for example
`--device "Belső hangforrás"`.

### Terminal support

The launcher auto-detects a terminal emulator. It prefers, in order:

1. `--terminal <NAME>` or `OPENCODE_VOICE_TERMINAL`
2. `$TERMINAL`, current-terminal hints (for example `KITTY_PID` or
   `ALACRITTY_SOCKET`), `$TERM_PROGRAM`, or `$TERM`
3. `xdg-terminal-exec` when it is installed and configured
4. the desktop's default (Konsole on KDE, GNOME Terminal/Ptyxis on GNOME,
   XFCE4 Terminal on XFCE)
5. the first installed from: kitty, Ghostty, Alacritty, Konsole, GNOME
   Terminal, WezTerm, Ptyxis, foot, XFCE4 Terminal, Tilix, Terminator, xterm,
   `x-terminal-emulator`

Use `--terminal auto` to clear the `--terminal` pin and fall back to
environment/desktop detection. The same setting can be pinned in the systemd
unit with:

```ini
Environment=OPENCODE_VOICE_TERMINAL=kitty
```

If your terminal is not in the list, installing `xdg-terminal-exec` lets the
launcher use your desktop's registered default terminal instead.

### Tuning

- **Misses the phrase:** lower `--threshold` (try `0.10`), speak a little
  closer, or set the correct PipeWire source.
- **Quiet speech is missed by the efficiency gate:** lower `--energy-gate`
  (for example `0.001`) or set it to `0` to decode continuously.
- **False triggers:** raise `--threshold` (try `0.20`–`0.30`) or increase
  `--cooldown`.
- **"Continue", "stop", or "help" not recognized:** say the command during the
  five-second listening window. “Help” uses a slightly lower per-keyword
  threshold because it is a short word.
- **The wrong terminal opens:** pass `--terminal <NAME>`, set
  `OPENCODE_VOICE_TERMINAL=...`, or install `xdg-terminal-exec` and configure
  your preferred terminal there.
- **No notification appears:** install `libnotify` (`notify-send`). Voice
  commands still work without it; only the visual countdown is missing.
- To add another pronunciation, edit `WAKE_KEYWORDS` or `COMMAND_KEYWORDS` in
  `src/detector.rs`. The keywords are sentencepiece/BPE pieces and can be
  generated with the model's `bpe.model`.

## How it works

1. `cpal` opens the default input (preferring 16 kHz mono; otherwise the device
   default) and downmixes to mono in the audio callback.
2. A lock-free ring buffer hands samples to a single worker thread in 100 ms
   chunks.
3. A cheap RMS gate drops quiet chunks and keeps a half-second preroll. When
   speech starts it replays the preroll, then feeds the always-on wake stream
   to a sherpa-onnx Zipformer2 keyword spotter. After a second of silence the
   wake stream is reset, so idle CPU stays near zero. When “Hey, OpenCode” is
   recognized, it shows a `notify-send` notification and starts a five-second
   countdown.
4. During the countdown only the command stream is fed: `open`, `continue`,
   `stop`, and `help`. The command stream uses the same loaded model, so there
   is no second model and no extra idle CPU.
5. If a command is recognized, the service runs it immediately. Otherwise,
   after five seconds it launches the detected terminal with `opencode`.
6. “Open” starts a normal session immediately. “Continue” starts
   `opencode --continue`. “Stop” calls the OpenCode API
   (`/api/session/active` and `/api/session/{id}/interrupt`) to interrupt every
   session that is currently generating, even when the terminal is not focused.
   “Help” starts `opencode-voice-help` in a new terminal window.
7. After an action, the streams are reset, stale audio is drained, and the
   service enters a cooldown (4 seconds for launches, 1 second for stop).

The model and binary are built for x86_64 Linux. The release binary links
against the system `libstdc++`/glibc and does not require Rust at runtime.

## Build

Builds happen on GitHub Actions, not on the T480. The workflow uses an
`archlinux:latest` container, installs Rust and ALSA headers, then runs
`cargo test --release --locked` and `cargo build --release --locked`.

Push a tag to publish a release:

```bash
git tag v0.3.0
git push origin v0.3.0
```

You can also run the **Build** workflow manually from the Actions tab.

## Uninstall

```bash
systemctl --user disable --now opencode-voice
rm -f ~/.config/systemd/user/opencode-voice.service
rm -f ~/.local/bin/opencode-voice
rm -f ~/.local/bin/opencode-voice-help
rm -rf ~/.local/share/opencode-voice
systemctl --user daemon-reload
```

## License

MIT — Copyright (c) 2026 Cseman Péter.

The wake-word model is
[`sherpa-onnx-kws-zipformer-gigaspeech-3.3M-2024-01-01`](https://github.com/k2-fsa/sherpa-onnx/releases/tag/kws-models)
and is licensed separately by its authors.
