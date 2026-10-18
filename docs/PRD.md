# ultra-instinct

Ultra-Instinct is a **Rust-native browser-agent runtime**: it observes the page,
builds a finite action space, decides (Instinct first, optional escalation),
guards with hard integrity checks, executes only through a one-shot
**ActionTicket**, then re-observes, diffs, and verifies.

It is **not** a voluntary MCP preflight the model must remember to call.
MCP remains an optional adapter. The primary product is a library `Agent`
that owns the loop.

HGRA is one experimental matcher under `experiments/hgra/`. It is **frozen**
and not on the product path. A crate or binary named `hgra` in the product
graph is a bug.

> **Pivot (2026-10-05).** This PRD supersedes the earlier framing that
> Ultra-Instinct is "not an agent" and "does not click". See
> [`docs/adr/0001-agent-runtime-pivot.md`](adr/0001-agent-runtime-pivot.md).
> ActionTicket + host interceptor (B0/B1) remain a valid ablation until the
> agent fully owns execution.

## Primary API (agent runtime)

```rust
use ultra_instinct_agent::AgentBuilder;
use ultra_instinct_policy::InstinctPolicy;

let mut agent = AgentBuilder::new(browser, InstinctPolicy::default())
    .max_steps(60)
    .build("Click Sign in");
let outcome = agent.run();
```

Instinct decides among finite ActionSpace candidates. Guard + ActionTicket bind
execution. MCP is an optional adapter, not required to run the loop.

## Product loop

```text
goal
  ↓
Ultra-Instinct Agent
  ↓
observe → ActionSpace
  ↓
policy (Instinct → optional escalation)
  ↓
guard (hard gates) → ActionTicket
  ↓
executor revalidate + consume + execute
  ↓
observe → diff → verify → history
  ↓
next turn (or DONE / BLOCKED)
```

## Boundaries

| Layer | Owns |
|---|---|
| **Instinct** | HOW a finite choice is made (scores, threshold/margin, abstain) |
| **Ultra-Instinct policy** | WHAT browser evidence each candidate gets; ActionSpace construction |
| **Ultra-Instinct guard** | Physical/logical executability; ticket issue |
| **Ultra-Instinct executor** | Exact ticketed action or nothing |
| **TextResolver** | Arbitrary `TYPE_TEXT` strings (not Instinct); optional goal-grounded model resolver behind `model-text` (ADR 0006) |
| **Host / MCP** | Optional adapter; must not bypass tickets |

## What Ultra-Instinct deliberately does not do

- Depend on JEV / TypeSafe in the core path (optional escalation feature only).
- Ship HGRA as the default matcher / policy.
- Accept model-generated CSS selectors or JavaScript for execution.
- Silently turn Instinct abstention into "top candidate wins".
- Treat page content as trusted instructions.

## ActionTicket (enforcement boundary)

`Allow` issues an `ActionTicket` (ticket id, snapshot id, world / target
fingerprints, target id, action). The executor **must** revalidate against a
fresh observation, then press the **exact** ticket target (or refuse stale /
world-changed). Tickets are one-shot. Verify should bind ticket + before/after
snapshots (`verify_delta`). See `bench/arms/B01.md` for the interceptor ablation.

## Matchers / policy (HGRA frozen)

- Interim locate/guard ranking: `WeightedMatcher`.
- Default policy (`jev` feature): **JEV** via `RemotePolicy`; the offline fallback is **Instinct** (`hexuria/instinct`, pin by rev) via `--policy instinct` (no model, no key).
- **HGRA is frozen** under `experiments/hgra/`. No matcher PRs during the pivot.

## Acceptance bar

- All executed targets originate from the current observed ActionSpace.
- Hidden / disabled / covered / background-modal / stale-ticket targets never execute.
- Instinct abstention is preserved; escalation is explicit.
- Normal tasks: low false-refusal. Adversarial: refuse, never wrong-action.
- No-effect and wrong postcondition: detect on verify.
- System runs without MCP and without Jev when the task fits Instinct + TextResolver.

## Historical notes

`RESULTS.md` A1–A8 tables are **historical** (pre-ticket, planner variables
changed with Ultra-Instinct). Rewrite only from pinned agent A/B/C/D or B0/B1/B2
runs. B0/B1 remain the interceptor ablation until the agent owns the loop.
