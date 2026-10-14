#!/usr/bin/env bash
# Builds RAD's text-to-speech engine: mio-tts-server from mio-tts-cpp (MIT, C++/llama.cpp/ggml),
# which runs MioTTS-0.1B and the MioCodec vocoder. RAD starts it on demand on 127.0.0.1.
#   - development (`cargo tauri dev`) uses tts/bin/mio-tts-server directly;
#   - bundles: `cargo tauri build --config app/src-tauri/tauri.sidecars.conf.json` ships the copy in
#     app/src-tauri/binaries/ next to the RAD executable.
# The models (~510 MB) are downloaded from Settings → Voice, not here.
set -euo pipefail

cd "$(dirname "$0")/.."
REPO=https://github.com/mmnga/mio-tts-cpp.git
COMMIT=6180947f713f754e490581149d07fd00b4613b66   # pinned: the server API RAD speaks
SRC=tts/mio-tts-cpp
triple="${1:-$(rustc -vV | sed -n 's/^host: //p')}"
ext=""
[[ "$triple" == *windows* ]] && ext=".exe"

if [[ ! -d "$SRC/.git" ]]; then
  git clone --quiet "$REPO" "$SRC"
fi
git -C "$SRC" fetch --quiet origin "$COMMIT" 2>/dev/null || true
git -C "$SRC" checkout --quiet "$COMMIT"
git -C "$SRC" submodule update --init --recursive --quiet

# One self-contained binary: static ggml/llama, no OpenSSL (it only ever talks to 127.0.0.1).
cmake -S "$SRC" -B "$SRC/build-rad" \
  -DCMAKE_BUILD_TYPE=Release \
  -DLLAMA_CPP_SOURCE_DIR="$PWD/$SRC/llama.cpp" \
  -DBUILD_SHARED_LIBS=OFF \
  -DLLAMA_OPENSSL=OFF \
  -DLLAMA_BUILD_TESTS=OFF -DLLAMA_BUILD_EXAMPLES=OFF \
  -DMIO_TTS_BUILD_LLAMA_SERVER=OFF \
  -DGGML_NATIVE=OFF
cmake --build "$SRC/build-rad" --config Release -j "${JOBS:-8}" --target mio-tts-server

bin="$SRC/build-rad/mio-tts-server$ext"
[[ -f "$bin" ]] || bin="$SRC/build-rad/Release/mio-tts-server$ext"
mkdir -p tts/bin app/src-tauri/binaries
cp "$bin" "tts/bin/mio-tts-server$ext"
cp "$bin" "app/src-tauri/binaries/mio-tts-server-$triple$ext"
echo "Text-to-speech engine ready: app/src-tauri/binaries/mio-tts-server-$triple$ext"
