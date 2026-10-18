# ADR 0005: Consume-before-press on the host helper; one staleness barrier

- Status: accepted
- Date: 2026-10-06 (Asia/Manila)
- Baseline: `87ffc2d` (impeccable-rust audit of the Agent + Instinct + ticket path)
- Relates: ADR 0001, ADR 0003 §1 / §7, ADR 0004 §2

## Context

The read-only audit found two places where the code said less, or more, than
the ticket boundary in ADR 0003 actually guarantees:

- **R1.** `ultra_instinct_agent::execute_ticketed` marks a ticket consumed
  **before** dispatch (ADR 0003 §1), but the host helper
  `ultra_instinct_guard::consume_ticket_once` pressed first and marked consumed
  only after `press` returned `Ok`. A press that failed after reaching the page
  left the lease unconsumed, so the host could replay the same ticket — and
  its doc comment claimed the opposite ("mark consumed even if press fails").
- **R2.** `Predicted` carried `observation_fingerprint` (whole-page
  `WorldSnapshot::of`) and `text_fingerprint`. Neither was ever read. Public
  fields named like fingerprints imply a second pre-ticket stale check that
  does not exist.

## Decision

1. **R1 — one consume order everywhere.** `consume_ticket_once` is now:
   ledger check → `revalidate` → **mark consumed** → `press`. A stale ticket is
   refused without being consumed (re-observe, re-decide, new ticket). A
   press error after consumption returns `ConsumeError::Press`; the next call
   with the same ticket returns `ticket-consumed` without pressing. This is
   the same order as `execute_ticketed`.
2. **R2 — remove, do not wire.** The two fields are deleted from `Predicted`.
   Wiring them would not add safety and would add false refusals:
   - `observation_fingerprint` hashed the **whole page**. ADR 0004 §2 made
     ticket worlds **target-scoped** precisely so an unrelated banner does
     not force a stale discard. A whole-page pre-check would undo that.
   - The ticket already binds everything the text context was built from:
     target role, label, and region fingerprint (via `revalidate`), plus the
     target-scoped world. The goal clause cannot change between predict and
     act. The resolution's own context fingerprint is still checked once, at
     resolution time (`resolution belongs to a different context`).

   So the agent path has exactly **one** staleness barrier: the
   `ActionTicket` issued by `gate` on the decided observation and revalidated
   by `execute_ticketed` against a fresh observation. `Predicted`'s doc
   comment says so.

## Consequences

- Breaking for any external reader of `Predicted::{observation_fingerprint,
  text_fingerprint}` (none in the workspace; API is 0.1 / `publish = false`).
- Host harnesses using `consume_ticket_once` can no longer retry a failed
  press with the same ticket; they must issue a new one. Regression test:
  `consume_ticket_once_marks_consumed_before_press_so_failed_press_cannot_retry`.
- Stale vs spent is pinned by property tests
  (`guard/tests/ticket_props.rs`, `agent/tests/props.rs`): only
  `WorldChanged` / `TargetChanged` / `TargetGone` are stale; a consumed ticket
  is always `ticket-consumed`, even when the world also drifted.
