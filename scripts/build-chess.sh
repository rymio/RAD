#!/usr/bin/env bash
# Builds RAD's chess engine (chess/rad-chess: RustChess behind a JSON request) and places it where
# the app finds it:
#   - development (`cargo tauri dev`) uses chess/rad-chess/target/release/rad-chess directly;
#   - bundles: scripts/build-app.sh ships the copy in app/src-tauri/binaries/ next to the RAD executable.
# RustChess is fetched from GitHub at a pinned commit (it has no licence file, so it isn't vendored).
set -euo pipefail

cd "$(dirname "$0")/.."
triple="${1:-$(rustc -vV | sed -n 's/^host: //p')}"
ext=""
[[ "$triple" == *windows* ]] && ext=".exe"

cargo build --release --manifest-path chess/rad-chess/Cargo.toml --target "$triple"
mkdir -p app/src-tauri/binaries
cp "chess/rad-chess/target/$triple/release/rad-chess$ext" "app/src-tauri/binaries/rad-chess-$triple$ext"
# Plain path for `cargo tauri dev`.
mkdir -p chess/rad-chess/target/release
cp "chess/rad-chess/target/$triple/release/rad-chess$ext" "chess/rad-chess/target/release/rad-chess$ext"
echo "Chess engine ready: app/src-tauri/binaries/rad-chess-$triple$ext"
