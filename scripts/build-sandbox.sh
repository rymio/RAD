#!/usr/bin/env bash
# Builds RAD's Linux sandbox engine (macOS on Apple Silicon):
#   - libkrun 1.19.6 (Apache-2.0)      microVMs on Apple's Hypervisor.framework
#   - libkrunfw 5.6.2 (GPL/LGPL)       the Linux kernel libkrun boots, as a library
#   - gvproxy 0.9.0 (Apache-2.0)       the sandbox's private NAT network
#   - rad-sandbox                      RAD's launcher (sandbox/launcher), signed with the
#                                      hypervisor entitlement
# Output: sandbox/bin (used by `cargo tauri dev`) and app/src-tauri/{binaries,frameworks}
# (bundled by ./scripts/build-app.sh). Needs: Rust, Xcode tools, and `brew install lld llvm`.
set -euo pipefail

cd "$(dirname "$0")/.."
if [[ "$(uname -s)" != "Darwin" || "$(uname -m)" != "arm64" ]]; then
  echo "The sandbox engine is only available on macOS with Apple Silicon for now — skipping."
  exit 0
fi

KRUN_TAG=v1.19.6
KRUNFW_URL=https://github.com/containers/libkrunfw/releases/download/v5.6.2/libkrunfw-prebuilt-aarch64.tgz
KRUNFW_SHA=adef6739cc3bda2a1b57f04c08ccbfdf206a6c831d76b32de1beddc0e7984e86
GVPROXY_URL=https://github.com/containers/gvisor-tap-vsock/releases/download/v0.9.0/gvproxy-darwin
GVPROXY_SHA=e2a6825ba789fe588828b418c09aaf16525dec6d8cb2d16c94e72535dd4606be
triple="${1:-$(rustc -vV | sed -n 's/^host: //p')}"
W=sandbox/work
OUT=sandbox/bin
mkdir -p "$W" "$OUT"

# Tools libkrun needs to cross-compile its guest init: ld.lld and libclang.
find_brew() { for f in "$@"; do p="$(brew --prefix "$f" 2>/dev/null || true)"; [[ -n "$p" && -d "$p" ]] && { echo "$p"; return; }; done; }
LLD_DIR="$(dirname "$(command -v ld.lld 2>/dev/null || true)")"
if [[ ! -x "$LLD_DIR/ld.lld" ]]; then LLD_DIR="$(find_brew lld lld@21 lld@20)/bin"; fi
LLVM_LIB="$(find_brew llvm llvm@21 llvm@20)/lib"
if [[ ! -x "$LLD_DIR/ld.lld" || ! -f "$LLVM_LIB/libclang.dylib" ]]; then
  echo "error: libkrun needs lld and libclang — run: brew install lld llvm" >&2
  exit 1
fi

sha() { shasum -a 256 "$1" | cut -d' ' -f1; }
fetch() { # url sha dest
  if [[ ! -f "$3" || "$(sha "$3")" != "$2" ]]; then
    curl -fL --progress-bar -o "$3.part" "$1"
    [[ "$(sha "$3.part")" == "$2" ]] || { echo "error: checksum mismatch for $1" >&2; rm -f "$3.part"; exit 1; }
    mv "$3.part" "$3"
  fi
}

echo "==> libkrunfw (Linux kernel)"
fetch "$KRUNFW_URL" "$KRUNFW_SHA" "$W/libkrunfw.tgz"
rm -rf "$W/libkrunfw" && mkdir -p "$W/libkrunfw" && tar xzf "$W/libkrunfw.tgz" -C "$W/libkrunfw"
make -s -C "$W/libkrunfw/libkrunfw" 2>&1 | grep -v "reducing alignment" || true

echo "==> libkrun $KRUN_TAG"
if [[ ! -d "$W/libkrun/.git" ]]; then git clone --quiet --depth 1 --branch "$KRUN_TAG" https://github.com/containers/libkrun.git "$W/libkrun"; fi
PATH="$LLD_DIR:$PATH" LIBCLANG_PATH="$LLVM_LIB" RUSTFLAGS="-C link-arg=-Wl,-rpath,$LLVM_LIB" \
  make -s -C "$W/libkrun" BLK=1 NET=1

echo "==> gvproxy"
fetch "$GVPROXY_URL" "$GVPROXY_SHA" "$W/gvproxy"

echo "==> rad-sandbox launcher"
cp "$W/libkrun/target/release/libkrun.1.19.6.dylib" "$OUT/libkrun.1.dylib"
cp "$W/libkrunfw/libkrunfw/libkrunfw.5.dylib" "$OUT/libkrunfw.5.dylib"
install_name_tool -id @rpath/libkrun.1.dylib "$OUT/libkrun.1.dylib"
install -m 755 "$W/gvproxy" "$OUT/gvproxy"
clang -O2 -Wall -I"$W/libkrun/include" -L"$OUT" -lkrun.1 \
  -Wl,-rpath,@loader_path -Wl,-rpath,@loader_path/../Frameworks \
  -o "$OUT/rad-sandbox" sandbox/launcher/rad-sandbox.c
codesign --force --sign - "$OUT/libkrun.1.dylib" "$OUT/libkrunfw.5.dylib" "$OUT/gvproxy" 2>/dev/null
codesign --force --sign - --entitlements sandbox/launcher/hvf-entitlements.plist "$OUT/rad-sandbox"

# For bundles: executables as Tauri sidecars, libraries into Contents/Frameworks.
mkdir -p app/src-tauri/binaries app/src-tauri/frameworks
cp "$OUT/rad-sandbox" "app/src-tauri/binaries/rad-sandbox-$triple"
cp "$OUT/gvproxy" "app/src-tauri/binaries/gvproxy-$triple"
cp "$OUT/libkrun.1.dylib" "$OUT/libkrunfw.5.dylib" app/src-tauri/frameworks/
echo "Sandbox engine ready: $OUT"
