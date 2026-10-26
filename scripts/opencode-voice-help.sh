#!/usr/bin/env bash
set -euo pipefail

clear

cat <<'EOF'
OpenCode Voice - voice commands
===============================

  "Hey, OpenCode"              Start listening for five seconds
  "Hey, OpenCode, open"        Open OpenCode immediately
  "Hey, OpenCode, continue"    Resume the last session (opencode --continue)
  "Hey, OpenCode, stop"        Interrupt the active OpenCode response
  "Hey, OpenCode, help"        Show this help window

After "Hey, OpenCode", say open, continue, stop, or help within five seconds.
If no command is heard, OpenCode opens normally.

The microphone audio is processed entirely offline by the local wake-word
model. Only "opencode-voice" and the local OpenCode API are used; no audio or
text is sent to a cloud service.

Service commands
----------------
  systemctl --user status opencode-voice
  systemctl --user restart opencode-voice
  journalctl --user -u opencode-voice -f

EOF

printf 'Press Enter to close...'
read -r _
