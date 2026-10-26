#!/usr/bin/env bash
set -euo pipefail

clear

cat <<'EOF'
OpenCode Voice - voice commands
===============================

  "Hey, OpenCode"              Start listening for five seconds
  "Hey, OpenCode, open"        Open OpenCode
  "Hey, OpenCode, open mini"   Run opencode mini
  "Hey, OpenCode, mini"        Run opencode mini
  "Hey, OpenCode, open ChatGPT"  Open ChatGPT in Firefox
  "Hey, OpenCode, open Claude"   Open Claude in Firefox
  "Hey, OpenCode, continue"    Resume the last session (opencode --continue)
  "Hey, OpenCode, resume"      Alias for continue
  "Hey, OpenCode, stop"        Interrupt the active OpenCode response
  "Hey, OpenCode, help"        Show this help window
  "Hey, OpenCode, never mind"  Cancel the countdown and do nothing

After "Hey, OpenCode", say open, open mini, mini, open ChatGPT, open Claude,
continue/resume, stop, help, or never mind within five seconds. If no command
is heard, the countdown cancels without opening OpenCode.

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
