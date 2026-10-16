#!/usr/bin/env bash
# Builds RAD's Tor browser engine (Servo + embedded Arti, browser/rad-tor-browser) and places it
# where the app finds it:
#   - development (`cargo tauri dev`) uses browser/rad-tor-browser/target/release/rad-tor-browser directly;
#   - bundles ship the copy in app/src-tauri/binaries/ next to the RAD executable.
# The first build compiles Servo, SpiderMonkey and Arti (Tor) and takes a while; later builds are quick.
set -euo pipefail

cd "$(dirname "$0")/.."
triple="${1:-$(rustc -vV | sed -n 's/^host: //p')}"
ext=""
[[ "$triple" == *windows* ]] && ext=".exe"

cargo build --release --manifest-path browser/rad-tor-browser/Cargo.toml --target "$triple"
mkdir -p app/src-tauri/binaries
cp "browser/rad-tor-browser/target/$triple/release/rad-tor-browser$ext" "app/src-tauri/binaries/rad-tor-browser-$triple$ext"
# Plain path for `cargo tauri dev`.
mkdir -p browser/rad-tor-browser/target/release
cp "browser/rad-tor-browser/target/$triple/release/rad-tor-browser$ext" "browser/rad-tor-browser/target/release/rad-tor-browser$ext"
echo "Tor browser engine ready: app/src-tauri/binaries/rad-tor-browser-$triple$ext"
