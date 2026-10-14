#!/usr/bin/env bash
# Builds RAD's web browser engine (Servo, browser/rad-browser) and places it where the app finds it:
#   - development (`cargo tauri dev`) uses browser/rad-browser/target/release/rad-browser directly;
#   - bundles: `cargo tauri build --config app/src-tauri/tauri.browser.conf.json` ships the copy in
#     app/src-tauri/binaries/ next to the RAD executable.
# The first build compiles Servo and SpiderMonkey and takes a while; later builds are quick.
set -euo pipefail

cd "$(dirname "$0")/.."
triple="${1:-$(rustc -vV | sed -n 's/^host: //p')}"
ext=""
[[ "$triple" == *windows* ]] && ext=".exe"

cargo build --release --manifest-path browser/rad-browser/Cargo.toml --target "$triple"
mkdir -p app/src-tauri/binaries
cp "browser/rad-browser/target/$triple/release/rad-browser$ext" "app/src-tauri/binaries/rad-browser-$triple$ext"
# Plain path for `cargo tauri dev`.
mkdir -p browser/rad-browser/target/release
cp "browser/rad-browser/target/$triple/release/rad-browser$ext" "browser/rad-browser/target/release/rad-browser$ext"
echo "Browser engine ready: app/src-tauri/binaries/rad-browser-$triple$ext"
