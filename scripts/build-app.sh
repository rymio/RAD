#!/usr/bin/env bash
# Builds the RAD desktop app with its engines inside the bundle:
#   - the web browser engine (Servo)        → scripts/build-browser.sh
#   - the text-to-speech engine (MioTTS)    → scripts/build-tts.sh
#   - the Linux sandbox engine (libkrun)    → scripts/build-sandbox.sh (macOS on Apple Silicon)
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
./scripts/build-tts.sh "$triple"
./scripts/build-sandbox.sh "$triple"

# Bundle whichever engines were built: executables as sidecars, libraries into Frameworks.
bins=(); libs=()
for b in rad-browser mio-tts-server rad-sandbox gvproxy; do
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
echo "Built with: ${bins[*]:-no engines}"
echo "On macOS: quit RAD, then replace /Applications/RAD.app with $PWD/$app"
