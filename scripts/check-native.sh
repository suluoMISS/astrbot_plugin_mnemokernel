#!/usr/bin/env bash
set -euo pipefail

MNEMO_ROOT="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)"
LOCAL_CARGO="$MNEMO_ROOT/.tools/cargo"
LOCAL_RUSTUP="$MNEMO_ROOT/.tools/rustup"

if [[ -x "$LOCAL_CARGO/bin/cargo" ]]; then
    export CARGO_HOME="$LOCAL_CARGO"
    export RUSTUP_HOME="$LOCAL_RUSTUP"
    export PATH="$LOCAL_CARGO/bin:$PATH"
fi

if ! command -v cargo >/dev/null 2>&1; then
    echo "Rust is unavailable. Install the toolchain pinned by rust-toolchain.toml." >&2
    exit 1
fi

cd "$MNEMO_ROOT/rust"
cargo fmt --all -- --check
cargo test --workspace --locked
cargo clippy --workspace --all-targets --locked -- -D warnings

if command -v maturin >/dev/null 2>&1; then
    cd "$MNEMO_ROOT/rust/crates/mnemokernel-py"
    maturin build --release --locked --out "$MNEMO_ROOT/dist/native-current"
else
    echo "maturin is unavailable; Rust checks passed, wheel build skipped." >&2
fi
