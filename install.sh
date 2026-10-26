#!/usr/bin/env bash
set -euo pipefail

REPO="${OPENCODE_VOICE_REPO:-Peter5235252/opencode-voice}"
MODEL_NAME="sherpa-onnx-kws-zipformer-gigaspeech-3.3M-2024-01-01"
MODEL_URL="https://github.com/k2-fsa/sherpa-onnx/releases/download/kws-models/${MODEL_NAME}.tar.bz2"

BIN_DIR="${HOME}/.local/bin"
DATA_HOME="${XDG_DATA_HOME:-${HOME}/.local/share}"
MODEL_ROOT="${DATA_HOME}/opencode-voice/models"
MODEL_DIR="${MODEL_ROOT}/${MODEL_NAME}"
UNIT_DIR="${XDG_CONFIG_HOME:-${HOME}/.config}/systemd/user"

need() {
    command -v "$1" >/dev/null 2>&1 || {
        printf 'missing dependency: %s\n' "$1" >&2
        exit 1
    }
}

need curl
need tar
need systemctl

command -v konsole >/dev/null 2>&1 || {
    printf 'Konsole is not installed.\n' >&2
    exit 1
}
command -v opencode >/dev/null 2>&1 || {
    printf 'opencode is not in PATH.\n' >&2
    exit 1
}

mkdir -p "$BIN_DIR" "$MODEL_ROOT" "$UNIT_DIR"
TMP_DIR="$(mktemp -d)"
trap 'rm -rf "$TMP_DIR"' EXIT

printf 'Downloading the latest opencode-voice release...\n'
ASSET_URL="$(
    curl -fsSL "https://api.github.com/repos/${REPO}/releases/latest" \
        | grep -o 'https://[^"]*x86_64-linux\.tar\.gz' \
        | head -n 1
)"

if [ -z "$ASSET_URL" ]; then
    printf 'No x86_64 release asset found for %s.\n' "$REPO" >&2
    printf 'Run the Build workflow on GitHub first.\n' >&2
    exit 1
fi

curl -fL "$ASSET_URL" -o "${TMP_DIR}/opencode-voice.tar.gz"
tar -xzf "${TMP_DIR}/opencode-voice.tar.gz" -C "$TMP_DIR"
install -Dm755 "${TMP_DIR}/opencode-voice" "${BIN_DIR}/opencode-voice"
if [ -f "${TMP_DIR}/opencode-voice-help" ]; then
    install -Dm755 "${TMP_DIR}/opencode-voice-help" "${BIN_DIR}/opencode-voice-help"
fi

if [ ! -f "${MODEL_DIR}/tokens.txt" ]; then
    printf 'Downloading the offline wake-word model...\n'
    curl -fL "$MODEL_URL" -o "${TMP_DIR}/model.tar.bz2"
    tar -xjf "${TMP_DIR}/model.tar.bz2" -C "$MODEL_ROOT"
else
    printf 'Wake-word model already present.\n'
fi

cat > "${UNIT_DIR}/opencode-voice.service" <<EOF
[Unit]
Description=OpenCode voice launcher
Documentation=https://github.com/${REPO}
After=graphical-session.target pipewire.service wireplumber.service
PartOf=graphical-session.target
ConditionPathExists=${BIN_DIR}/opencode-voice

[Service]
Type=simple
WorkingDirectory=%h
ExecStart=${BIN_DIR}/opencode-voice --model-dir ${MODEL_DIR}
Restart=on-failure
RestartSec=3
Nice=10
IOSchedulingClass=idle
NoNewPrivileges=true

[Install]
WantedBy=graphical-session.target
EOF

systemctl --user daemon-reload
systemctl --user enable opencode-voice.service
systemctl --user restart opencode-voice.service

printf '\nInstalled.\n'
printf '  binary:  %s\n' "${BIN_DIR}/opencode-voice"
printf '  help:    %s\n' "${BIN_DIR}/opencode-voice-help"
printf '  model:   %s\n' "${MODEL_DIR}"
printf '  service: %s\n' "${UNIT_DIR}/opencode-voice.service"
printf '\nSay "Hey, OpenCode" and then open, continue, stop, or help within 5 seconds.\n'
printf 'Logs: journalctl --user -u opencode-voice -f\n'
