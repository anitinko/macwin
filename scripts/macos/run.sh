#!/usr/bin/env bash
# Runs MacWin.app through LaunchServices with the given macwin arguments, so
# macOS attributes its permission prompts (Accessibility, Input Monitoring,
# Local Network, Paste from Other Apps) to MacWin.app instead of the
# terminal. Output goes to ~/Library/Logs/macwin/macwin.log and is followed
# here. Ctrl+C in this terminal stops the app gracefully.
#
#   scripts/macos/run.sh probe permissions --prompt
#   scripts/macos/run.sh pair --code 123456
#   scripts/macos/run.sh run
#   scripts/macos/run.sh tray        # the menu bar app (also: open target/MacWin.app)
#
# There is no terminal input inside the app: pass the pairing code with --code.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
APP="$ROOT/target/MacWin.app"
BIN="$APP/Contents/MacOS/macwin"
if [[ ! -x "$BIN" ]]; then
    echo "MacWin.app is missing: run scripts/macos/bundle.sh first" >&2
    exit 1
fi

LOG_DIR="$HOME/Library/Logs/macwin"
LOG="$LOG_DIR/macwin.log"
mkdir -p "$LOG_DIR"
: >"$LOG"

tail -n +1 -F "$LOG" 2>/dev/null &
TAIL=$!
cleanup() { kill "$TAIL" 2>/dev/null || true; }
trap cleanup EXIT
trap 'pkill -INT -f "$BIN" 2>/dev/null || true' INT TERM

open -n -W --stdout "$LOG" --stderr "$LOG" "$APP" --args "$@"
sleep 0.3
