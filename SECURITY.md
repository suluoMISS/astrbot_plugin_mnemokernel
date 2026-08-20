# Security model

MnemoKernel treats platform messages, LLM output, tool arguments, imported
memories, and semantic-worker proposals as untrusted input.

## Trust boundary

Only the Rust kernel may create canonical IDs, validate evidence references,
perform state transitions, or write official memory tables. The Python AstrBot
adapter must not open the SQLite database directly. If `_mnemokernel` cannot be
loaded, memory is unavailable; there is no permissive fallback.

## Invariants in the foundation milestone

- scopes include platform, bot account, conversation kind, session, and persona;
- source events are idempotent within a scope;
- changing content under an existing source key is a hard conflict;
- raw events are append-only at the database layer;
- memory proposals must cite admitted raw event IDs;
- terminal memory states cannot be reactivated;
- recall attempts are audited even when nothing is returned;
- diagnostic logs do not include message content.
- privacy-sensitive event fields are stored in a revocable payload table;
- scope purge is native-only, audited, checkpointed, and compacted;
- replaying migrations or source events cannot resurrect purged payloads.

## Known limitations

- data-at-rest encryption and cryptographic key destruction are not implemented;
- filesystem snapshots, third-party backups, and SSD wear levelling are outside
  the logical purge guarantee;
- group membership is represented by a scope boundary, not yet by per-viewer ACLs;
- stable memory cards are materialized from validated journal claims, but semantic
  supersession and activation-decay workers are not implemented in this milestone;
- wheel signing and a Windows CI result are not yet available.

Do not enable event capture for highly sensitive conversations until encryption,
retention, export cleanup, and backup handling have been implemented and reviewed.
