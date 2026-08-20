# ADR-003: Event, observation, validity, and recording time are distinct

- Status: accepted
- Date: 2026-08-19

## Context

Human-like memory is temporal: a message can arrive late, describe an older
event, revise a previously valid belief, or be summarized hours later. Treating
all of these as one timestamp makes daily journals unstable and turns old facts
into apparently current facts.

## Decision

MnemoKernel uses four time concepts:

1. `occurred_at_ms` is when the source event occurred according to the trusted
   platform adapter.
2. `observed_at_ms` is when AstrBot observed that event.
3. `created_at_ms`/`ingested_at_ms` is when the kernel recorded a row.
4. `valid_from_ms` and `valid_to_ms` describe when an L2 claim is believed to
   apply; they are introduced only with evidence-backed atomic memory in J2.

J1 Episode bounds are derived by the kernel from the minimum and maximum
`occurred_at_ms` of admitted evidence. A model may mention relative time in
prose, but cannot supply authoritative timestamps, dates, time zones, or
validity intervals. Late events may cause a deterministic rebuild of an older
journal; they are not silently moved to the observation day.

Journal dates are projections of `occurred_at_ms` through one configured IANA
time zone. The configured zone and rendering version are part of export audit
metadata. Ordering ties are broken by stable event IDs so identical inputs
produce byte-identical journals.

## Consequences

- late delivery and historical narration remain distinguishable;
- daily summaries can be rebuilt without depending on model interpretation;
- current-view logic can later supersede a claim without erasing its history;
- platform timestamps that are absent or invalid must be flagged by the
  adapter and replaced according to a documented fallback, never guessed by
  the LLM.
