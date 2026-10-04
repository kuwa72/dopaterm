#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")/.."
TARGET=x86_64-pc-windows-gnu
cargo test --bin dopaterm --quiet
cargo test --target "$TARGET" --bin dopaterm --quiet
cargo build --release --target "$TARGET" --bin dopaterm --bin mouse_test --quiet
cargo build --bin mouse_test --quiet
powershell.exe -NoProfile -ExecutionPolicy Bypass -File "$(wslpath -w scripts/verify-mouse-windows.ps1)" \
    -Binary "$(wslpath -w "target/$TARGET/release/dopaterm.exe")" \
    -NativeTest "$(wslpath -w "target/$TARGET/release/mouse_test.exe")" \
    -LinuxTest "$PWD/target/debug/mouse_test" -Distro "${WSL_DISTRO_NAME:-Ubuntu}"
