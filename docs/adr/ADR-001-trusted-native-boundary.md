# ADR-001: Rust owns the trusted memory boundary

- Status: accepted
- Date: 2026-08-19

## Context

Long-term memory fails in damaging ways when generated summaries can silently
become facts, old values can be overwritten without lineage, or retrieval code
can cross user and conversation boundaries. Python is suitable for AstrBot
integration and model orchestration, but a direct Python-to-database path would
make the policy optional in practice.

## Decision

The AstrBot plugin is a thin Python adapter. A Rust kernel owns canonical IDs,
scope derivation, schema validation, evidence admission, state transitions,
transactions, and audit records. LLM-based workers may only submit typed
proposals. They never receive database credentials and never commit mutations.

The boundary starts with a versioned JSON protocol through PyO3. This is less
efficient than sharing in-process object layouts, but gives both languages an
inspectable contract and keeps ABI evolution explicit. More compact transport
can be introduced after profiling without changing domain semantics.

## Consequences

- missing or incompatible native code disables memory but not ordinary chat;
- platform adaptation can change without weakening storage rules;
- native wheels must be produced for supported AstrBot platforms;
- the kernel becomes a small security-sensitive component with stricter tests
  and release controls than the surrounding orchestration code.

