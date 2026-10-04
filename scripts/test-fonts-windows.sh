#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")/.."
bash scripts/test-mouse-windows.sh
cargo test --target x86_64-pc-windows-gnu --bin dopaterm installed_catalog_only_contains_available_monospaced_faces -- --nocapture
EXE=$(wslpath -w target/x86_64-pc-windows-gnu/release/dopaterm.exe)
powershell.exe -NoProfile -Command "\$env:DOPA_SETTINGS_DEMO='1'; \$path=Join-Path \$env:TEMP ('dopaterm-fonts-' + [guid]::NewGuid() + '.png'); & '$EXE' --no-fx --screenshot \$path; if (\$LASTEXITCODE -ne 0 -or !(Test-Path \$path)) { throw 'Settings screenshot failed' }; Write-Output \$path"
