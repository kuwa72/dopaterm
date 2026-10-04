#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")/.."
TARGET=x86_64-pc-windows-gnu
bash scripts/test-mouse-windows.sh
cargo build --release --target "$TARGET" --bin background_test --quiet
powershell.exe -NoProfile -ExecutionPolicy Bypass -File "$(wslpath -w scripts/verify-background-windows.ps1)" \
    -Binary "$(wslpath -w "target/$TARGET/release/dopaterm.exe")" \
    -Fixture "$(wslpath -w "target/$TARGET/release/background_test.exe")"
