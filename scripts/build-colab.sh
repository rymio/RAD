#!/usr/bin/env bash
# Builds the Rust Colab kernel (colab/rad-rust-kernel: evcxr behind JSON lines) and places it where
# the app finds it:
#   - development (`cargo tauri dev`) uses colab/rad-rust-kernel/target/release/rad-rust-kernel;
#   - bundles: scripts/build-app.sh ships the copy in app/src-tauri/binaries/ next to the RAD executable.
# The kernel needs Rust (cargo, rustc) on the computer at run time too: evcxr compiles each cell.
# evcxr loads every cell it compiles as a dylib into its runtime (this same executable), so a
# notarized build with the hardened runtime must give the kernel the
# com.apple.security.cs.disable-library-validation entitlement.
# Python Colab needs no build: its kernel (app/src-tauri/src/colab_kernel.py) is inside the app.
set -euo pipefail

cd "$(dirname "$0")/.."
triple="${1:-$(rustc -vV | sed -n 's/^host: //p')}"
ext=""
[[ "$triple" == *windows* ]] && ext=".exe"

cargo build --release --manifest-path colab/rad-rust-kernel/Cargo.toml --target "$triple"
mkdir -p app/src-tauri/binaries
cp "colab/rad-rust-kernel/target/$triple/release/rad-rust-kernel$ext" "app/src-tauri/binaries/rad-rust-kernel-$triple$ext"
# Plain path for `cargo tauri dev`.
mkdir -p colab/rad-rust-kernel/target/release
cp "colab/rad-rust-kernel/target/$triple/release/rad-rust-kernel$ext" "colab/rad-rust-kernel/target/release/rad-rust-kernel$ext"
echo "Rust Colab kernel ready: app/src-tauri/binaries/rad-rust-kernel-$triple$ext"
