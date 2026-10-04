#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "${BASH_SOURCE[0]}")"
mkdir -p bin
for arch in x64 x86; do
  case "$arch" in
    x64) cc=x86_64-w64-mingw32-gcc ;;
    x86) cc=i686-w64-mingw32-gcc ;;
  esac
  if ! command -v "$cc" >/dev/null; then
    printf 'SKIP %s: %s is not available\n' "$arch" "$cc" >&2
    continue
  fi
  "$cc" -std=c11 -Wall -Wextra -Werror -O2 -municode -DUNICODE -D_UNICODE \
    owned_fixture.c -o "bin/owned-fixture-$arch.exe" -lcomctl32 -luser32 -lgdi32 -lshell32
done
