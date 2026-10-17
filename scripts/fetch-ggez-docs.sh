#!/usr/bin/env bash
# Fetches the ggez 0.10.0 documentation into assets/ggez-docs/ (committed, so it's available
# offline). The Games view's agent searches it (app/src-tauri/src/games.rs → game_docs):
#   docs/      the guides, FAQ and platform notes
#   examples/  the official examples (all written for 0.10)
#   src/       the library source: its doc comments are the API reference (what docs.rs shows)
# ggez is MIT-licensed; its LICENSE is kept alongside.
set -euo pipefail

cd "$(dirname "$0")/.."
TAG=0.10.0
SHA=645f68de781bc887a3f52f0a1295d1fd8c2abd12ec657c5948193643bb8cfa14
OUT=assets/ggez-docs
tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT

curl -sfL -o "$tmp/ggez.tgz" "https://codeload.github.com/ggez/ggez/tar.gz/refs/tags/$TAG"
[[ "$(shasum -a 256 "$tmp/ggez.tgz" | cut -d' ' -f1)" == "$SHA" ]] || { echo "error: checksum mismatch for ggez $TAG" >&2; exit 1; }
tar xzf "$tmp/ggez.tgz" -C "$tmp"
src="$tmp/ggez-$TAG"

rm -rf "$OUT"
mkdir -p "$OUT"
cp "$src/README.md" "$src/CHANGELOG.md" "$src/LICENSE" "$OUT/"
rsync -a --include='*/' --exclude='*.skt.md' --include='*.md' --exclude='*' "$src/docs/" "$OUT/docs/"
mkdir -p "$OUT/examples" && cp "$src"/examples/*.rs "$OUT/examples/"
rsync -a --include='*/' --include='*.rs' --exclude='*' "$src/src/" "$OUT/src/"
echo "ggez $TAG" > "$OUT/VERSION"
echo "ggez docs: $(find "$OUT" -type f | wc -l | tr -d ' ') files, $(du -sh "$OUT" | cut -f1) in $OUT"
