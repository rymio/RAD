#!/usr/bin/env bash
# Builds the RAD desktop app with its engines inside the bundle:
#   - the web browser engine (Servo)      → scripts/build-browser.sh
#   - the text-to-speech engine (MioTTS)  → scripts/build-tts.sh
# Both are incremental, so re-running is quick. Extra arguments go to `cargo tauri build`
# (e.g. --bundles app). Result: target/release/bundle/…
set -euo pipefail

cd "$(dirname "$0")/.."
triple="$(rustc -vV | sed -n 's/^host: //p')"

./scripts/build-browser.sh "$triple"
./scripts/build-tts.sh "$triple"

(cd app/ui && { [[ -d node_modules ]] || npm ci; })
cd app
cargo tauri build --config src-tauri/tauri.sidecars.conf.json "$@"

echo
echo "Built with the browser and voice engines. On macOS: quit RAD, then replace /Applications/RAD.app with"
echo "  $(cd .. && pwd)/target/release/bundle/macos/RAD.app"
