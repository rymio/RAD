# Rymnet Compile Guide

## Prerequisites

Install the Rust toolchain:

```bash
rustc --version
cargo --version
```

If Rust is missing, install it from:

```text
https://rustup.rs/
```

## Workspace Location

The Rust workspace is:

```bash
rymnet/
```

From the repository root:

```bash
cd rymnet
```

## Check the Workspace

Run a fast compile/type check:

```bash
cargo check --workspace
```

## Format the Code

Format all crates:

```bash
cargo fmt --all
```

## Run Tests

Run all unit and doc tests:

```bash
cargo test --workspace
```

## Build Debug Binaries

Build everything in debug mode:

```bash
cargo build --workspace
```

Debug binaries are written under:

```text
target/debug/
```

Expected binaries:

```text
target/debug/rym
target/debug/rymnetd
```

Run them directly:

```bash
./target/debug/rym --help
./target/debug/rymnetd --help
```

## Build Release Binaries

Build optimized binaries:

```bash
cargo build --workspace --release
```

Release binaries are written under:

```text
target/release/
```

Expected binaries:

```text
target/release/rym
target/release/rymnetd
```

Run:

```bash
./target/release/rym --help
./target/release/rymnetd --help
```

## Install From Local Source

Install the CLI:

```bash
cargo install --path crates/rym
```

Install the daemon:

```bash
cargo install --path crates/rymnetd
```

Then check:

```bash
rym --help
rymnetd --help
```

## Common Development Loop

Use this sequence after changes:

```bash
cargo fmt --all
cargo check --workspace
cargo test --workspace
```

For a release build:

```bash
cargo build --workspace --release
```

## Smoke Test After Compile

From the workspace:

```bash
cargo run -p rym -- --data-dir /tmp/rymnet-demo init
cargo run -p rym -- --data-dir /tmp/rymnet-demo domain claim rob.rym
cargo run -p rym -- --data-dir /tmp/rymnet-demo site create rob.rym
cargo run -p rym -- --data-dir /tmp/rymnet-demo site publish rob.rym
cargo run -p rym -- --data-dir /tmp/rymnet-demo search welcome
```

Start the daemon:

```bash
cargo run -p rymnetd -- --data-dir /tmp/rymnet-demo run
```

Then open:

```text
http://127.0.0.1:7447/rob.rym/
```

## Notes on Local Networking

`rymnetd` binds local ports:

- peer listener: `0.0.0.0:7444`
- gateway: `127.0.0.1:7447`
- admin placeholder: `127.0.0.1:7448`

If a port is already in use, change the generated config:

```text
~/.rymnet/config.toml
```

For multi-node local tests, use separate data directories and separate listen ports in each config.

## Troubleshooting

If Cargo cannot find crates or dependencies:

```bash
cargo clean
cargo check --workspace
```

If installed binaries are not found, check:

```bash
echo "$PATH"
ls ~/.cargo/bin
```

If `rymnetd` cannot bind a port, check whether another process is already listening on that port and update `config.toml`.

