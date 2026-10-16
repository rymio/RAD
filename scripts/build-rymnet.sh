#!/usr/bin/env bash
# Builds the Rymnet programs RAD drives from its RYMNET view — rymwin (the Rymnet Desktop),
# rymnetd (the peer daemon), rym (the CLI) and rymsh (the shell) — from RAD's own `rymnet/` workspace, ready to run:
#   rymnet/target/release/{rym, rymnetd, rymwin, rymsh}
# `cargo tauri dev` uses them from there; ./scripts/build-app.sh puts them inside the app.
set -euo pipefail

cd "$(dirname "$0")/../rymnet"
cargo build --release -p rym -p rymnetd -p rymwin -p rymsh
echo "Rymnet: $PWD/target/release/{rym,rymnetd,rymwin,rymsh}"
