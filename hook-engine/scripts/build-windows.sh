#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")/.."
# A local toolchain may be selected by the caller. No download/install occurs.
export CARGO_TARGET_X86_64_PC_WINDOWS_GNU_LINKER="${CARGO_TARGET_X86_64_PC_WINDOWS_GNU_LINKER:-x86_64-w64-mingw32-gcc}"
export CARGO_TARGET_I686_PC_WINDOWS_GNU_LINKER="${CARGO_TARGET_I686_PC_WINDOWS_GNU_LINKER:-i686-w64-mingw32-gcc}"
mkdir -p dist
for target in x86_64-pc-windows-gnu i686-pc-windows-gnu; do
  case "$target" in x86_64-*) suffix=x64;; *) suffix=x86;; esac
  cargo build --locked --release --target "$target" -p coralspy-hook-payload
  cargo build --locked --release --target "$target" -p coralspy-hook-broker
  cp "target/$target/release/coralspy-hook-broker.exe" "dist/coralspy-hook-broker-$suffix.exe"
  # Payloads are embedded in their matching broker; no loose payload is installed.
done
python3 scripts/audit-binaries.py
