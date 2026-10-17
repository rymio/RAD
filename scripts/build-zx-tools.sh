#!/usr/bin/env bash
# Builds the ZX Spectrum development tools for the Games view's "ZX Spectrum assembler" projects:
#   - sjasmplus 1.24.0 (BSD-3-Clause, https://github.com/z00m128/sjasmplus): a Z80 assembler
#     that writes Spectrum snapshots and tapes.
# Output: zxtools/bin (used by dev builds; build-app.sh copies it into RAD's bundle as
# Contents/Helpers/zx). Needs: a C++ compiler and make (Xcode command line tools).
set -euo pipefail

cd "$(dirname "$0")/.."
TAG=v1.24.0
SRC=zxtools/src/sjasmplus
OUT=zxtools/bin
mkdir -p "$OUT" zxtools/src

if [[ ! -d "$SRC/.git" ]]; then
  git clone --quiet --depth 1 --branch "$TAG" --recurse-submodules https://github.com/z00m128/sjasmplus.git "$SRC"
fi
echo "==> sjasmplus $TAG"
make -s -C "$SRC" -j"$(sysctl -n hw.ncpu 2>/dev/null || nproc)" >/dev/null
install -m 755 "$SRC/sjasmplus" "$OUT/sjasmplus"
cp "$SRC/LICENSE.md" "$OUT/sjasmplus-LICENSE.md"
echo "ZX tools ready: $OUT"
