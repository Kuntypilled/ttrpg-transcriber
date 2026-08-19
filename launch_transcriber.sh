#!/usr/bin/env bash
# Launcher wrapper for the Session Transcriber, called by the .desktop entry.
# The campaign is chosen in the GUI, so this takes no arguments.
#
# Resolves its own location, so the checkout can live anywhere. Any argument
# given is passed through to transcribe_gui.py.

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$(readlink -f "${BASH_SOURCE[0]}")")" && pwd)"

LOG="${GM_TRANSCRIBER_LOG:-${XDG_STATE_HOME:-$HOME/.local/state}/ttrpg-transcriber/transcribe_gui.log}"
mkdir -p "$(dirname "$LOG")"

echo "[$(date '+%Y-%m-%dT%H:%M:%S')] Launcher invoked" >> "$LOG" 2>&1

# X11 is forced because the GTK3 front end has known quirks under the Wayland
# backend on tiling compositors. Set GDK_BACKEND yourself to override.
export GDK_BACKEND="${GDK_BACKEND:-x11}"

exec python3 "$SCRIPT_DIR/transcribe_gui.py" "$@" >> "$LOG" 2>&1
