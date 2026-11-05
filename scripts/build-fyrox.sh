#!/usr/bin/env bash
# Builds Gilgamesh's world (fyrox/world: a Fyrox 1.0 project) and places it where the app finds it:
#   - development (`cargo tauri dev`) uses fyrox/world/target/release/{executor,editor} directly;
#   - bundles: scripts/build-app.sh ships the copies in app/src-tauri/binaries/ next to RAD:
#     rad-world (the world) and rad-world-editor (the Fyrox editor with the world in it).
# Fyrox (MIT) comes from crates.io, pinned to 1.0.1. The first build takes a few minutes.
set -euo pipefail

cd "$(dirname "$0")/.."
triple="${1:-$(rustc -vV | sed -n 's/^host: //p')}"
ext=""
[[ "$triple" == *windows* ]] && ext=".exe"

cargo build --release --manifest-path fyrox/world/Cargo.toml -p executor -p editor
mkdir -p app/src-tauri/binaries
cp "fyrox/world/target/release/executor$ext" "app/src-tauri/binaries/rad-world-$triple$ext"
cp "fyrox/world/target/release/editor$ext" "app/src-tauri/binaries/rad-world-editor-$triple$ext"
echo "Gilgamesh's world ready: app/src-tauri/binaries/rad-world-$triple$ext (+ rad-world-editor)"
