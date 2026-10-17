# ultra-instinct

The product is `ultra-instinct`; crates are `aui-*` (Autonomous Ultra
Instinct) and binaries are `ultra-instinct` plus its short alias `aui`.
HGRA is an experimental matcher frozen under `experiments/hgra/`. A crate
or binary named `hgra` on the product path is a bug.

**Product:** Ultra-Instinct is a **Rust-native browser-agent runtime**
([`docs/PRD.md`](docs/PRD.md), [ADR 0001](docs/adr/0001-agent-runtime-pivot.md)).
The library `Agent` **owns the loop**: it observes the page, builds a finite
`ActionSpace`, lets a policy choose (Instinct first, optional explicit escalation),
hard-gates the choice, executes only through a one-shot `ActionTicket`, then
re-observes, diffs, and verifies.

MCP (`ultra-instinct mcp`) is an **optional adapter**, not the orchestration
surface. Nothing on the product path requires MCP, Browser Use, CUA, or Jev.

Public API is 0.1 and unstable until 1.0. Toolchain pin: Rust 1.99.0
(`rust-toolchain.toml`). `publish = false`. `#![forbid(unsafe_code)]` on every
crate.

## Anti-drift (read before changing anything on the agent path)

> **Anti-drift block — Agent + Instinct + ticket path (impeccable audit, baseline `87ffc2d`).**
>
> 1. **The agent owns the loop.** `observe → ActionSpace (front layer applied)
>    → policy → gate → ActionTicket → execute_ticketed → settle → observe →
>    diff / value check → history`. Do not reintroduce "Ultra-Instinct is not an
>    agent / does not click" framing, and do not make MCP or a host the loop.
> 2. **Instinct owns HOW, Ultra-Instinct owns WHAT.** Instinct picks among finite offered
>    actions (scores, threshold / margin, abstain). Ultra-Instinct supplies browser
>    evidence and builds the action space. No browser concepts in Instinct. No
>    second float confidence gate on the agent path. Abstain is never turned
>    into "top candidate wins". Habituation (ADR 0008) only lowers the urge of
>    a repeated `dispatched-no-visible-change` / wrong-effect action; it never
>    adds candidates or bypasses the gate. A click whose only change is a
>    target's control state (checked / expanded / value / selected) verifies
>    as state-changed (ADR 0009), without that state entering the fingerprint.
> 3. **Hard invalidity is guard evidence, not a score.** Disabled, readonly
>    (TYPE / SELECT), hidden / zero-area, occluded, front-layer, offscreen,
>    missing target, unsupported action → `aui_guard::gate` refuses. The
>    ranked `guard()` + `0.55` / `0.05` float gate exists only for the MCP / CLI
>    preflight surface (historical A5 / A6 arms).
> 4. **One executor boundary, one staleness barrier.** `execute_ticketed` is
>    the only path from a ticket to page input. Order: ledger (one-shot) →
>    input kind == `ticket.action` → **fresh observe** → `revalidate`
>    (target-scoped world fingerprint + target role / label / fingerprint) →
>    hard gate on the fresh region → **mark consumed before dispatch** →
>    dispatch to `ticket.target_id`. The agent's CLICK is a trusted pointer
>    press at the region center (`Input::Click`); the DOM `this.click()` tiers
>    are its CDP-refusal fallback and the retry (`Input::ScriptClick`) after a
>    pointer click shows no visible change (ADR 0009). The MCP host's
>    `browser_click` stays DOM-side. The host helper `consume_ticket_once` uses
>    the same consume-before-press order. `Predicted` carries no fingerprint of
>    its own; do not add a "pre-ticket check" that is not wired into a refusal.
> 5. **Stale ≠ spent.** `WorldChanged` / `TargetChanged` / `TargetGone` are
>    stale → discard the prediction, re-observe, decide again (bounded by
>    `max_consecutive_stale`). `TicketConsumed` / `TicketMismatch` / gate
>    refusal / page rejection / dispatch failure are **not** stale and are
>    never silently retried with the same lease.
> 6. **Ticket worlds are target-scoped.** `WorldSnapshot::of_target`: front
>    layer and focus are global; clickable / occluded sets are the target
>    neighborhood (ancestors, same-parent siblings, children, root peers within
>    `NEIGHBOR_RADIUS_PX` = 160, inclusive). MCP `seen_world` stays whole-page
>    (`WorldSnapshot::of`).
> 7. **Text is not Instinct.** `TYPE_TEXT` / `SELECT` payloads come from a
>    `TextResolver`, checked against their context fingerprint, before the
>    ticket is consumed, so executor revalidation always runs after resolver
>    latency. Payloads are CDP arguments, never spliced into script source;
>    there is no coordinate tier for text. SELECT payloads are grounded in
>    observed enabled options when observe captured them: resolver text must
>    equal one option label, else exactly one option label must occur in the
>    goal, else the agent abstains. The deterministic resolver excludes
>    literals successfully typed earlier, then picks an untyped quoted literal
>    whose preceding three goal words share a word with the field label, else
>    the first untyped literal, else abstains (parity with the bench A3q helper).
> 7a. **Model text is payload-only and grounded** (feature `model-text`,
>    ADR 0006). The model fills the payload of an action Instinct already chose;
>    replies must echo the context fingerprint, pass shape checks, and occur
>    in the goal clause, else deterministic fallback, else abstain. One opt-in
>    exception (`--allow-derived-text`, ADR 0009): a value read or worked out
>    from page text may be typed when the goal quotes no untyped value, the
>    field is not secret (password / hidden / file inputs; PIN /
>    CVV / card / one-time-code labels), and the value is one line of ≤ 64
>    chars; every derived value is printed. CI uses scripted models only; no
>    provider SDK or key handling in the tree.
> 7b. **Harder page types** (ADR 0007). Observe pierces open shadow roots and
>    same-origin iframes (`pierce: true`). ComboBox / ListBox / Option are
>    first-class roles. Virtualized lists are the visible window only (scroll
>    + re-observe). Autocomplete is TYPE then ticketed option act after
>    re-observe. After combobox TYPE, read-only option settling is limited to
>    10 polls at 25 ms intervals and reads its `aria-controls` / `aria-owns`
>    popup when present. The document fallback ends early only after visible
>    options change from the first poll and stabilize; input is never
>    re-dispatched. Observe also records control state (value, checked,
>    expanded, select options) as evidence only: it is not in the region
>    fingerprint, and password values are never read. Do not claim cross-origin
>    iframe or closed-shadow coverage.
> 8. **Remote escalation is explicit and closed.** Feature `remote`;
>    reply is exactly a choice id + kind from the offered menu or abstain.
>    Selectors, coordinates, scripts, extra fields → hard error. The request
>    may carry evidence a transport asks for (`wants_evidence`, ADR 0009):
>    the viewport JPEG beside it (`jev`: a System One image, `--no-screenshot`),
>    `page` {url, title, ≤ 6000 chars of visible text} (`jev`, `clef`,
>    `--no-page-text`), and per-target `box` / `near_text`. Evidence never
>    widens the reply.
> 9. **HGRA stays frozen.** No matcher tuning on the agent path.
> 10. **One blocking CDP socket per agent.** Parallelism means separate agents
>    in separate tabs with no shared state. Formal tools stay unjustified for
>    this path (no `unsafe`, no atomics shared across threads, no recovery
>    protocol): Loom / Kani / TLA+ / Miri / Lean are **NOT JUSTIFIED**. The
>    owners are unit tests, `proptest`, adversarial fixtures, replay fixtures,
>    cargo-mutants (nightly), and cargo-fuzz on the untrusted-input parsers
>    (nightly).
>
> Any change to observable semantics on this path names the boundary it
> affects (gate, ticket, executor, world, policy, text) and the test that owns
> it.

## Crates (product graph)

| Crate | Owns |
|---|---|
| `aui-core` | `InteractionManifold`, regions, `ActionSpace`, fixture grammar |
| `aui-cdp` | CDP transport trait, websocket, replay transport, replay script builder |
| `aui-browser` | CDP observe (DOM/AX fusion, identity, stacking), raw CDP inputs |
| `aui-observe` | Observation history and id diff |
| `aui-geometry` | Geometry helpers |
| `aui-policy` | `BrowserPolicy` + `PolicyContext` (`set_situation`, default no-op), `InstinctPolicy` (pinned `hexuria/instinct` rev) + `TRUST_CAP_MILLIS`/`set_evidence_adjustments` (bounded learned-trust evidence), `TextResolver`, `ModelTextResolver` (feature `model-text`), `RemotePolicy` (feature `remote`), multi-step clause split |
| `aui-guard` | Hard `gate`, `ActionTicket` issue / `revalidate`, `TicketLedger`, `consume_ticket_once`, front layer + `WorldSnapshot`; ranked `guard()` for MCP preflight |
| `aui-agent` | `Agent` state machine, `execute_ticketed`, `BrowserRuntime`, verification mapping, `MockBrowser` |
| `aui-protocol` | Guard / ticket / verify wire types |
| `aui-resonance` | `WeightedMatcher` (MCP / CLI locate + ranked guard only) |
| `aui-mcp` | Optional JSON-RPC adapter (observe / guard / verify) |
| `aui-cli` | `ultra-instinct run` (agent loop, `--policy instinct|jev|clef|clef-flash`, `--diary <dir>`); `replay --diary` (offline arena: agree / would-abstain / regress per recorded decision); `observe` / `guard` / `verify` / `locate` / `inspect` / `diff` preflight helpers; `mcp` (stdio); `TypesafeTransport` (feature `jev`: JEV-primary over the closed remote wire); `ClefTransport` (feature `clef`: Cloudflare Workers AI Clef / Clef-Flash over the same wire through `typesafe-sdk`'s Clef provider, one decision call, single-candidate heads answered locally) |
| `aui-dojo` | Battle diary schema v1 (`DiaryLine` JSONL), `DiaryWriter` / `read_diary`, site + situation keys, decision→`ActionSpace` rebuild for the replay arena; versioned `LessonStore` (words / places / moves / trust, keyed per situation, carrying proving diary ids); lessons only adjust evidence inside the finite `ActionSpace` |

Instinct is pinned by git rev in the workspace `Cargo.toml`; bump only with a
deliberate eval. HGRA lives in `experiments/hgra/` and the resonance `hgra`
feature, not the default product path.

## Verification owners

- Agent loop, stale discards, multi-step clauses: `aui-agent` unit tests,
  `tests/mock_loop.rs`, `tests/adversarial.rs`, `tests/replay_cdp.rs`.
- Executor boundary: `executor.rs` unit tests, `tests/props.rs`
  (substitution, staleness, consumed-beats-stale, `is_stale` classification).
- Gate / ticket / world: `aui-guard` unit tests,
  `tests/ticket_props.rs` (radius boundary 159 / 160 / 161, focus-only change,
  consumed never stale), `tests/world_context.rs`.
- Instinct evidence: `aui-policy` unit tests. Do not add a second model of
  Instinct scoring or of `WeightedMatcher::rank`.
- `write_fixture` / `parse_fixture` own the manifold fixture grammar. CDP
  replay is a different grammar. Fusion is the only DOM/accessibility merge.
  Region identity across observations is owned by the browser `IdentityMap`.
- Untrusted input (fixture grammar, CDP replay scripts, MCP stdin lines,
  remote-model replies, model-text replies): `fuzz/` cargo-fuzz targets on a
  dated nightly (`scripts/fuzz-smoke.sh`, `.github/workflows/nightly.yml`),
  backed by the structured/garbage proptests at small case counts.
- Observe protocol cost: `crates/aui-browser/tests/call_budget.rs`
  pins the CDP call count per observe as a regression tripwire.
- Repo rules (toolchain pin, `forbid(unsafe_code)`, crate ceilings, pinned
  nightlies): `scripts/check_repo_rules.sh`. Dependency direction and the
  no-async/no-model-SDK ban below `aui-cli`:
  `scripts/architecture.txt` + `scripts/check_architecture.py`. Both run in
  `ci.yml`'s `repo-rules` job.
- Workflow integrity: `zizmor --persona=auditor` in `ci.yml`. Dependency
  stagnation: `.github/dependabot.yml`.
- Mutation testing: `.github/workflows/mutants.yml` (nightly + manual) on
  `gate.rs`, `ticket.rs`, `executor.rs`, and `world.rs` (`of_target` /
  `neighborhood_of` / `nearby`). How to run locally:
  [`docs/IMPECCABLE-AUDIT.md`](docs/IMPECCABLE-AUDIT.md#mutation-testing).

Live Chrome tests stay `#[ignore]`; CI uses CDP replay fixtures.

```
Verification impact

[x] Pure Rust deterministic behavior
[ ] Concurrency / interleaving
[ ] System model
[ ] Crash-recovery / replay
[ ] Persistence
[ ] TLA+
[ ] Proof kernel
[ ] Workflow / DSL
[ ] Unsafe / memory
[x] Property-test / fuzz surface
[ ] No verification architecture impact

Reason: <which boundary — gate / ticket / executor / world / policy / text>
Affected invariants: <from the anti-drift block>
Tests or proofs updated: <owner tests>
```

## MCP (optional adapter)

`ultra-instinct mcp` is a newline-delimited JSON-RPC server: observe, guard,
verify (locate / inspect / diff remain as deprecated helpers; `act` is a
deprecated alias of guard that never clicks). It owns a 16-entry snapshot
ring and up to four live CDP sessions; a failed live call drops its session,
no retry. Ranked guard keeps the `0.55` / `0.05` float gate for the historical
A5 / A6 bench arms, runs `gate::check` before Allow, and issues `of_target`
tickets like the agent. MCP never bypasses tickets and is never required to
run the agent.

`browser_*` tools are the browser-use-compatible host surface (drop-in MCP
names: `browser_navigate`, `browser_get_state`, `browser_click`,
`browser_type`, `browser_scroll`, `browser_screenshot`, tab tools). Element
indexes come from `browser_get_state` in reading order; `browser_click` /
`browser_type` issue an `of_target` ticket and run `execute_ticketed`, so a
stale page refuses instead of clicking the wrong element. Page-level calls
(navigate, scroll, screenshot, tabs) are plain CDP like browser-use's. Over MCP, `browser_screenshot` answers with an `image` content item (`image/png`) the client can show, then the metadata as text; `Server::call_tool` keeps returning the base64 in its body. `browser_screenshot` and `browser_save_as_pdf` take an optional `path` that writes the decoded file.
Coordinate clicks are deliberately not offered. Diagnostics:
`browser_exec`/`browser_javascript_exec` (Runtime.evaluate),
`browser_extract_content` (deterministic DOM→markdown, no model call),
`browser_read_console`/`browser_read_network` (CDP events buffered by the
transport, domains enabled lazily, drained per read). The `CdpTransport`
trait gains `drain_events` (default empty); the ws transport buffers
unsolicited frames. Advanced input: `browser_right_click` /
`browser_middle_click` / `browser_double_click` / `browser_triple_click` /
`browser_file_upload` go gate → ticket → `consume_ticket_once` before the
press runs; `browser_hover`, `browser_drag` (index→index), `browser_zoom`,
`browser_send_key`/`browser_hold_key`, `browser_save_as_pdf`,
`browser_get_dropdown_options`/`browser_select_dropdown`,
`browser_search`, `browser_form_input` (type alias).

## Signals

Signals (`no-op`, `loop-detected`, `repeated_query`) are data for the MCP host
journal. They do not retry and do not select an executor.
