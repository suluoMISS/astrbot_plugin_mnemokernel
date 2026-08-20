# Building the native kernel

MnemoKernel does not provide a Python database fallback. A valid native build is
a runtime capability, not an optional performance optimization.

## Pinned tools

- Rust: 1.97.1 from rust-toolchain.toml
- Maturin: 1.14.1
- PyO3: resolved by rust/Cargo.lock
- Stable Python ABI: abi3-py310

The repository-local .tools directory is only a development convenience and is
ignored. CI installs the same pinned versions independently.

## Rust quality gate

From the repository root:

    ./scripts/check-native.sh

Without the helper:

    cd rust
    cargo fmt --all -- --check
    cargo test --workspace --locked
    cargo clippy --workspace --all-targets --locked -- -D warnings

The PyO3 crate disables Cargo's Rust test harness. Its behavior is exercised by
building and importing the wheel; domain and store behavior remain covered in
their dependency-free Rust test crates.

## Build a wheel

    cd rust/crates/mnemokernel-py
    maturin build --release --locked --out ../../../dist/native-current

The wheel must then be installed in a clean Python environment:

    python scripts/smoke_native_wheel.py dist/native-current

The smoke test opens a temporary Schema 4 database, checks capabilities, writes
one event through PyO3, stores fact and preference claims, recalls both through
their typed paths, exercises privacy controls, purges the scope, verifies replay
does not resurrect the payload, scans the compacted database for marker plaintext,
and verifies that a closed kernel rejects further calls.

## Historical verified target

An earlier Schema 2 development baseline produced and imported:

    mnemokernel_native-0.1.0a1-cp310-abi3-manylinux_2_34_x86_64.whl

That wheel predates the current Schema 4 sources and must not be shipped. The
current alpha.4 wheel is generated under dist/native-current. The release ZIP
bundles the ABI3 runtime extracted from that wheel, so AstrBot does not need to
install a local requirement during plugin upload.

Windows x86_64 remains gated on the workflow in
.github/workflows/native.yml; a workflow file existing is not evidence that the
hosted job has passed.

The release ZIP may contain the verified ABI3 runtime under its private
`native_runtime/` directory. Do not copy a raw .so, .dll, or .pyd into an
unrelated AstrBot installation; use the generated ZIP or wheel and retain its
checksum/provenance.
