# ADR 0001: Ultra-Instinct is the Rust browser-agent runtime; Instinct owns HOW

- Status: accepted
- Date: 2026-10-05 (Asia/Manila)
- Branch: `feat/agent-runtime-pivot`
- Supersedes: the "Ultra-Instinct is not an agent / does not click" product framing in
  `docs/PRD.md` (pre-pivot) and the ActionTicket-only host-executor assumption in
  `docs/DECISIONS.md` § ActionTicket (hosts may still intercept; the primary product
  path is now an owned agent loop).

## Context

Uniform bench and live remasures showed Ultra-Instinct as a voluntary MCP preflight is
not a firewall: `Allow` was not bound to the click (TOCTOU), and combo arms could
"pass" via Browser Use fallback without pressing Ultra-Instinct. ActionTicket (#13)
started the lease boundary. Separately, `browser-use/jev-ultrafast` (MIT) is a
clean reference for a finite action-space agent loop. Instinct (`hexuria/instinct`, ADR
0010) is a domain-agnostic decision engine: consumers own WHAT; Instinct owns HOW.

Uriah approved a deliberate pivot: Ultra-Instinct becomes the Rust-native browser
agent/runtime covering the useful jev-ultrafast architecture, with stronger
observation, ticketed execution, and verification. Instinct is the default
deterministic decision kernel. HGRA stays frozen under `experiments/`.

## Decision

1. **Ultra-Instinct owns the loop.** observe → ActionSpace → policy → guard →
   ActionTicket → executor revalidate/consume → execute → observe → diff/verify →
   history. Primary API is a Rust `Agent` (Phase 4). MCP is an optional adapter,
   not the orchestration surface.
2. **Instinct owns HOW; Ultra-Instinct owns WHAT.** Pin `hexuria/instinct` by git rev when
   `InstinctPolicy` lands (Phase 2). Do not put browser concepts into Instinct. Do not
   recreate a second float confidence gate for policy choice. Hard browser
   invalidity (occluded, disabled, hidden, front-layer, stale ticket) is
   Ultra-Instinct guard evidence, not a Instinct score.
3. **ActionTicket stays the enforcement boundary.** Issued on guard success;
   one-shot; revalidated immediately before input; cannot substitute target or
   action. Stale → discard prediction, re-observe, decide again.
4. **TextResolver is separate.** `TYPE_TEXT` target selection ≠ string
   generation. Instinct must not invent arbitrary field text.
5. **HGRA remains frozen** in `experiments/hgra/`. Weighted (and later Instinct)
   are the product decision path. No matcher tuning during this pivot.
6. **RESULTS.md stays historical** until A/B/C/D (or B0/B1/B2 interceptor
   ablation) runs on pinned main with the agent-owned loop. B0/B1 remain
   relevant as the interceptor ablation until the agent fully owns execution.
7. **Crate target (incremental, not theater):**
   ```
   ultra-instinct-core      (manifold + ActionSpace)     — reuse
   ultra-instinct-browser  (observe + ticketed execute) — expand
   ultra-instinct-policy   (Instinct + escalation)           — new (Phase 2)
   ultra-instinct-guard    (hard integrity + tickets)   — slim toward gates
   ultra-instinct-agent    (loop)                       — new (Phase 4)
   ultra-instinct-mcp      (adapter)                    — optional
   ultra-instinct      (run/observe/step)           — expand
   experiments/hgra   — frozen
   ```
   Existing geometry / observe / resonance / protocol crates stay until their
   responsibilities are proven redundant (Phase 8 cleanup). Do not delete in
   Phase 0–1.

## Mapping (reuse / new / delete later)

| jev-ultrafast | Ultra-Instinct now | Direction |
|---|---|---|
| `agent.py` | *(missing)* | **new** `ultra-instinct-agent` |
| `browser.py` + `snapshot.js` | `ultra-instinct-browser` DOM/AX fusion | **reuse/expand**; do not port JS wholesale |
| `model.py` action_space | `ActionSpace` in core (Phase 1) | **new types from manifold** |
| `model.py` choose | Instinct policy (Phase 2) + optional escalation | **new**; pin Instinct `a42d16b…` |
| `model.py` field_text | `TextResolver` (Phase 5) | **new**; not Instinct |
| `questions.py` | consumer policy data in Ultra-Instinct | **new** evals/data |
| `fresh()` / act guards | ActionTicket + `revalidate` | **reuse/expand** |
| Browser.act | ticketed CDP executor | **expand**; no unguarded agent click |
| history | agent journal | **new** with observe |

**Already on main (keep):** InteractionManifold, DOM/AX fusion, identity,
stacking, world context, ActionTicket issue/revalidate, verify_delta,
WeightedMatcher (interim until Instinct), MCP guard tools (adapter).

**Delete only after proven redundant (Phase 8):** legacy `contract.rs`,
deprecated `act` alias, duplicated float confidence gates once Instinct decides,
generic matcher path if unreachable.

## Consequences

- PRD supersedes "does not click / is not an agent".
- Phase 1 lands ActionSpace only — no Instinct pin in Cargo.toml yet.
- Future Instinct pin: `https://github.com/hexuria/instinct` @ `a42d16b6f5ccc3273939c8e3d3f462d78765bfea`
  (record at pin time; bump when integrating).
- B0/B1 harness remains valid ablation evidence for the interceptor path.
