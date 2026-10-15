#!/usr/bin/env bash
# Builds RAD's Chromium engine (browser/rad-chromium: CEF, the Chromium Embedded Framework) and
# lays it out ready to run:
#   macOS:          browser/rad-chromium/target/bundle/rad-chromium.app (CEF framework + helpers)
#   Linux, Windows: browser/rad-chromium/target/bundle/rad-chromium/   (executable + CEF files)
# `cargo tauri dev` uses it from there; ./scripts/build-app.sh (and build-linux.sh,
# build-windows.ps1) put it inside the app. The first build downloads CEF (~300 MB, once).
set -euo pipefail

cd "$(dirname "$0")/../browser/rad-chromium"
cargo build --release --bins
rm -rf target/bundle
cargo run --release --quiet --bin rad-chromium-bundle >/dev/null
if [[ -d target/bundle/rad-chromium.app ]]; then
  echo "Chromium engine ready: browser/rad-chromium/target/bundle/rad-chromium.app"
else
  echo "Chromium engine ready: browser/rad-chromium/target/bundle/rad-chromium/"
fi
