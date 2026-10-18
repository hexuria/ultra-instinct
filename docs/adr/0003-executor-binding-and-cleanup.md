# ADR 0003: Executor-bound tickets for every input, hard gate, Phase 8 cleanup

- Status: accepted
- Date: 2026-10-06 (Asia/Manila)
- Branch: `feat/agent-live-exec`
- Relates: ADR 0001, ADR 0002

## Context

After ADR 0002 the agent owned the loop, but only CLICK reached the page:
`BrowserSession::press` refused every other action, the agent mapped
TYPE_TEXT / SELECT onto a click ticket, typed text was dropped, and the
agent re-ran the ranked `guard()` (Weighted matcher + float `0.55` gate) on a
choice Instinct had already made. Remote escalation was an empty feature flag, the
CLI had no way to run the agent, and the legacy JEV `contract.rs` types were
still exported.

## Decision

1. **One executor boundary.** `ultra_instinct_agent::execute_ticketed(browser,
   ledger, ticket, input)` is the only path from a ticket to page input. In
   order: ledger (one-shot) → input kind must equal `ticket.action` →
   **fresh observe** → `revalidate` (world fingerprint + target role / label /
   fingerprint) → hard gate on the fresh region → mark consumed **before**
   dispatch → dispatch to `ticket.target_id`. `BrowserRuntime::dispatch` is raw
   and documented as executor-only.
2. **Hard gate, not ranking, on the agent path.** `ultra_instinct_guard::gate`
   checks action claim, disabled, hidden / zero-area, occluded, front layer
   (behind an open dialog), offscreen, and issues an `ActionTicket` whose
   `action` is the real one (Click / Type / Select). No matcher, no float
   threshold. The agent no longer calls `guard()`. `guard()` stays for the MCP
   / CLI preflight surface, where a host proposes a label.
3. **Front layer applied to the action space.** The agent builds
   `ActionSpace::from_manifold(&with_front_layer(m))`, so a control behind an
   open modal is never offered to Instinct (and the gate refuses it anyway).
4. **Live CDP inputs.** `BrowserSession` gains `type_text`, `select_option`,
   `scroll`, `navigate`, `ready_state`, `field_value`:
   - TYPE / SELECT resolve the **observed node** (node id, then backend id) and
     call a fixed function with the payload as a CDP **argument** (never
     spliced into source). The function throws — changing nothing — on
     disabled, readonly, non-editable, not-a-select, or zero / several
     matching options. A throw is `BrowserError::InputRejected`; an
     unresolvable node is `TargetUnresolved`. There is **no coordinate tier**
     for text: fail closed instead of typing into whatever has focus.
   - SCROLL is one trusted `mouseWheel` at the viewport center, 0.8 × height.
   - A native `<select>` now carries the `select` claim from its tag (it
     observes as `generic`), which also aligns live hit-testing with the
     replay script builder.
5. **Verification per input.** TYPE / SELECT read the field back
   (`field_value`): match → `success`, mismatch → `wrong-effect`; unreadable →
   manifold diff. CLICK / SCROLL / WAIT use the diff (`navigation`,
   `state-changed`, `no-effect`). `wrong-effect` counts with `no-effect`
   toward the consecutive bound → `Blocked`.
6. **Bounded staleness.** A stale ticket returns `AgentError::Stale`, the
   agent goes back to Ready, and the next tick re-observes and re-decides.
   `max_consecutive_stale` (default 5) turns a page that never settles into a
   failure instead of a loop.
7. **Text payloads stay out of Instinct.** SELECT options use the same
   `TextResolver` as TYPE_TEXT (`field_role = "select"`). The resolution
   fingerprint must match its context. Resolution happens before the ticket is
   consumed, so the executor's fresh revalidation always runs **after**
   resolver latency.
8. **Policy evidence fixes** (each with a test):
   - Operation verbs (`type`, `select`, `click`, …) are operation evidence. A
     verb no longer caps its own target's label score at 920; an operation the
     goal did not name is capped at 500 when the goal named another one.
   - Target evidence also scores the goal's *target phrase* (quoted payload,
     leading verb, and following connective removed): `Click Go` → `Go`.
   - Repeated-action avoidance: if the winner is exactly the last executed
     action and it verified (`success` / `state-changed` / `navigation`), the
     single intent is satisfied → `DONE`. Applies to scroll / wait controls too.
   - Fixed a panic in `DeterministicTextResolver` on `type into X`.
9. **Remote escalation (feature `remote`).** `RemotePolicy<T: RemoteTransport>`
   with a Ultra-Instinct-owned JSON wire: request = goal + offered actions +
   history; reply = exactly `{"choice":{"id","kind"}}` or `{"abstain"}`. Off-menu
   ids, kind mismatches, any extra field (selector, coordinates, script), and
   malformed JSON are hard errors. No HTTP stack, no Jev / TypeSafe dependency:
   the consumer supplies the transport. `UnconfiguredRemote` always abstains;
   `ScriptedRemote` replays replies for tests.
10. **CLI.** `ultra-instinct run --goal … (--cdp [url] [--url page] | --fixture
    replay.cdp.json | --fixture page.manifold)`. A CDP replay runs the full
    loop offline; a static manifold is a predict-only dry run.

## Phase 8 cleanup (proven redundant only)

Deleted (no constructor or reader anywhere in the workspace, MCP, CLI, bench,
or examples):

- `ultra-instinct-protocol`: `ComputerTask`, `ComputerResult`, `Intent`,
  `Constraints`, `ExpectedOutcome`, `FallbackReason`, `ReportedExecutor`,
  `ProtocolError::EmptyExpectedText`, and the unused `Request` enum.
  `contract.rs` → `values.rs` keeping `MatcherConfidence`, `ProtocolError`,
  `StateDelta` (used by guard and MCP).

Deprecated (still reachable, kept to avoid breaking callers):

- `ultra_instinct_guard::consume_ticket` — not one-shot; use
  `consume_ticket_once` or `execute_ticketed`.
- `ultra_instinct_mcp::TOOL_ACT` / MCP + CLI `act` — alias of guard that never
  clicks; the historical bench arms A5/A6 still call it over MCP.

Not deleted (still reachable): `ultra-instinct-resonance` Weighted matcher and the
float guard thresholds — they back the MCP / CLI `guard`, `locate`, combo
bench, and world-context tests. The agent path no longer touches them.
`LoopPhase` / `LOOP_ORDER` — MCP `tool_for_phase` maps them.

## Consequences

- Every executed target originates from the decided observation's action
  space, is gated, ticketed, revalidated against a fresh observation, and
  consumed once — for CLICK, TYPE_TEXT, and SELECT alike.
- Each target-bound step costs three observations (decide, executor
  revalidation, after). Correctness over CDP round trips; measure before
  optimizing.
- Instinct policy is still single-intent per goal; multi-step goals need a planner
  or the remote tier. That is a policy gap, not a runtime gap.
