# MnemoKernel native workspace

The native workspace is the trusted boundary. It owns canonical IDs, protocol
validation, state transitions, evidence constraints, SQLite transactions, and
audit records. Python must never bypass it with direct database writes.

Crates:

- `mnemokernel-core`: pure domain rules, no Python or database dependency;
- `mnemokernel-store`: transactional SQLite implementation;
- `mnemokernel-py`: minimal PyO3 JSON bridge exposed as `_mnemokernel`.

Run `cargo test --workspace` with a Rust toolchain. The PyO3 crate is released
as a Python 3.10+ ABI3 wheel for Linux x86_64 and Windows x86_64; the wheel
also remains directly smoke-testable before it is bundled into an AstrBot ZIP.
