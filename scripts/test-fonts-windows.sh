#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")/.."
bash scripts/test-mouse-windows.sh
cargo test --target x86_64-pc-windows-gnu --bin dopaterm installed_catalog_only_contains_available_monospaced_faces -- --nocapture
EXE=$(wslpath -w target/x86_64-pc-windows-gnu/release/dopaterm.exe)
# `&` does not wait for GUI-subsystem exes, so poll for the output file
# instead of checking $LASTEXITCODE (also unset for GUI apps).
powershell.exe -NoProfile -Command "\$env:DOPA_SETTINGS_DEMO='1'; \$path=Join-Path \$env:TEMP ('dopaterm-fonts-' + [guid]::NewGuid() + '.png'); & '$EXE' --no-fx --screenshot \$path; \$ok=\$false; for (\$i=0; \$i -lt 120 -and !\$ok; \$i++) { if (Test-Path \$path) { \$ok=\$true; break }; Start-Sleep -Milliseconds 500 }; if (!\$ok) { throw 'Settings screenshot failed' }; Write-Output \$path"
