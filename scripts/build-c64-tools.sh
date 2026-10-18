#!/usr/bin/env bash
# Builds the Commodore 64 development tools for the Games view's "C64 assembler" projects and
# the C64 lab:
#   - 64tass 1.60.3243 (GPL-2.0-or-later, https://tass64.sourceforge.net): a 6502 assembler
#     that writes C64 program (PRG) files. RAD runs it as a separate program.
# Output: c64tools/bin (used by dev builds; build-app.sh copies it into RAD's bundle as
# Contents/Helpers/c64). Needs: a C compiler and make (Xcode command line tools), unzip.
set -euo pipefail

cd "$(dirname "$0")/.."
VERSION=1.60.3243
SHA256=9d83be3d23a2c55e085b7c7a7856c2f96080447ea120a6a8c21a217ed76427f0
SRC=c64tools/src
OUT=c64tools/bin
mkdir -p "$OUT" "$SRC"

ZIP="$SRC/64tass-$VERSION-src.zip"
if [[ ! -f "$ZIP" ]]; then
  curl -fsSL -o "$ZIP" "https://downloads.sourceforge.net/project/tass64/source/64tass-$VERSION-src.zip"
fi
echo "$SHA256  $ZIP" | shasum -a 256 -c - >/dev/null || { echo "64tass download doesn't match its checksum" >&2; rm -f "$ZIP"; exit 1; }
if [[ ! -d "$SRC/64tass-$VERSION-src" ]]; then
  unzip -q -o "$ZIP" -d "$SRC"
fi
echo "==> 64tass $VERSION"
make -s -C "$SRC/64tass-$VERSION-src" -j"$(sysctl -n hw.ncpu 2>/dev/null || nproc)" >/dev/null
install -m 755 "$SRC/64tass-$VERSION-src/64tass" "$OUT/64tass"
cp "$SRC/64tass-$VERSION-src/LICENSE-GPL-2.0" "$OUT/64tass-LICENSE.txt"
echo "C64 tools ready: $OUT"
