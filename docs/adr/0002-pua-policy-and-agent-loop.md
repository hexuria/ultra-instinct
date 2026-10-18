# ADR 0002: Instinct policy pin + owned Agent loop

- Status: accepted (guard/executor consequences superseded in part by ADR 0003)
- Date: 2026-10-05 (Asia/Manila)
- Branch: `feat/agent-pua-runtime`
- Relates: ADR 0001

## Context

Phase 0–1 landed ActionSpace and superseded the "not an agent" PRD. Phases 2–7
need a deterministic decision kernel (Instinct), ticketed execution the agent cannot
bypass, TYPE_TEXT isolation, optional escalation, and verification wired to the
loop.

## Decision

1. **Pin** `hexuria/instinct` at git rev `a42d16b6f5ccc3273939c8e3d3f462d78765bfea`
   via workspace deps (`instinct-core`, `instinct-text`, `instinct-lexicon`).
2. **New crate `ultra-instinct-policy`**: `BrowserPolicy`, `InstinctPolicy`,
   `EscalatingPolicy`, `TextResolver` / `DeterministicTextResolver`.
   Hard-invalid targets are excluded by `ActionSpace::from_manifold` before Instinct
   scores. Instinct abstain is first-class; never silently execute top-ranked after
   abstain. No float `0.55` gate in policy.
3. **New crate `ultra-instinct-agent`**: owns observe → ActionSpace → policy → guard
   → ActionTicket → revalidate/consume → press → observe → diff/verify →
   history. `MockBrowser` enables offline e2e. Stale world since prediction
   discards the prediction (Ready), not a permanent task failure.
4. **One-shot tickets**: `TicketLedger` + `TicketInvalid::TicketConsumed`.
5. **HGRA** remains frozen under `experiments/`. MCP is not required for Agent.
6. **TYPE_TEXT**: deterministic resolver only by default; model resolver stays
   feature-gated and unimplemented until needed (`model-text`).
   *Update:* implemented behind `model-text` in
   [ADR 0006](0006-model-text-resolver.md); deterministic stays the default.

## Consequences

- Resonance / float guard thresholds remain for the MCP `guard` tool path until
  Phase 8 proves them unreachable; Agent path uses Instinct for choice and guard for
  integrity.
- Live CDP typing/scroll execution is still thin (Click is the press path);
  documented as a gap in `docs/EVAL.md`.
