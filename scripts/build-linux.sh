#!/usr/bin/env bash
# Builds the RAD desktop app for Linux (Ubuntu 22.04 / 24.04, x86_64 or arm64) with its engines
# inside the package:
#   - the web browser engine (Servo)        → scripts/build-browser.sh   (skip: --no-browser)
#   - the text-to-speech engine (MioTTS)    → scripts/build-tts.sh
#   - the Linux sandbox engine is macOS-only for now and is left out.
# Result: target/release/bundle/{deb,appimage}/…
#
#   ./scripts/build-linux.sh --install-deps     first time: installs the Ubuntu packages (sudo)
#   ./scripts/build-linux.sh                    later builds (incremental)
#   ./scripts/build-linux.sh --no-browser       quicker: RAD without the built-in browser
#   ./scripts/build-linux.sh --bundles deb      only the .deb (default: deb,appimage)
# Other arguments go to `cargo tauri build`.
set -euo pipefail

cd "$(dirname "$0")/.."

install_deps=0
browser=1
args=()
have_bundles=0
while [[ $# -gt 0 ]]; do
  case "$1" in
    --install-deps) install_deps=1 ;;
    --no-browser) browser=0 ;;
    --bundles|-b) have_bundles=1; args+=("$1" "$2"); shift ;;
    *) args+=("$1") ;;
  esac
  shift
done
[[ $have_bundles == 1 ]] || args+=(--bundles deb,appimage)

if [[ "$(uname -s)" != "Linux" ]]; then
  echo "error: this script builds the Linux app — run it on Ubuntu (use build-app.sh on macOS)" >&2
  exit 1
fi

# ------------------------------------------------------------------ system packages
if [[ $install_deps == 1 ]]; then
  pkgs=(
    # Tauri (WebKitGTK app shell, tray icon, packaging)
    build-essential curl wget file pkg-config libssl-dev libwebkit2gtk-4.1-dev libxdo-dev
    libayatana-appindicator3-dev librsvg2-dev
    # screenshots, clipboard, global shortcuts, keychain (Secret Service)
    libxcb1-dev libxrandr-dev libdbus-1-dev libpipewire-0.3-dev libwayland-dev libegl-dev libgbm-dev
    # text-to-speech engine (llama.cpp / ggml)
    cmake g++ git
    # browser engine (Servo + SpiderMonkey)
    clang llvm-dev libclang-dev m4 python3 gperf libunwind-dev libfreetype6-dev libfontconfig1-dev
    libharfbuzz-dev libgl1-mesa-dri libgles2-mesa-dev libegl1-mesa-dev libudev-dev libxkbcommon-dev
    libxkbcommon-x11-0 libx11-dev libxcb-render0-dev libxcb-shape0-dev libxcb-xfixes0-dev libvulkan1
  )
  # AppImage tooling runs as an AppImage itself and needs FUSE 2 (renamed on Ubuntu 24.04).
  if apt-cache show libfuse2t64 >/dev/null 2>&1; then pkgs+=(libfuse2t64); else pkgs+=(libfuse2); fi
  sudo apt-get update
  sudo apt-get install -y "${pkgs[@]}"
fi

missing=()
command -v cargo >/dev/null || missing+=("Rust — curl https://sh.rustup.rs -sSf | sh")
command -v cmake >/dev/null || missing+=("cmake — re-run with --install-deps")
pkg-config --exists webkit2gtk-4.1 2>/dev/null || missing+=("WebKitGTK 4.1 — re-run with --install-deps")
if ! command -v node >/dev/null || (( $(node -p 'process.versions.node.split(".")[0]') < 18 )); then
  missing+=("Node.js 18 or newer — e.g. curl -fsSL https://deb.nodesource.com/setup_20.x | sudo -E bash - && sudo apt-get install -y nodejs")
fi
if [[ ${#missing[@]} -gt 0 ]]; then
  echo "Missing before RAD can be built:" >&2
  printf '  - %s\n' "${missing[@]}" >&2
  exit 1
fi
cargo tauri --version >/dev/null 2>&1 || cargo install tauri-cli --version '^2' --locked

triple="$(rustc -vV | sed -n 's/^host: //p')"

# ------------------------------------------------------------------ models and engines
./scripts/fetch-models.sh
[[ $browser == 1 ]] && ./scripts/build-browser.sh "$triple"
./scripts/build-tts.sh "$triple"

# Bundle whichever engines are built for this platform. Engines from an earlier build with
# --no-browser left out on purpose stay out.
bins=()
for b in rad-browser mio-tts-server; do
  [[ $b == rad-browser && $browser == 0 ]] && continue
  [[ -f "app/src-tauri/binaries/$b-$triple" ]] && bins+=("\"binaries/$b\"")
done
cfg="$PWD/target/rad-bundle.conf.json"
mkdir -p target
(IFS=,; echo "{\"bundle\":{\"externalBin\":[${bins[*]:-}]}}") > "$cfg"

# ------------------------------------------------------------------ app
(cd app/ui && { [[ -d node_modules ]] || npm ci; })
(cd app && cargo tauri build --config "$cfg" "${args[@]}")

echo
echo "Built with: ${bins[*]:-no engines}"
for f in target/release/bundle/deb/*.deb target/release/bundle/appimage/*.AppImage target/release/bundle/rpm/*.rpm; do
  [[ -f "$f" ]] && echo "  $PWD/$f"
done
echo "Install the .deb with:  sudo apt install ./target/release/bundle/deb/<file>.deb"
