#!/usr/bin/env bash
# Build the windowed (AppLoad/qtfb) flavour for reMarkable 1/2 (32-bit ARMv7).
#
# Uses cargo-zigbuild when present (works from macOS; links against glibc 2.27
# so the binary runs on old and new rM2 OS releases). Otherwise falls back to
# plain cargo with arm-linux-gnueabihf-gcc (Debian/Ubuntu: gcc-arm-linux-gnueabihf).
set -euo pipefail
cd "$(dirname "$0")/.."

TARGET=armv7-unknown-linux-gnueabihf
if command -v cargo-zigbuild >/dev/null 2>&1; then
    cargo zigbuild --release --target "$TARGET.2.27" "$@"
else
    cargo build --release --target "$TARGET" "$@"
fi
echo "built: target/$TARGET/release/riddle"
