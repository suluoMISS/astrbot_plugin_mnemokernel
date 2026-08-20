# MnemoKernel native workspace

The native workspace is the trusted boundary. It owns canonical IDs, protocol
validation, state transitions, evidence constraints, SQLite transactions, and
audit records. Python must never bypass it with direct database writes.

Crates:

- `mnemokernel-core`: pure domain rules, no Python or database dependency;
- `mnemokernel-store`: transactional SQLite implementation;
- `mnemokernel-py`: minimal PyO3 JSON bridge exposed as `_mnemokernel`.

Run `cargo test --workspace` with a Rust toolchain. Building a distributable
Python wheel will be added with the cross-platform CI milestone.

