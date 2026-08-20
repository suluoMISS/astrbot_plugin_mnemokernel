# ADR-002: Identity and scope are derived, local, and non-transitive

- Status: accepted
- Date: 2026-08-19

## Context

The same display name can identify different people, one person can use several
accounts, and group messages have a different audience from private messages.
An LLM is good at suggesting associations but cannot be trusted to decide that
two platform identities are the same person. An incorrect merge would leak
memory across users or conversations.

## Decision

The canonical memory scope is the ordered tuple
`(protocol, platform_id, bot_account_id, conversation_kind, session_id,
persona_id)`. The Rust kernel validates every component and derives `scope_id`;
no model-supplied scope or database identifier is accepted as authoritative.

Private and group scopes are always distinct. Group capture is opt-in through
an explicit allowlist, and a group purge requires administrator authorization
in the AstrBot adapter before a request crosses the native boundary. Outbound
bot messages use the same scope derivation as the triggering conversation.

Display names, inferred aliases, and semantic similarity are attributes, not
identity. J1 performs no automatic cross-account, cross-group, cross-platform,
or cross-persona merge. A future user-confirmed alias can link scopes for a
specific purpose, but it cannot silently replace their separate access rules.

## Consequences

- accidental similarity cannot widen the retrieval or deletion boundary;
- the same human may initially have separate memories on separate accounts;
- model workers receive an event admission list, never authority to choose a
  scope;
- any future shared or global memory tier needs a separate ADR and explicit
  consent rather than an extension of the current scope key.
