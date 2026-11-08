#!/usr/bin/env bash
# Builds Coach's speech recognition helper (coach/rad-coach-asr: Whisper word timings and Silero VAD,
# whisper.cpp) and places it where RAD finds it:
#   - development (`cargo tauri dev`) uses coach/target/release/rad-coach-asr directly;
#   - bundles: scripts/build-app.sh ships the copy in app/src-tauri/binaries/ next to the RAD executable.
# whisper.cpp brings its own ggml, which can't share RAD's process with llama.cpp, hence the helper.
# The first build compiles whisper.cpp (CMake); allow a few minutes.
set -euo pipefail

cd "$(dirname "$0")/.."
triple="${1:-$(rustc -vV | sed -n 's/^host: //p')}"
ext=""
[[ "$triple" == *windows* ]] && ext=".exe"
features=()
[[ "$triple" == *apple-darwin* ]] && features=(--features metal)

cargo build --release --manifest-path coach/Cargo.toml -p rad-coach-asr --target "$triple" "${features[@]}"
mkdir -p app/src-tauri/binaries
cp "coach/target/$triple/release/rad-coach-asr$ext" "app/src-tauri/binaries/rad-coach-asr-$triple$ext"
# Plain path for `cargo tauri dev`.
mkdir -p coach/target/release
cp "coach/target/$triple/release/rad-coach-asr$ext" "coach/target/release/rad-coach-asr$ext"
echo "Coach speech helper ready: app/src-tauri/binaries/rad-coach-asr-$triple$ext"
