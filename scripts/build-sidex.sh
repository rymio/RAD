#!/usr/bin/env bash
# Builds SideX (MIT, https://github.com/Sidenai/sidex) — VS Code's workbench on Tauri — for
# RAD's "SideX" view, with RAD's small patch (scripts/sidex/rad.patch):
#   - `SideX --folder PATH` opens that folder
#   - one SideX at a time: launching it again opens the folder in a window of the running one
# Output: sidex/target/release/bundle/macos/SideX.app (build-app.sh copies it into RAD's
# bundle as Contents/Helpers/SideX.app). Needs: Node 20+, Rust. Optional: Go 1.26+ for SideX's
# own AI agent server (the editor works without it).
set -euo pipefail

cd "$(dirname "$0")/.."
SIDEX_REPO=https://github.com/Sidenai/sidex.git
SIDEX_REV=ad978652682270a593e7029269beff5411477a70
D=sidex

if [[ "$(uname -s)" != "Darwin" ]]; then
  echo "SideX is only bundled with RAD on macOS for now — skipping."
  exit 0
fi
command -v npm >/dev/null || { echo "error: SideX needs Node.js (npm)" >&2; exit 1; }

echo "==> SideX source ($SIDEX_REV)"
if [[ ! -d "$D/.git" ]]; then
  git clone --quiet "$SIDEX_REPO" "$D"
fi
if [[ "$(git -C "$D" rev-parse HEAD)" != "$SIDEX_REV" ]]; then
  git -C "$D" fetch --quiet --depth 1 origin "$SIDEX_REV"
  git -C "$D" checkout --quiet --force "$SIDEX_REV"
fi
# Apply RAD's patch unless it's already there.
if git -C "$D" apply --check ../scripts/sidex/rad.patch 2>/dev/null; then
  git -C "$D" apply ../scripts/sidex/rad.patch
elif ! git -C "$D" apply --reverse --check ../scripts/sidex/rad.patch 2>/dev/null; then
  echo "error: RAD's SideX patch doesn't apply — reset $D (git -C $D checkout -- .) and try again" >&2
  exit 1
fi

echo "==> SideX dependencies"
(cd "$D" && npm install --no-audit --no-fund --loglevel=error)

if command -v go >/dev/null; then
  echo "==> SideX agent server (Go)"
  (cd "$D" && npm run -s server:build)
else
  echo "    (Go not installed: building SideX without its own AI agent server)"
fi

echo "==> SideX app"
# No updater artifacts: those need SideX's release signing key.
(cd "$D" && npm run -s tauri -- build --bundles app)
app="$D/target/release/bundle/macos/SideX.app"
[[ -d "$app" ]] || { echo "error: SideX.app wasn't built" >&2; exit 1; }
echo "SideX ready: $app"
