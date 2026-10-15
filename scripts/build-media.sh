#!/usr/bin/env bash
# Builds RAD's media engine (media/rad-media: libVLC through vlc-rs, used by the Music and Video
# views) and lays it out ready to run, with libVLC and its plugins next to it:
#   media/rad-media/target/bundle/rad-media/{rad-media, lib/, plugins/}
# `cargo tauri dev` uses it from there; ./scripts/build-app.sh puts it inside the app.
#
# libVLC comes from:
#   macOS: VLC.app (`brew install --cask vlc`); VLC_APP overrides /Applications/VLC.app.
#   Linux: the system's libVLC (`apt install libvlc-dev vlc-plugin-base`); it isn't copied, so
#          the computer running RAD needs libVLC installed too.
set -euo pipefail

cd "$(dirname "$0")/../media/rad-media"
out=target/bundle/rad-media

case "$(uname -s)" in
  Darwin)
    vlc="${VLC_APP:-/Applications/VLC.app}/Contents/MacOS"
    if [[ ! -f "$vlc/lib/libvlc.dylib" ]]; then
      echo "VLC isn't installed (no $vlc/lib/libvlc.dylib). Install it with: brew install --cask vlc" >&2
      exit 1
    fi
    LIBVLC_LIB_DIR="$vlc/lib" cargo build --release
    rm -rf "$out"
    mkdir -p "$out/lib"
    cp target/release/rad-media "$out/"
    # libvlc + libvlccore (the versioned names are what the libraries refer to each other by).
    cp -a "$vlc/lib/"*.dylib "$out/lib/"
    ditto "$vlc/plugins" "$out/plugins"
    # The VLC.app interface isn't used by the engine.
    rm -f "$out/plugins/libmacosx_plugin.dylib"
    # Only look for libVLC next to the engine, never in an installed VLC.app.
    install_name_tool -delete_rpath "$vlc/lib" "$out/rad-media" 2>/dev/null || true
    # VLC.app came from a download: without this, macOS checks every one of the ~340 libraries
    # the first time each is loaded, and the engine can take a minute to start.
    xattr -dr com.apple.quarantine "$out" 2>/dev/null || true
    # libVLC's plugin cache: starts then load only the plugins they need.
    VLC_PLUGIN_PATH="$PWD/$out/plugins" "$out/rad-media" --gen-cache
    ;;
  Linux)
    cargo build --release
    rm -rf "$out"
    mkdir -p "$out"
    cp target/release/rad-media "$out/"
    ;;
  *)
    echo "Building the media engine on this system isn't scripted yet: build media/rad-media with LIBVLC_LIB_DIR set to the VLC SDK's lib folder." >&2
    exit 1
    ;;
esac

echo "Media engine ready: media/rad-media/$out ($(du -sh "$out" | cut -f1))"
