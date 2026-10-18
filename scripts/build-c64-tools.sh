#!/usr/bin/env bash
# Builds the Commodore 64 development tools for the Games view's "C64 assembler" projects and
# the C64 lab:
#   - 64tass 1.60.3243 (GPL-2.0-or-later, https://tass64.sourceforge.net): a 6502 assembler
#     that writes C64 program (PRG) files. RAD runs it as a separate program.
# Output: c64tools/bin (used by dev builds; the app build scripts bundle it as c64/). Needs: a C
# compiler and make (Xcode command line tools / build-essential), unzip. On Windows (Git Bash)
# the project's own Windows build is downloaded instead.
set -euo pipefail

cd "$(dirname "$0")/.."
VERSION=1.60.3243
SRC=c64tools/src
OUT=c64tools/bin
mkdir -p "$OUT" "$SRC"

sha256() { if command -v sha256sum >/dev/null; then sha256sum "$1"; else shasum -a 256 "$1"; fi | cut -d' ' -f1; }

case "$(uname -s)" in
  MINGW*|MSYS*|CYGWIN*)
    ZIP="$SRC/64tass-$VERSION.zip"
    [[ -f "$ZIP" ]] || curl -fsSL -o "$ZIP" "https://downloads.sourceforge.net/project/tass64/binaries/64tass-$VERSION.zip"
    if [[ "$(sha256 "$ZIP")" != 04bf54b7e975c13485f991a59a54e3cb909f9c439aab8288c4927951bdcce781 ]]; then
      rm -f "$ZIP"; echo "64tass download doesn't match its checksum" >&2; exit 1
    fi
    rm -rf "$SRC/win" && mkdir -p "$SRC/win"
    powershell -NoProfile -Command "Expand-Archive -Force '$(cygpath -w "$ZIP")' '$(cygpath -w "$SRC/win")'"
    cp "$SRC/win/64tass-$VERSION/64tass.exe" "$OUT/"
    cp "$SRC/win/64tass-$VERSION/LICENSE-GPL-2.0" "$OUT/64tass-LICENSE.txt"
    ;;
  *)
    ZIP="$SRC/64tass-$VERSION-src.zip"
    [[ -f "$ZIP" ]] || curl -fsSL -o "$ZIP" "https://downloads.sourceforge.net/project/tass64/source/64tass-$VERSION-src.zip"
    if [[ "$(sha256 "$ZIP")" != 9d83be3d23a2c55e085b7c7a7856c2f96080447ea120a6a8c21a217ed76427f0 ]]; then
      rm -f "$ZIP"; echo "64tass download doesn't match its checksum" >&2; exit 1
    fi
    [[ -d "$SRC/64tass-$VERSION-src" ]] || unzip -q -o "$ZIP" -d "$SRC"
    echo "==> 64tass $VERSION"
    make -s -C "$SRC/64tass-$VERSION-src" -j"$(sysctl -n hw.ncpu 2>/dev/null || nproc)" >/dev/null
    install -m 755 "$SRC/64tass-$VERSION-src/64tass" "$OUT/64tass"
    cp "$SRC/64tass-$VERSION-src/LICENSE-GPL-2.0" "$OUT/64tass-LICENSE.txt"
    ;;
esac
echo "C64 tools ready: $OUT"
