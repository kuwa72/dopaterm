#!/usr/bin/env bash
# Build dopaterm.exe for Windows (x86_64-pc-windows-gnu) and deploy it
# to the user's local bin directory, along with the sample Lua effects.
#
# Usage:
#   scripts/build-windows.sh            # build + copy
#   scripts/build-windows.sh --debug    # dev build + copy
set -euo pipefail

cd "$(dirname "$0")/.."

TARGET=x86_64-pc-windows-gnu
PROFILE=release
[[ "${1:-}" == "--debug" ]] && PROFILE=debug

WIN_BIN=/mnt/c/Users/ykuwa/.local/bin
WIN_FXDIR=/mnt/c/Users/ykuwa/AppData/Roaming/dopaterm/effects

if [[ "$PROFILE" == release ]]; then
    cargo build --release --target "$TARGET"
else
    cargo build --target "$TARGET"
fi

mkdir -p "$WIN_BIN"
EXE="$WIN_BIN/dopaterm.exe"
SRC="target/$TARGET/$PROFILE/dopaterm.exe"
if ! cp "$SRC" "$EXE" 2>/dev/null; then
    # exe may be locked by a running process; rename is usually allowed.
    mv -f "$EXE" "$EXE.bak" 2>/dev/null || true
    if cp "$SRC" "$EXE"; then
        rm -f "$EXE.bak" 2>/dev/null || true
    else
        echo "error: $EXE is locked — close the running dopaterm window and retry" >&2
        exit 1
    fi
fi
echo "installed: $WIN_BIN/dopaterm.exe"

# Lua effect plugins (optional; the app runs fine without them).
mkdir -p "$WIN_FXDIR"
if compgen -G "effects/*.lua" > /dev/null; then
    cp effects/*.lua "$WIN_FXDIR/"
    echo "installed: $WIN_FXDIR/*.lua"
fi
