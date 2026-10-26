# opencode-voice

A tiny, fully offline wake-word launcher for Linux. Say **"Hey, OpenCode"** and
it opens a new Konsole window running `opencode`.

Built for a low-power ThinkPad T480:

- **Offline** — no cloud, no API keys, microphone audio stays on the machine.
- **Lightweight** — 3.3M-parameter sherpa-onnx keyword spotter (~20 MB model).
- **Efficient** — single worker thread, 16 kHz mono where PipeWire can provide
  it, and roughly 4% of one CPU core while continuously decoding (RTF ~0.04 on
  an i5-8250U). Idle CPU is much lower.
- **Simple** — Rust binary + user systemd service. No Electron, no Python, no
  background daemon framework.

## Requirements

- Arch Linux / CachyOS (or any distro with PipeWire and KDE Plasma)
- KDE Konsole
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
  --threshold <0-1>   Keyword confidence threshold (default: 0.15)
  --cooldown <SECS>   Ignore detections for this long after a launch (default: 4)
  --wav <FILE>        Test a WAV file instead of listening to the microphone
  --dry-run           Detect and log, but do not launch Konsole
  --list-devices      List available audio input devices and exit
  -h, --help          Print help
  -V, --version       Print version
```

Examples:

```bash
# See what the microphone options are called
opencode-voice --list-devices

# Listen on a specific microphone, without opening Konsole
opencode-voice --device "USB" --dry-run

# Check detection against a 16 kHz mono WAV file
opencode-voice --wav sample.wav
```

If the default source is not the microphone you want, set it in PipeWire or
pass `--device` to the service. A substring is enough, for example
`--device "Belső hangforrás"`.

### Tuning

- **Misses the phrase:** lower `--threshold` (try `0.10`), speak a little
  closer, or set the correct PipeWire source.
- **False triggers:** raise `--threshold` (try `0.20`–`0.30`) or increase
  `--cooldown`.
- To add another pronunciation, edit `KEYWORDS` in `src/detector.rs`. The
  keywords are sentencepiece/BPE pieces and can be generated with the model's
  `bpe.model`.

## How it works

1. `cpal` opens the default input (preferring 16 kHz mono; otherwise the device
   default) and downmixes to mono in the audio callback.
2. A lock-free ring buffer hands samples to a single worker thread.
3. The worker feeds normalized `f32` samples to a sherpa-onnx Zipformer
   transducer configured for keyword spotting. Keywords are constrained to the
   wake phrase, so the model only searches a handful of BPE paths.
4. On a detection, the service starts `konsole --separate --title OpenCode -e
   opencode`, resets the stream, drains stale audio, and enters a cooldown.

The model and binary are built for x86_64 Linux. The release binary links
against the system `libstdc++`/glibc and does not require Rust at runtime.

## Build

Builds happen on GitHub Actions, not on the T480. The workflow uses an
`archlinux:latest` container, installs Rust and ALSA headers, then runs
`cargo test --release --locked` and `cargo build --release --locked`.

Push a tag to publish a release:

```bash
git tag v0.1.0
git push origin v0.1.0
```

You can also run the **Build** workflow manually from the Actions tab.

## Uninstall

```bash
systemctl --user disable --now opencode-voice
rm -f ~/.config/systemd/user/opencode-voice.service
rm -f ~/.local/bin/opencode-voice
rm -rf ~/.local/share/opencode-voice
systemctl --user daemon-reload
```

## License

MIT. The wake-word model is
[`sherpa-onnx-kws-zipformer-gigaspeech-3.3M-2024-01-01`](https://github.com/k2-fsa/sherpa-onnx/releases/tag/kws-models)
and is licensed separately by its authors.
