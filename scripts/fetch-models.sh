#!/usr/bin/env bash
# Downloads RAD's built-in GGUF models into ./models (git-ignored) and verifies them.
# The desktop bundle ships these files; the server and tests use them instead of downloading.
set -euo pipefail

cd "$(dirname "$0")/.."
mkdir -p models

# file  sha256  url — keep in sync with crates/rad-local/src/lib.rs (CATALOG)
MODELS=(
  "nomic-embed-text-v1.5.Q8_0.gguf 3e24342164b3d94991ba9692fdc0dd08e3fd7362e0aacc396a9a5c54a544c3b7 https://huggingface.co/nomic-ai/nomic-embed-text-v1.5-GGUF/resolve/main/nomic-embed-text-v1.5.Q8_0.gguf"
)

sha256() {
  if command -v sha256sum >/dev/null; then sha256sum "$1" | cut -d' ' -f1; else shasum -a 256 "$1" | cut -d' ' -f1; fi
}

for entry in "${MODELS[@]}"; do
  read -r file sum url <<<"$entry"
  if [[ -f "models/$file" ]] && [[ "$(sha256 "models/$file")" == "$sum" ]]; then
    echo "ok        models/$file"
    continue
  fi
  echo "fetching  models/$file"
  curl -fL --retry 3 -o "models/$file.part" "$url"
  if [[ "$(sha256 "models/$file.part")" != "$sum" ]]; then
    rm -f "models/$file.part"
    echo "checksum mismatch for $file" >&2
    exit 1
  fi
  mv "models/$file.part" "models/$file"
done
