#!/usr/bin/env bash
# Builds RAD's DOS engine for the DOS view:
#   - DOSBox Pure 1.0-preview7 (GPL-2.0-or-later, https://codeberg.org/schelling/dosbox-pure),
#     a DOSBox fork made to be driven frame by frame (a libretro core), with RAD's patch
#     dos/patches/direct-c.patch (RAD's hard disk folder is drive C:, written directly);
#   - dos/rad-dos (GPL-2.0-or-later): the small program that runs the core for RAD.
# Output: dos/rad-dos/target/bundle/rad-dos/{rad-dos, dosbox_pure_libretro.dylib, licences}.
# `cargo tauri dev` uses it from there; ./scripts/build-app.sh puts it inside the app as
# Contents/Helpers/rad-dos. Needs: git, a C++ compiler and make (Xcode command line tools), Rust.
set -euo pipefail

cd "$(dirname "$0")/.."
TAG=1.0-preview7
COMMIT=cdaea10ce8cac231bc341ca05d4c5e4e5fe0fbd5
SRC=dos/src/dosbox-pure
OUT=dos/rad-dos/target/bundle/rad-dos
mkdir -p dos/src

if [[ ! -d "$SRC/.git" ]]; then
  git clone --quiet --depth 1 --branch "$TAG" https://codeberg.org/schelling/dosbox-pure.git "$SRC"
fi
if [[ "$(git -C "$SRC" rev-parse HEAD)" != "$COMMIT" ]]; then
  echo "dos/src/dosbox-pure isn't DOSBox Pure $TAG ($COMMIT); delete it and run this again" >&2
  exit 1
fi
# RAD's patch (applied once; `git diff` shows it).
if git -C "$SRC" diff --quiet; then
  git -C "$SRC" apply "$PWD/dos/patches/direct-c.patch"
fi
echo "==> DOSBox Pure $TAG"
make -s -C "$SRC" -j"$(sysctl -n hw.ncpu 2>/dev/null || nproc)" >/dev/null

echo "==> rad-dos"
(cd dos/rad-dos && cargo build --release --quiet)

rm -rf "$OUT"
mkdir -p "$OUT"
cp dos/rad-dos/target/release/rad-dos "$OUT/"
case "$(uname -s)" in
  Darwin) cp "$SRC/dosbox_pure_libretro.dylib" "$OUT/" ;;
  *) cp "$SRC/dosbox_pure_libretro.so" "$OUT/" ;;
esac
cp "$SRC/LICENSE" "$OUT/LICENSE-GPL-2.0.txt"
cp "$SRC/DOSBOX-AUTHORS" "$OUT/DOSBOX-AUTHORS.txt"
echo "DOS engine ready: $OUT"
