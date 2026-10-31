#!/usr/bin/env bash
# Builds the RAD desktop app with its engines inside the bundle:
#   - the web browser engine (Servo)        → scripts/build-browser.sh
#   - the Chromium engine (CEF)             → scripts/build-chromium.sh (skip: RAD_NO_CHROMIUM=1)
#   - the media engine (libVLC, Music/Video) → scripts/build-media.sh   (skip: RAD_NO_MEDIA=1; needs VLC)
#   - the text-to-speech engine (MioTTS)    → scripts/build-tts.sh
#   - the Linux sandbox engine (libkrun)    → scripts/build-sandbox.sh (macOS on Apple Silicon)
#   - SideX, the code editor (VS Code)      → scripts/build-sidex.sh   (skip: RAD_NO_SIDEX=1; macOS)
#   - ZX Spectrum dev tools (sjasmplus)     → scripts/build-zx-tools.sh
#   - Commodore 64 dev tools (64tass)       → scripts/build-c64-tools.sh
#   - the chess engine (RustChess)          → scripts/build-chess.sh   (Games → Chess)
#   - the DOS engine (DOSBox Pure)          → scripts/build-dos.sh     (skip: RAD_NO_DOS=1)
# All are incremental, so re-running is quick. Extra arguments go to `cargo tauri build`
# (e.g. --bundles app). Result: target/release/bundle/…
#
# On macOS the .app is built and signed first, then the .dmg is made from the signed app with
# hdiutil (Tauri's own DMG step runs before signing and scripts Finder, which can fail and
# leave half-built images mounted).
set -euo pipefail

cd "$(dirname "$0")/.."
triple="$(rustc -vV | sed -n 's/^host: //p')"

./scripts/build-browser.sh "$triple"
[[ "${RAD_NO_TOR:-}" == 1 ]] || ./scripts/build-tor-browser.sh "$triple"
[[ "${RAD_NO_CHROMIUM:-}" == 1 ]] || ./scripts/build-chromium.sh
# Without VLC installed the app is still built; Music and Video then say the engine is missing.
[[ "${RAD_NO_MEDIA:-}" == 1 ]] || ./scripts/build-media.sh || echo "warning: building without the media engine (Music, Video)" >&2
./scripts/build-tts.sh "$triple"
./scripts/build-sandbox.sh "$triple"
./scripts/build-zx-tools.sh || echo "warning: building without the ZX Spectrum assembler" >&2
./scripts/build-c64-tools.sh || echo "warning: building without the C64 assembler" >&2
./scripts/build-chess.sh "$triple" || echo "warning: building without the chess engine" >&2
[[ "${RAD_NO_DOS:-}" == 1 ]] || ./scripts/build-dos.sh || echo "warning: building without the DOS engine" >&2
[[ "${RAD_NO_SIDEX:-}" == 1 ]] || ./scripts/build-sidex.sh || echo "warning: building without SideX (the code editor)" >&2

# Bundle whichever engines were built: executables as sidecars, libraries into Frameworks.
bins=(); libs=()
for b in rad-browser rad-tor-browser mio-tts-server rad-sandbox gvproxy rad-chess; do
  [[ -f "app/src-tauri/binaries/$b-$triple" || -f "app/src-tauri/binaries/$b-$triple.exe" ]] && bins+=("\"binaries/$b\"")
done
for l in libkrun.1.dylib libkrunfw.5.dylib; do
  [[ -f "app/src-tauri/frameworks/$l" ]] && libs+=("\"frameworks/$l\"")
done
cfg="$PWD/target/rad-bundle.conf.json"
mkdir -p target
(IFS=,; echo "{\"bundle\":{\"externalBin\":[${bins[*]:-}],\"macOS\":{\"frameworks\":[${libs[*]:-}]}}}") > "$cfg"

# macOS: build the .app here and make the .dmg ourselves after signing.
args=()
want_dmg=0
if [[ "$(uname -s)" == "Darwin" ]]; then
  want_dmg=1
  have_bundles=0
  while [[ $# -gt 0 ]]; do
    if [[ "$1" == "--bundles" || "$1" == "-b" ]] && [[ $# -ge 2 ]]; then
      have_bundles=1
      b=",$2,"
      [[ "$b" == *",dmg,"* ]] || want_dmg=0
      b="${b//,dmg,/,app,}"; b="${b#,}"; b="${b%,}"
      args+=(--bundles "$b")
      shift 2
    else
      args+=("$1")
      shift
    fi
  done
  [[ $have_bundles == 1 ]] || args+=(--bundles app)
  # Leftovers of interrupted DMG builds: detach and remove them.
  hdiutil info | awk '/^image-path/ {p=$3} /\/Volumes\/dmg\./ && p ~ /target\/release\/bundle/ {print $NF}' | while read -r v; do hdiutil detach "$v" -force -quiet || true; done
  rm -f target/release/bundle/macos/rw.*.dmg
else
  args=("$@")
fi

(cd app/ui && { [[ -d node_modules ]] || npm ci; })
(cd app && cargo tauri build --config "$cfg" ${args[@]+"${args[@]}"})

app="target/release/bundle/macos/RAD.app"
chromium="browser/rad-chromium/target/bundle/rad-chromium.app"
if [[ -d "$app" && -d "$chromium" && "${RAD_NO_CHROMIUM:-}" != 1 ]]; then
  # The Chromium engine is an app of its own (CEF framework + helper apps); it goes where macOS
  # expects nested apps, and is signed inside-out like everything else.
  rm -rf "$app/Contents/Helpers/rad-chromium.app"
  mkdir -p "$app/Contents/Helpers"
  ditto "$chromium" "$app/Contents/Helpers/rad-chromium.app"
  identity="${APPLE_SIGNING_IDENTITY:--}"
  ents=browser/rad-chromium/entitlements.plist
  cef="$app/Contents/Helpers/rad-chromium.app/Contents/Frameworks"
  for f in "$cef/Chromium Embedded Framework.framework/Libraries/"*.dylib; do [[ -f "$f" ]] && codesign --force --sign "$identity" "$f"; done
  codesign --force --sign "$identity" "$cef/Chromium Embedded Framework.framework"
  for h in "$cef/"*Helper*.app; do codesign --force --sign "$identity" --entitlements "$ents" "$h"; done
  codesign --force --sign "$identity" --entitlements "$ents" "$app/Contents/Helpers/rad-chromium.app"
fi
media=media/rad-media/target/bundle/rad-media
if [[ -d "$app" && -x "$media/rad-media" && "${RAD_NO_MEDIA:-}" != 1 ]]; then
  # The media engine with libVLC and its plugins, signed with the app's identity (macOS only
  # loads libraries signed like the process that loads them).
  rm -rf "$app/Contents/Helpers/rad-media"
  mkdir -p "$app/Contents/Helpers"
  ditto "$media" "$app/Contents/Helpers/rad-media"
  identity="${APPLE_SIGNING_IDENTITY:--}"
  find "$app/Contents/Helpers/rad-media" -name '*.dylib' -type f -print0 | xargs -0 codesign --force --sign "$identity"
  codesign --force --sign "$identity" "$app/Contents/Helpers/rad-media/rad-media"
fi
if [[ -d "$app" && -x zxtools/bin/sjasmplus ]]; then
  # The ZX Spectrum assembler (Games view), signed with the app's identity.
  mkdir -p "$app/Contents/Helpers/zx"
  cp zxtools/bin/sjasmplus zxtools/bin/sjasmplus-LICENSE.md "$app/Contents/Helpers/zx/"
  codesign --force --sign "${APPLE_SIGNING_IDENTITY:--}" "$app/Contents/Helpers/zx/sjasmplus"
fi
if [[ -d "$app" && -x c64tools/bin/64tass ]]; then
  # The C64 assembler (Games view, C64 lab), signed with the app's identity.
  mkdir -p "$app/Contents/Helpers/c64"
  cp c64tools/bin/64tass c64tools/bin/64tass-LICENSE.txt "$app/Contents/Helpers/c64/"
  codesign --force --sign "${APPLE_SIGNING_IDENTITY:--}" "$app/Contents/Helpers/c64/64tass"
fi
dos=dos/rad-dos/target/bundle/rad-dos
if [[ -d "$app" && -x "$dos/rad-dos" && "${RAD_NO_DOS:-}" != 1 ]]; then
  # The DOS engine (DOS view): its own program with DOSBox Pure next to it.
  rm -rf "$app/Contents/Helpers/rad-dos"
  ditto "$dos" "$app/Contents/Helpers/rad-dos"
  codesign --force --sign "${APPLE_SIGNING_IDENTITY:--}" "$app/Contents/Helpers/rad-dos/dosbox_pure_libretro.dylib"
  codesign --force --sign "${APPLE_SIGNING_IDENTITY:--}" "$app/Contents/Helpers/rad-dos/rad-dos"
fi
sidex=sidex/target/release/bundle/macos/SideX.app
if [[ -d "$app" && -d "$sidex" && "${RAD_NO_SIDEX:-}" != 1 ]]; then
  # SideX, the code editor (SideX view): its own app inside RAD's, signed with the app's identity.
  rm -rf "$app/Contents/Helpers/SideX.app"
  mkdir -p "$app/Contents/Helpers"
  ditto "$sidex" "$app/Contents/Helpers/SideX.app"
  codesign --force --deep --sign "${APPLE_SIGNING_IDENTITY:--}" "$app/Contents/Helpers/SideX.app"
fi
rymnet=rymnet/target/release
if [[ -d "$app" && -x "$rymnet/rymwin" && "${RAD_NO_RYMNET:-}" != 1 ]]; then
  # The Rymnet programs (RYMNET and RYMWIN views): rymwin, rymnetd, rym and rymsh, signed with the app's identity.
  rm -rf "$app/Contents/Helpers/rymnet"
  mkdir -p "$app/Contents/Helpers/rymnet"
  identity="${APPLE_SIGNING_IDENTITY:--}"
  for b in rym rymnetd rymwin rymsh; do
    cp "$rymnet/$b" "$app/Contents/Helpers/rymnet/"
    codesign --force --sign "$identity" "$app/Contents/Helpers/rymnet/$b"
  done
  # rymwin's PDF Reader looks for libpdfium next to it.
  for f in "$rymnet"/libpdfium.dylib; do [[ -f "$f" ]] && cp "$f" "$app/Contents/Helpers/rymnet/" && codesign --force --sign "$identity" "$app/Contents/Helpers/rymnet/libpdfium.dylib"; done
fi
if [[ -d "$app" && -f sandbox/bin/rad-agent ]]; then
  # The containers' agent (a Linux program the VMs run): a resource, not a macOS executable.
  install -m 755 sandbox/bin/rad-agent "$app/Contents/Resources/rad-agent"
fi
if [[ -d "$app" ]]; then
  # Without a signing identity Tauri leaves the bundle unsealed. Sign it inside-out — libraries,
  # helpers (the sandbox launcher with its hypervisor entitlement), then the app — so the
  # signature is valid and the launcher keeps permission to use Hypervisor.framework.
  identity="${APPLE_SIGNING_IDENTITY:--}"
  for f in "$app"/Contents/Frameworks/*.dylib; do [[ -f "$f" ]] && codesign --force --sign "$identity" "$f"; done
  for f in "$app"/Contents/MacOS/*; do
    [[ "$(basename "$f")" == "rad-app" ]] && continue
    if [[ "$(basename "$f")" == "rad-sandbox" ]]; then
      codesign --force --sign "$identity" --entitlements sandbox/launcher/hvf-entitlements.plist "$f"
    else
      codesign --force --sign "$identity" "$f"
    fi
  done
  codesign --force --sign "$identity" "$app"
  codesign --verify --deep --strict "$app"
fi

if [[ $want_dmg == 1 && -d "$app" ]]; then
  version="$(sed -n 's/^  "version": *"\(.*\)",*$/\1/p' app/src-tauri/tauri.conf.json | head -1)"
  arch="$(uname -m | sed 's/arm64/aarch64/')"
  dmg="target/release/bundle/dmg/RAD_${version}_${arch}.dmg"
  stage="$(mktemp -d)"
  ditto "$app" "$stage/RAD.app"
  ln -s /Applications "$stage/Applications"
  mkdir -p "$(dirname "$dmg")"
  hdiutil create -quiet -volname RAD -srcfolder "$stage" -ov -format UDZO "$dmg"
  rm -rf "$stage"
  echo "Disk image: $PWD/$dmg"
fi

echo
[[ -d "$app/Contents/Helpers/rad-chromium.app" ]] && bins+=("rad-chromium")
[[ -d "$app/Contents/Helpers/rad-media" ]] && bins+=("rad-media")
[[ -d "$app/Contents/Helpers/rymnet" ]] && bins+=("rymnet")
echo "Built with: ${bins[*]:-no engines}"
echo "On macOS: quit RAD, then replace /Applications/RAD.app with $PWD/$app"
