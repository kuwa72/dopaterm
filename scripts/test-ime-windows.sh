#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")/.."
bash scripts/test-mouse-windows.sh
EXE=$(wslpath -w target/x86_64-pc-windows-gnu/release/dopaterm.exe)
powershell.exe -NoProfile -Command "\$env:DOPA_IME_DEMO='1'; \$path=Join-Path \$env:TEMP ('dopaterm-ime-' + [guid]::NewGuid() + '.png'); & '$EXE' --no-fx --screenshot \$path; if (\$LASTEXITCODE -ne 0 -or !(Test-Path \$path)) { throw 'IME screenshot failed' }; Write-Output \$path"
