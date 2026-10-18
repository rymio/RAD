#!/usr/bin/env bash
# Builds the ZX Spectrum development tools for the Games view's "ZX Spectrum assembler" projects:
#   - sjasmplus 1.24.0 (BSD-3-Clause, https://github.com/z00m128/sjasmplus): a Z80 assembler
#     that writes Spectrum snapshots and tapes.
# Output: zxtools/bin (used by dev builds; the app build scripts bundle it as zx/). Needs: a C++
# compiler and make (Xcode command line tools / build-essential). On Windows (Git Bash) the
# project's own Windows build is downloaded instead.
set -euo pipefail

cd "$(dirname "$0")/.."
TAG=v1.24.0
SRC=zxtools/src/sjasmplus
OUT=zxtools/bin
mkdir -p "$OUT" zxtools/src

sha256() { if command -v sha256sum >/dev/null; then sha256sum "$1"; else shasum -a 256 "$1"; fi | cut -d' ' -f1; }

case "$(uname -s)" in
  MINGW*|MSYS*|CYGWIN*)
    ZIP=zxtools/src/sjasmplus-1.24.0.win.zip
    [[ -f "$ZIP" ]] || curl -fsSL -o "$ZIP" "https://github.com/z00m128/sjasmplus/releases/download/$TAG/sjasmplus-1.24.0.win.zip"
    if [[ "$(sha256 "$ZIP")" != 7e1f8840842039bdb97e51a59abda2e5c25a9da160097e8f79a62583ab72d0e0 ]]; then
      rm -f "$ZIP"; echo "sjasmplus download doesn't match its checksum" >&2; exit 1
    fi
    rm -rf zxtools/src/win && mkdir -p zxtools/src/win
    powershell -NoProfile -Command "Expand-Archive -Force '$(cygpath -w "$ZIP")' '$(cygpath -w zxtools/src/win)'"
    cp zxtools/src/win/sjasmplus-1.24.0.win/sjasmplus.exe "$OUT/"
    curl -fsSL -o "$OUT/sjasmplus-LICENSE.md" "https://raw.githubusercontent.com/z00m128/sjasmplus/$TAG/LICENSE.md"
    ;;
  *)
    if [[ ! -d "$SRC/.git" ]]; then
      git clone --quiet --depth 1 --branch "$TAG" --recurse-submodules https://github.com/z00m128/sjasmplus.git "$SRC"
    fi
    echo "==> sjasmplus $TAG"
    make -s -C "$SRC" -j"$(sysctl -n hw.ncpu 2>/dev/null || nproc)" >/dev/null
    install -m 755 "$SRC/sjasmplus" "$OUT/sjasmplus"
    cp "$SRC/LICENSE.md" "$OUT/sjasmplus-LICENSE.md"
    ;;
esac
echo "ZX tools ready: $OUT"
