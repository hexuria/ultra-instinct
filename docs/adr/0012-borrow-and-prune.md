# ADR 0012: Borrow from agent-browser, prune what the agent loop outgrew

- Status: accepted
- Date: 2026-11-08 (Asia/Manila)
- Relates: ADR 0001, ADR 0003; AGENTS.md anti-drift rules 3 and 9
- Supersedes: the ranked `guard()` (`0.55` / `0.05` float gate) and the HGRA
  freeze ("HGRA stays frozen") recorded in AGENTS.md and `docs/DECISIONS.md`

## Context

The owner asked to clean up Ultra-Instinct and to take what is worth taking
from [vercel-labs/agent-browser](https://github.com/vercel-labs/agent-browser),
expecting that some of our own features would have to go. A side-by-side
comparison of the two tools produced one card per idea, and the owner marked
each card:

- **take**: a compact one-line-per-control page snapshot, a session daemon
  that one shell agent drives with short `aui` verbs, a guard policy (allowed
  domains, presses that need a confirm), `wait for` a text / URL / control,
  and a `skills` command that prints the guide shipped in the binary;
- **later**: launching Chrome from `aui`, saved login state;
- **skip**: other browser engines, live streaming;
- **drop**: the deprecated helpers (`locate`, `inspect`, `act`, `diff`), the
  ranked guard, the compare command, and the stale result tables;
- **keep**: the browser-use alias tools.

Since ADR 0001 the agent owns the loop and Instinct chooses among a finite
`ActionSpace`; nothing on that path ranks labels. What still ranked was the
host preflight: MCP / CLI `locate` and `guard` called
`ultra-instinct-resonance`'s `WeightedMatcher`, `guard` added the `0.55` /
`0.05` float gate and a buried-label rule on top, and the frozen HGRA matcher
(`ultra-instinct-hyper`) was still built, tested and published so the
resonance `hgra` feature resolved. Those existed for the historical A5 / A6
bench arms. They were a second, scored way to pick a target beside the hard
gate.

## Decision

### Borrow

Recorded with the borrow change (claims 1–4).

### Prune

1. **One hard gate, no score.** `guard()` finds its target by exact label: a
   region matches when its label equals the query text after trimming,
   collapsing whitespace and ignoring case; a query role must be equal;
   `within` keeps only descendants of that container. No match refuses
   `missing-target`. Without a proposed id, more than one visible match
   escalates `ambiguous` (hidden matches are not counted; if all are hidden
   the first is the target and the hidden check refuses it). A proposed id
   must be one of the matches, else `front-layer` when it is buried and
   `proposed-not-top` otherwise. The target then goes through the existing
   checks in order (disabled, hidden, occluded, front layer, offscreen, then
   `gate::check`). Allow carries confidence 1.0, no margin, and an
   `of_target` ticket; candidates are up to five matches at 1.0. The world
   change check still comes first. `near` and `position` are a
   `GuardError` that points at `proposed` and `within`; MCP refuses `near`,
   `position`, `matcher` and `dims` by name. `GuardReason::LowConfidence`
   stays in the wire enum, unused, because the protocol crate is shared with
   the borrow change.
2. **Region state lives in geometry.** `RegionState`, `Availability` and
   `Visibility` move from resonance to `ultra-instinct-geometry` (not core:
   they use geometry's viewport test, and geometry depends on core).
3. **Resonance and HGRA go.** `ultra-instinct-resonance`
   (`WeightedMatcher`, `HgraMatcher`, `ContextScope`), `experiments/hgra`
   (`ultra-instinct-hyper`), the `hgra` features, and the `evals/locate`
   corpus are deleted. The product graph is 13 crates.
4. **No deprecated helpers.** MCP drops `locate`, `inspect`, `act` and
   `diff` and the `repeated_query` signal that only served locate;
   `tool_for_phase` maps Locate and Inspect to `observe` and Diff to
   `verify`. The CLI drops the same subcommands, the matcher options,
   `compare.rs`, and the `sidebar_locate`, `fixture_compare` and
   `live_drive` examples. `observe` already returns every region's id,
   label, parent and state, and `verify` compares two snapshots.
5. **Results state the current product.** The pre-ticket A1–A8 tables move
   verbatim to `bench/results/HISTORICAL-A1-A8.md`; `RESULTS.md` opens with
   the 2026-10-17 Claude Code agents table.

## Consequences

- The A5 / A6 bench arms (and the A7 / A8 combo arms and the B0/B1 stub,
  which also call `locate` / `inspect` / `act`) can no longer be re-run on
  this build. Their scripts and result files stay as history.
- `ultra-instinct-hyper` and `ultra-instinct-resonance` are no longer
  published. Versions already on crates.io stay there.
- A host that asked `guard` for a near-miss label ("Sign" for "Sign in")
  used to get a ranked Allow or a low-confidence refusal; it now gets
  `missing-target` and must use the label `observe` reports.
- Two exact twins (for example a page "Cancel" and a dialog "Cancel")
  escalate `ambiguous` where the old front-layer ranking picked the dialog's;
  the host passes `proposed` or `within`.
- Owners:
  - `ultra-instinct-guard` unit tests (clear button, different label, twins,
    hidden twin, `within`, case and whitespace, disabled, buried page button
    under a dialog, proposed non-match, `near` error) and
    `tests/world_context.rs`.
  - `ultra-instinct-geometry` state tests.
  - `ultra-instinct-mcp` `tests/server.rs`, `tests/world_context.rs`,
    `tests/acme_mail.rs` (t7 / t8 re-homed under guard), `tests/chaos.rs`.
  - `ultra-instinct` `tests/cli_execute.rs`, `tests/mcp_stdio*.rs`.
  - `scripts/architecture.txt` and `scripts/check_architecture.py` for the
    dependency graph.
