#!/usr/bin/env bash
# Restart the running Windows dopaterm.exe with the freshly built binary.
# Captures the current command line, kills the process, rebuilds + installs,
# then relaunches with the same arguments.
#
#   scripts/restart-windows.sh [--debug]
set -euo pipefail
cd "$(dirname "$0")/.."

EXE_WIN="C:\\\\Users\\\\ykuwa\\\\.local\\\\bin\\\\dopaterm.exe"

# Capture the running process's command line (best effort; may be empty).
CMDLINE="$(
    powershell.exe -NoProfile -Command \
        "(Get-CimInstance Win32_Process -Filter \"Name='dopaterm.exe'\" | Select-Object -First 1 -ExpandProperty CommandLine)" \
        2>/dev/null | tr -d '\r' || true
)"
# Strip the executable token (quoted or not) to recover the argument string.
ARGS="$(printf '%s' "$CMDLINE" | sed -E 's/^[[:space:]]*"?[^"]*dopaterm\.exe"?[[:space:]]*//')"
if [ -n "$CMDLINE" ]; then
    echo "captured args: ${ARGS:-<none>}"
fi

# Kill the running instance so the exe is not locked.
powershell.exe -NoProfile -Command \
    "Get-Process dopaterm -ErrorAction SilentlyContinue | Stop-Process -Force" \
    2>/dev/null || true

./scripts/build-windows.sh "$@"

# Relaunch detached so it survives this WSL session.
if [ -n "${ARGS// /}" ]; then
    powershell.exe -NoProfile -Command \
        "Start-Process -FilePath '$EXE_WIN' -ArgumentList '$ARGS'" >/dev/null
else
    powershell.exe -NoProfile -Command \
        "Start-Process -FilePath '$EXE_WIN'" >/dev/null
fi
echo "relaunched: dopaterm.exe"
