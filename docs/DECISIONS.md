# Decisions

## Text model: OpenAI, Anthropic, and Gemini wire shapes (2026-10-17)

Status: proposed on `feat/text-model-providers`. Asia/Manila.

**Decision.** The env-driven HTTP text model (`http_text.rs`, was
`openai_text.rs`) speaks three wire shapes: OpenAI-compatible chat
completions (default, jev-ultrafast parity), Anthropic Messages, and Gemini
`generateContent`. `TEXT_MODEL_API` picks one; unset, the exact
`TEXT_MODEL_BASE_URL` host does. Keys ride headers only (Bearer, `x-api-key`,
`x-goog-api-key`), cleartext stays localhost-only, and each provider reply is
reduced to the `parse_command_reply` contract, so fingerprint echo, shape,
and goal grounding are unchanged. Provider refusals and truncation map to
Declined / Malformed (fallback, else abstain).

**Discarded.** Provider SDKs; keys in URLs; per-provider reasoning knobs
beyond Anthropic `effort: low` (Gemini's differ per model generation).

## Model-text TextResolver behind `model-text` (2026-10-06)

Status: accepted on `feat/model-text-resolver`. Asia/Manila.
ADR: [`docs/adr/0006-model-text-resolver.md`](adr/0006-model-text-resolver.md).

**Decision.** `ModelTextResolver` asks an injectable `TextModel` for the
TYPE_TEXT / SELECT payload only, after PUA chose the action. Replies must
echo the context fingerprint, pass shape checks, and be grounded in the goal
clause; otherwise deterministic fallback, otherwise abstain (nothing typed).
Tickets / gate / executor unchanged. Live LLMs plug in through
`CommandTextModel` (`run --text-model-cmd`); CI uses scripted models.

**Discarded.** Model-chosen targets; built-in HTTP provider client and key
handling; ungrounded generation by default.

## PUA pin + Agent loop (Phases 2–7) (2026-10-05)

Status: accepted on `feat/agent-pua-runtime`. Asia/Manila.
ADR: [`docs/adr/0002-pua-policy-and-agent-loop.md`](adr/0002-pua-policy-and-agent-loop.md).

**Decision.** Pin PUA @ `fe3f1fd…`. Ship `hyper-use-policy` (PuaPolicy,
EscalatingPolicy, TextResolver) and `hyper-use-agent` (owned loop, MockBrowser
e2e). TicketLedger enforces one-shot consume. Stale prediction → re-observe.
HGRA stays frozen. Eval honesty in `docs/EVAL.md`.

**Discarded.** Rewriting RESULTS from A1–A8; mechanical jev-ultrafast port;
deleting resonance wholesale before unreachable; required Jev/remote dependency.



## Agent-runtime pivot: Hyper-Use owns the loop; PUA owns HOW (2026-10-05)

Status: accepted on `feat/agent-runtime-pivot`. Asia/Manila.
ADR: [`docs/adr/0001-agent-runtime-pivot.md`](adr/0001-agent-runtime-pivot.md).

**Decision.** Hyper-Use becomes the Rust-native browser-agent runtime covering
the useful architecture of `browser-use/jev-ultrafast` (MIT reference, not a
port). PUA (`hexuria/pua`) is the default deterministic finite decision kernel.
ActionTicket + guard remain the integrity / stale-action boundary. TextResolver
is separate from PUA. HGRA stays frozen. MCP is an adapter, not the core.

**Phase 0–1 in this change.** Audit + ADR + PRD supersession + `ActionSpace` /
`ObservedAction` / `ActionKind` built from `InteractionManifold`. No PUA Cargo
pin yet. No `Agent` loop yet.

**Supersedes.** "Hyper-Use is not an agent / does not click" as the *primary*
product claim. Host interceptor (B0/B1) remains supported and measurable.

**Discarded for this change.** Pinning PUA, `PuaPolicy`, full Agent, deleting
resonance/contract/act, HGRA work, rewriting RESULTS.

## ActionTicket boundary + HGRA freeze (2026-10-05)

Status: accepted on `feat/action-ticket`. Asia/Manila.

**Decision.** The product boundary is an `ActionTicket` issued on
`GuardDecision::Allow`. Hyper-Use remains non-clicking on the agent MCP path.
Hosts (or an invisible Browser Use interceptor) must `revalidate` then press
the exact target. Ticketed `verify` accepts before/after snapshots (+ optional
ticket) and uses `verify_delta` where the expectation is a delta.

**HGRA frozen.** Leave HGRA in `experiments/`. Weighted is the only product
matcher until the B0/B1/B2 Browser Use ablation (`bench/arms/B01.md`) answers
whether Hyper-Use itself reduces wrong actions. Matcher score floors that
paper over uncalibrated thresholds (e.g. geo floor hacks) are not the path.

**RESULTS.md.** Uniform A1–A8 tables are **historical** (pre-ticket, planner
variables changed with Hyper-Use, tip ≠ current main). Rewrite RESULTS only
from pinned B0/B1/B2 runs.

**Hard gates (direction, not landed).** Occluded / front-layer / disabled /
hidden / wrong ancestry / stale ticket should be impossible *before* rank;
`buried_better_label` stays until that split. Do not delete it in the ticket PR.

**Discarded for this change.** Full Browser Use monkeypatch (B1-full), crate
fold, deleting legacy `act` / `contract.rs`, whole-page world equality →
target-scoped fingerprints — next after B0/B1 stub proves the lease.

## Product pivot: browser action firewall (2026-10-05)

Hyper-Use is an independent action-verification layer for browser agents. It
resolves what an agent is about to interact with, refuses ambiguous or unsafe
actions, and verifies the resulting state change.

**Discarded alternatives**

- Hyper-Use as a Browser Use / CUA replacement. Uniform bench (`bench/uniform`,
  PR #2): Luna + Browser Use 91% first-try; Hyper-Use alone 43%; JEV + Hyper-Use
  30%. The winning combo uses Hyper-Use around Browser Use, not instead of it.
- Keeping `hyper-use-browser-use`, `hyper-use-cua`, `hyper-use-macos`, and
  `hyper-use-executor` in the product graph. A8 attaches real Browser Use to
  Chrome; the Rust replay crates were representations of integrations that the
  winning arm did not use.
- Synthetic DOM `this.click()` / coordinate press on the public MCP path. A
  safety layer must not execute less-realistic clicks than Browser Use
  (`isTrusted=false` can reach controls a person could not). Actuation leaves
  Hyper-Use; `GuardDecision` is the product output.
- Shipping HGRA / `hyper-use-hyper` on the default path. It has not been shown
  to beat `WeightedMatcher`. Quarantined under `experiments/hgra/`.
- Implementing typing / select / scroll inside Hyper-Use. Browser Use already
  does those; adding them recreates a worse Browser Use.

**Accepted downsides**

- Hosts must perform the click themselves after `Allow`. Hyper-Use alone cannot
  finish a press-only task end-to-end.
- Bench arms A5–A8 that drove MCP `act` must move to `guard` + host actuation
  (or stay on `bench/uniform` as historical evidence).
- Removing executor crates breaks the old `--executor browser-use|cua` CLI/MCP
  surface. That is intentional.
- Contextual row/card/dialog resolution moves from `bench/arms/combo.py` into
  Rust; until that lands, look-alike rows still need host-supplied context or
  escalation.


## World context gate: hyper-use owns variable-change gating (2026-10-05)

Status: accepted on `feat/world-context-gate`. Asia/Manila.

**Decision.** Hyper-Use is the de facto cursor for a browser agent: it says
whether a proposed click still makes sense in the page as it is *now*. The
guard owns three world variables. It does not own the goal, the next step, or
retries; those stay with the host (Browser Use, a vision model, JEV).

1. **Ancestry** (`LocateQuery::within(container)`). The target must be a
   strict descendant of `container` through parent links. A hard constraint,
   folded into the semantic term by minimum in both matchers
   (`hyper-use-resonance/src/context.rs`, `ContextScope`).
2. **Focus / cursor** (`LocateQuery::near(anchor)`, MCP `near: "focus"`).
   The anchor is `PageState.focused` (accessibility `focused`) or an explicit
   id. Scope = the innermost ancestor-or-self of the anchor that contains a
   best text/role match; regions outside it score semantic 0. No focus, an
   anchor that vanished, or no containing ancestor: the default ranking. This
   is the cargo-runner rule: the cursor line picks the test; no cursor, the
   default command.
3. **Front layer** (`hyper-use-guard/src/world.rs`). A region with
   `Role::Dialog` (`role="dialog"`/`"alertdialog"`, `<dialog>`, or the AX
   role) is a dialog; `RegionFlags::modal` (`aria-modal="true"` or the AX
   `modal` property) makes it modal. A modal blocks every region not inside
   it. A non-modal dialog blocks regions whose center is under its box.
   "Inside" is the parent chain when both sides came from the DOM, else box
   containment (AX-only nodes have no parent link). The guard ranks on a copy
   where blocked regions carry the versioned occluded penalty, so a dialog's
   own control outranks its buried twin, and it refuses
   `GuardReason::FrontLayer` when:
   - the top region is blocked;
   - the host's `proposed` region is blocked;
   - the raw best match is blocked and a weaker match only took the top
     because of the penalty (no rerouting a click into a dialog the host may
     not have seen; `does_not_reroute_a_buried_best_match_into_the_dialog`).
4. **World change** (`GuardRequest::seen_world`, MCP `seen_snapshot`). When
   any of these differ from the observation the host decided on, the guard
   escalates `GuardReason::WorldChanged`, even for a target that would
   otherwise allow:
   - focused region id;
   - open dialog front layer;
   - the set of clickable region ids;
   - the set of regions already marked `occluded` by observe (hit-test or
     stacking).
   This is the "vision model saw the page, then a modal / cookie banner /
   focus move / new control appeared" case.

Observe now reports `parent` per region, `focused`, `front_layer`, and a
blocked region's visibility as `occluded`; inspect reports `blocked_by`;
locate and guard report `scope`.

**Why.** Browser Use and CUA hosts, and our own fixture press, fire
`element.click()`, which reaches a control a person could not click behind a
modal. The old gate only checked score and margin on labels, so it allowed
those clicks. A matcher can score 5/5 on the HGRA locate corpus and still do
this: that corpus is label ranking on static pages and is **not** the bar for
this product. The bar is the adversarial fixtures below, each of which a
label-only gate gets wrong (the tests assert the old failure first).

**Discarded alternatives.**

- More HGRA math (bundle weights, region junk, temporal weight). The algebra
  was not the gap; the inputs were. The paused WIP is in `git stash` on the
  firewall branch, not on this branch.
- Encoding context *only* in the HGRA hypervector. Context still lives in the
  shared semantic minimum so both matchers obey it. HGRA additionally binds the
  resolved `within` / `near` container into the query hypervector as the same
  permuted `parent` channel the region signature already stores, so cosine
  prefers in-scope twins before the minimum zeros the rest.
- Treating `near` as a soft distance score. A soft score lets an out-of-scope
  twin win on other terms; the scope rule is deterministic and falls back
  explicitly.
- Escalate instead of refuse for a buried target. Refuse names the reason
  (`front-layer`) so the host dismisses the dialog first; world change is the
  escalate case because the host must re-observe.

**Known gaps (honest) — what CDP still cannot tell us.**

- Stacking map (`CSS.getComputedStyleForNode` + paint-key among kept regions)
  approximates CSS Appendix E from each node's **own** style. It does **not**
  rebuild the full ancestor stacking-context chain through non-kept nodes, so
  two regions in different nested contexts can compare wrong. Document order
  among kept DOM elements is the tiebreak, not the full paint tree.
- Hit-test (`DOM.getNodeForLocation` at each clickable center) covers unlabeled
  overlays and non-kept nodes. Together with the stacking map, cookie banners,
  custom backdrops, and high-z toasts refuse as `occluded`. Dialog
  `front-layer` remains the path for `Role::Dialog`.
- Still invisible to both paths: `clip-path` / mask, canvas / WebGL, cross-origin
  cross-origin iframe contents, closed shadow trees (open shadow + same-origin iframe content are walked with `pierce: true`),
  and mid-animation frames. `pointer-events: none` coverers do not bury victims
  in the stacking map; hit-test already skips them the same way Chrome does.
- A native `<dialog>` opened with `showModal()` without `aria-modal` is modal
  only if Chrome's AX tree reports `modal`.
- Two sibling modals block each other's content: refuse, not guess.
- An AX-only dialog cannot tell a DOM control under its box from one inside
  it; box containment treats it as content (`acme_mail` compose sheet).
- World change compares focus, front layer, clickable ids, and the occluded
  set. It does **not** hash every label/rect; a same-id re-render that only
  moves geometry under unchanged focus/dialogs/occlusion is not escalated.
- HGRA is still experimental; no live run with `matcher: "hgra"` yet. Fixture
  remasure: `cargo test -p hyper-use-resonance --features hgra --test hgra_remeasure`
  and the twin-row vector tests (`hgra_query_vector_encodes_within_as_parent_channel`,
  `hgra_near_encodes_resolved_scope_into_the_query_vector`).
- Live Chrome smoke for this gate: `examples/world-context/smoke.sh` (modal
  confirm, twin Suspend + focus, cookie backdrop hit-test). Hit-test coords
  must be integer CSS pixels (`DOM.getNodeForLocation` rejects floats).
  Stacking-map occlusion is covered by CDP replay tests; live Chrome smoke for
  pure z-index (no hit-test miss) is still optional.

**Verifiers.** `crates/hyper-use-guard/tests/world_context.rs` (twin Suspend
rows; modal confirm; world-changed on dialog / focus / occluded / clickable
set; stacking-shaped occluded refuse), `crates/hyper-use-browser` stacking
unit tests (`higher_z_index_overlay_buries_the_button_under_its_center`,
`pointer_events_none_does_not_bury`), `crates/hyper-use-mcp/tests/world_context.rs`
(CDP replay: modal, focus, hit-test cookie, stacking map without hit override,
`seen_snapshot` on occlusion growth), `hyper-use-guard/src/world.rs` unit
tests, `context.rs` unit tests,
`guard_with_compose_open_blocks_nothing_outside_the_compose_box`. Fixtures:
`fixtures/twin-suspend-rows.manifold`, `fixtures/modal-confirm.manifold`,
`fixtures/modal-confirm-closed.manifold`.

## Deferred: contextual target resolution (follow-up)

Superseded in part by "World context gate" above: ancestry (`within`), focus
(`near`), and dialogs now resolve in Rust. `contains`-style sibling context
from `combo.py` is still not ported.

Port the row/card/dialog ancestor context logic from `bench/arms/combo.py`
into the Rust resolver (`TargetIntent { label, role, context: Context { contains, container } }`)
so look-alike buttons (Suspend × N rows) disambiguate without JEV. Intentionally
not in this deletion/refocus PR so the cut stays reviewable. Track as the next
commit on `refactor/action-firewall` or a follow-up PR.

**Act / press status**

`BrowserSession::press` and MCP/CLI `act` that click are going away from the
product path. Prefer `guard` → host acts → `verify`. Any remaining press in
browser fixtures is internal / transitional, not the public contract.

---

## Product name

The technique may be discussed as HGRA. The shipped product, crates, binary,
and skill folder are `hyper-use`. A crate named `hgra` is a bug.

## Hypervector algebra

Symbols are bipolar vectors in `{-1,+1}^d`, `d` one of 512, 1024, 2048, 4096.
The default is 2048. Bits are SplitMix64 expanded from FNV-1a of
`namespace || 0x1f || symbol || 0x1f || version || "hyper-use-hv1"`.
`std` `DefaultHasher` is not used because its keys are per-process.

Binding is element-wise multiply (commutative on bipolar values).
Bundling is a weighted sum then sign. A component that sums to exactly 0
becomes `+1`.
Permutation is a left rotation. Relation shifts are in `1..d`.
Similarity is cosine, which for these vectors is `dot / dims` in `[-1, 1]`.
Self-similarity is 1.

Region signatures bind role, label tokens, position zones, shape, actions,
a permuted parent, neighborhood relations, content state, and source bits.
Penalty flags are not mixed into the vector. They subtract after the weighted
sum so a disabled twin drops by exactly the versioned penalty (detached also
drops contextual consistency).

The hypervector term is one cosine of the bundled query against that
signature, not a second learned model. The query bundles one bound probe per
constraint (role, each label token, position, action) at equal weight. It was
the mean of per-probe cosines until "HGRA semantic parity and bundled query".
Weights are [`ResonanceModel::V1`] basis points.

## Geometry

Normalization is `(x - viewport.x) / viewport.width` and the same for `y` and
extents. `y` grows downward. Zones split at 1/3 and 2/3. Center is the closed
middle third on both axes. Small area is `< 0.01`, medium `< 0.08`, else large.
Near means a gap of at most 0.08 viewport units. Aligned means centers within
0.05 on that axis.

## Identity versus diff

`diff` is id-based (added, removed, changed). `match_regions` pairs identical
ids first, then greedy similarity at or above 0.85. A rename is therefore both
an id-level remove+add and a similarity pair. Same label on opposite sides of
the viewport scores 0.8 and does not merge. A browser session now runs
`match_regions` itself when it observes (see "Session identity map"), so a
re-rendered control keeps its id and `diff` sees it as unchanged.

## Dependencies

No `unsafe`. No LLM. No macOS bindings. No browser framework.

`proptest` 1.11 is a dev-dependency of `hyper-use-hyper`, `hyper-use-resonance`,
and `hyper-use-browser`. It is not linked into the `hyper-use` binary.
Manifold fixtures stay a small line format. CLI JSON is still written by hand.

Phase 2 adds two crates, only on `hyper-use-browser`, not on the ranker:

- `serde_json` 1. Parses CDP result JSON. The public transport trait takes
  and returns JSON strings, so `serde_json::Value` is not part of the public
  signature. Hand-rolling a JSON parser was discarded: CDP documents are real
  JSON, and a private parser would be a second grammar with worse errors.
- `tungstenite` 0.26, default features off, `handshake` on. Blocking
  `ws://` client. `wss://` is refused so a TLS stack is not pulled in.
  A from-scratch websocket was discarded: masking and the upgrade handshake
  are easy to get wrong, and this crate is not the product. Chromium,
  headless_chrome, and fantoccini were discarded as too large.

The ranker crates still do not depend on either.

Crate versions are 0.1.0 and `publish = false`. The public API is unstable
until 1.0. There is no `From` for `RegionId` or for a float weight table:
both would make an illegal value easy to build.

## Verifiers skipped

Deterministic ranking, encoding, penalties, and diffs are owned by unit tests
and proptest (32 cases on the algebra, 16 on ranking): encoder stability at
512/1024/2048/4096, commutative self-inverse bind, self-similarity, id
tie-break, and penalty drops. That is the owner for this failure class.
There is no second formal model of the ranker.

- Miri: not run. The workspace forbids `unsafe` (`#![forbid(unsafe_code)]`).
  Miri would not check a different risk.
- Loom: not run. There are no threads, locks, or atomics.
- Kani, Lean, TLA+: not justified. There is no concurrent protocol, no crash
  recovery, and no kernel whose theorem would say more than the tests.
- Fuzz: not run. The fixture parser is small and covered by error-path tests.
  A later untrusted-input phase should add a fuzzer. That absence is accepted
  for Phase 1 because fixtures are local files the operator wrote.
- Benchmarks: one closed-load smoke test ranks 2000 regions on a single
  thread (`hyper-use-resonance` `tests/capacity.rs`). Success is the golden
  top-1 id `target` plus completion. The statistic is one-shot debug latency,
  not a mean or a histogram. Measured about 2.0s on this shared box after
  neighbor coalescing, and about 99.6s before it. The ceiling is 10s (5× the
  measurement) so a busy machine does not flake. It is not a regression gate.
  The PRD p95 under 20ms is not enforced. A 500ms ceiling fails this debug
  build, so it is not the CI check. `iai-callgrind` is deferred: valgrind is
  not assumed, and wall-clock on this box is not a trustworthy regression
  signal.
- cargo-vet / cargo-deny / RUSTSEC: skipped this pass. Do not add the deny
  job yet. USEFUL later because `tungstenite` is in the default graph
  (`hyper-use-browser`) and the optional `jev` feature locks an HTTP stack
  that default `cargo test` does not compile. `cargo-semver-checks` is NOT
  JUSTIFIED until a crate is published.
- Miri, sanitizers, Loom, Kani, TLA+, and Lean stay NOT JUSTIFIED. There is
  no `unsafe` and no concurrent core. See `docs/IMPECCABLE-AUDIT.md`.

Anti-drift: any change to ranking, penalties, geometry thresholds, or the
encoder version updates the Rust tests that pin them. Do not add a second
model of the same score without a conformance fixture.

## What this code cannot do

- No macOS accessibility. macOS returns `ExecutorError::NotImplemented`.
  A browser stub with no session does too. `BrowserExecutor` does not.
  `BrowserUseExecutor` and `CuaExecutor` are each a replay of one semantic act,
  not a live agent and not a fusion benchmark. The pixel CUA driver is still
  `CuaStub`. MCP stdio exists. It is not a second browser and it does not navigate.
- No navigation and no JEV runtime. `ComputerTask` is one locate or one act.
- The weighted matcher and HGRA are not calibrated to each other. A confidence
  of 0.55 is the act gate for a scored total. It is not a probability.
- Live CDP does not launch Chrome. `wss://` is refused. Occlusion is not
  detected. Unlabeled generic DOM containers are not regions unless they are
  a known control tag or carry a label or an explicit role.
- The heuristic fusion pass can merge two same-label controls whose centers
  are within 8px even when IoU is low. It only sees nodes with no backend-id
  partner. A second accessibility node for one DOM node is left as its own
  region.
- CDP snapshots store `captured_at_ms = 0`. The ranker does not read a clock.
- No learned embeddings and no LLM. Symbols are the fixed encoder.
- A bundle component that sums to exactly 0 becomes `+1`. There is no other
  tie-break inside the vector. Equal locate scores break by `RegionId`
  ascending, not by insertion order.
- The fixture line format cannot store a viewport origin other than `(0, 0)`.
  `write_fixture` returns `FixtureError::UnsupportedViewportOrigin` instead of
  dropping it. There is one parser. The writer is the roundtrip owner, not a
  second grammar.
- Signature bundle weights (`role` 2, `label` 3, and the rest) are relative.
  They are not a probability and must not be normalized to 1. Doing so would
  change vectors. `ResonanceModel` positive weights are a different table and
  must sum to exactly 100 basis points (epsilon 0; no float gate).

## Neighbor contributions

Identical `(relation, role)` neighbor vectors are counted and bundled once
with weight `0.25 * count`. `0.25` is dyadic, the running sum stays an exact
multiple of `0.25` at the sizes we rank, and order does not change the sign.
A unit test checks that two `0.25` parts match one `0.5`, in either order.
If `NEIGHBOR_WEIGHT` stops being a multiple of `1/4`, delete the collapse.
The region store stays an array of structs (`BTreeMap` of whole records).
Each rank reads the whole record and N is aimed at 2000. No arena and no
struct-of-arrays until a profile says the hot path is memory-bound.

## Error litmus

On 2026-10-05 the empty-symbol return in `Encoder::encode` was temporarily
skipped (`let _skipped = HyperError::EmptySymbol` instead of `return Err`).
`cargo test -p hyper-use-hyper --lib -- --exact tests::empty_symbol_and_empty_bundle_are_errors`
failed:

```
assertion `left == right` failed
  left: Ok(BipolarVector { dims: 512, head: [1, -1, 1, -1, -1, -1, -1, 1] })
 right: Err(EmptySymbol)
```

The return was restored. The test asserts `Err(HyperError::EmptySymbol)`, not
`is_err()`.

## Phase 2 browser and matchers

The product default locate path is `WeightedMatcher` (semantic 50, geometric
30, actionability 20, basis points, sum 100). Text and role combine by
minimum so a role hit cannot hide a text miss. Penalties are
`ResonanceModel::V1`'s penalty table. The hypervector weight is not applied.
`HgraMatcher` calls `locate_with` and nothing else. `locate` / `locate_with`
remain the HGRA entry points so existing penalty tests keep their meaning.
The CLI default is weighted. `--matcher hgra` selects the other. No benchmark
says which is better. Do not add a third score that averages them.

Act confidence gate: scored totals whose raw `f64` is below 0.55 return
`ExecutorError::ConfidenceBelowThreshold` and do not touch the transport.
`ActConfidence::Inspected` is the operator naming a region id. Those two
states are an enum, not a bool plus an optional score. The refusal is also
representable as `ComputerResult::refused` with `executed = false`. That
value is a journal record, not a second executor. CUA is not invoked.

CDP methods, in order, for one observation: `Page.getLayoutMetrics`,
`DOM.getDocument` depth -1, `Accessibility.getFullAXTree`, then
`DOM.getBoxModel` per DOM node id, then `DOM.getBoxModel` per accessibility
backend id, then one `Page.getNavigationHistory`. A `DOM.getBoxModel` CDP
error omits that node. A protocol error on `Page.getNavigationHistory` omits
the url and title; any other failure of that call aborts observe. Press:
`DOM.resolveNode` then `Runtime.callFunctionOn` of `function(){this.click()}`,
by node id and then by backend node id. A CDP error, or `exceptionDetails` in
the call result, falls through to the next tier and finally to
`Input.dispatchMouseEvent`.
A missing script entry is `CdpError::NoScriptedResponse` and does not fall
through, so a short fixture cannot become a silent coordinate click.
`press` in the CLI is `Action::Click`. Other actions return
`BrowserError::UnsupportedAction`.

Fusion v1: label Jaccard >= 0.5 or either label empty; roles equal or either
generic; IoU >= 0.5 or centroid distance <= 8px. Greedy, highest IoU. The
stored rect is the DOM rect. The id is `n{backendNodeId}`. Fusion v2 (below)
adds a backend-id join ahead of this heuristic.

`ComputerTask` has constructors `locate` and `act` only. Constraints and
expected outcome are data. There is no planner.

Verifiers for this phase: unit tests with exact `Err` variants
(`ExpectedTextMissing`, `RegionStillPresent`, `ConfidenceBelowThreshold`,
`WeightsDoNotSum`, `NoScriptedResponse`). proptest (16 cases) owns "garbage
CDP scripts do not panic". No Miri, Loom, Kani, TLA+, or Lean. The websocket
is one blocking reader. That is not a concurrent protocol. No second model
of fusion or of either ranker.

Live Chrome: a read-only `Browser.getVersion` may be run against an
already-open debugging port. Clicks in tests use the replay transport.

## MCP stdio

The server is `hyper-use mcp`. Tool names are exactly `observe`, `locate`,
`inspect`, `act`, `diff`, and `verify`. The product name is the server name,
not a prefix on each tool. A prefixed name would be a second vocabulary.
`navigate` is not a tool. Arguments named `goal`, `steps`, or `navigate` return
`GoalNotAccepted`. Arguments named `x`, `y`, or `coordinates` return
`CoordinatesNotAccepted`. Those states are not representable as a successful
click.

Transport is newline-delimited JSON-RPC 2.0 on stdin and stdout. A notification
(no `id`) gets no response. Batches are `InvalidRequest`. There is no
`Content-Length` framing, no resources, no prompts, and no sampling. The
official `rmcp` stack was discarded: it pulls an async runtime and a protocol
surface this host does not call. A blocking read loop is the whole transport.
Downside: clients that only speak the older header framing cannot connect.
Accepted, because the MCP stdio spec delimits messages with newlines.

`serde_json` 1 is a direct dependency of `hyper-use-browser`,
`hyper-use-browser-use`, `hyper-use-cua`, and `hyper-use-mcp`. `hyper-use-cli`
depends on it only through the optional `jev` feature. It parses CDP results,
replay scripts, and JSON-RPC. `serde_json::Value` is not a type in the ranker
crates. `hyper-use-core`, `hyper-use-hyper`, `hyper-use-geometry`, and
`hyper-use-resonance` do not depend on this crate.

`locate` defaults to `WeightedMatcher`. `matcher: "hgra"` selects
`HgraMatcher`. Every locate result sets `benchmark` to false. That flag is not
a measurement. Do not read it as a win.

`locate` and `inspect` omit `executed` and `verified`. `ComputerResult` always
sets both, so a locate that carries `executed: false` looks like a refusal.
Those keys stay on `act`, and on `verify` (`verified` is the check,
`executed: false` because verify does not press). `observe` and `diff` still
include both keys. A locate is not encoded as `ComputerResult`.

`act` calls `BrowserExecutor`. DOM semantic click stays ahead of coordinates.
A scored raw confidence below 0.55 returns a tool result with
`executed: false`, `fallback: "low-confidence"`, and no mechanism. It does not
return a JSON-RPC error and it does not press. The proof is the
`sign-in.cdp.json` fixture, which has no press responses: a click would be
`Browser`, and the test expects `executed: false`. An inspected act (no
confidence) still presses. `act` against a manifold fixture is
`ActNeedsCdp`, because that file has no DOM node. With an expectation, or on a
live `cdp` session, the result includes a fresh state delta (see
"MCP session and snapshot ring"). Live `cdp` is accepted and is not required
by tests.

Tool failures are `isError: true` with a JSON object whose `variant` matches
`ToolError`. Protocol failures (`ParseError`, `InvalidRequest`,
`MethodNotFound`, `MissingToolName`) are JSON-RPC errors with the same
`variant` field. Tests compare the enum with `assert_eq!`, not `is_err()`.

Error litmus, 2026-10-05: `resolve_origin`'s missing-source arm was temporarily
`Err(ToolError::MissingExpect)`. `cargo test -p hyper-use-mcp --test server --
error_variants_are_exact -- --exact` failed:

```
assertion `left == right` failed
  left: MissingExpect
 right: MissingFixture
```

The arm was restored to `ToolError::MissingFixture`.

Verifiers: unit and in-process JSON-RPC tests own the dispatch and the exact
variants. A 16-case proptest owns "random stdin lines do not panic". That is
not a fuzz campaign and not a proof. The stdio subprocess test owns "the
binary is not a name-only stub". No Miri, Loom, Kani, TLA+, or Lean. One
blocking reader is not a concurrent protocol. No second model of the ranker
or of fusion was added.

`Ranker` is the escape if `locate_with` returns `Err`. The default weighted
matcher and `HgraMatcher` with `ResonanceModel::V1` do not return it on a
manifold this crate just parsed. There is no fixture for it. A click is not
substituted.

## Fixture comparison, not a Browser Use score

The first comparison is the `fixture_compare` example in `hyper-use-cli`. It
parses `sidebar.manifold`, `sign-in.cdp.json`, `welcome.cdp.json`, and
`sign-in-press.cdp.json`, ranks with `WeightedMatcher`, and acts only on the
press fixture through `BrowserExecutor` and `ReplayTransport`.
`press-only.cdp.json` is omitted because it has no observation. Sign-in is not
diffed against welcome. `HgraMatcher` is not ranked here.

`executed` is true only after an action receipt. A scored raw confidence below
0.55 does not call the transport, and `executed` is false. A manifold file
cannot act, so the sidebar case has no `executed` field. The JSON is not a
`ComputerResult`: that type always carries `executed` and `verified`, which
would make a locate look like a fake refusal.

`typesafe-sdk` 0.2, feature `blocking`, is an optional `jev` feature of the
CLI crate only. It is not a path dependency, not a ranker dependency, and not
a second matcher. `cargo build` and `cargo test` do not compile it, so the
default `hyper-use` binary does not link Tokio. Enabling `jev` does: the SDK's
blocking client owns a current-thread runtime. That build is not the default.
Live calls run only when the feature and `HYPER_USE_JEV=1` are both set. Each
case is one `Client::from_env()` `system_one` choice. The model and base URL
come from the environment. State is the manifold (viewport, id, role, label,
rect), not a screenshot. The prompt does not contain the 0.55 rule or the
correct action. The client reads `choice` and does not read usage. `agree` is
id equality, or `press` / `do-not-press` against `executed`. It is not a win
and not a benchmark.

Downside accepted: four fixtures and one press. A live choice can disagree and
the run still succeeds, because this slice measures agreement, not a product
winner. That comparison does not call the Browser Use executor and is not a
Browser Use score. No tokens, screenshots, retries, latency, or winner are
recorded.

Verifier: the fixture test owns region ids, `executed` only on the press case,
absence of `jev` and `agree`, and confidence equal to a second `rank` of the
same manifold. The low-confidence arm owns "the transport log stays empty".
No Miri, Loom, Kani, or benchmark harness. The optional feature is typechecked
with `cargo check -p hyper-use-cli --features jev` and is not part of default CI.


## Browser Use semantic executor

Browser Use is an opt-in act backend, not a fallback and not an agent.
`ExecutorKind::BrowserUse` is absent from `DEFAULT_POLICY_ORDER`.
`select_executor` therefore keeps the CDP browser press when both are listed,
and `select_act_executor(None, _)` ignores Browser Use. The host names
`browser-use` with `select_requested`. A missing CDP session does not delegate.

The request is a `SemanticRequest`: region id, role, label, and action.
`to_wire` writes only those four keys. A fixture that carries `goal`, `url`,
`x`, `y`, `coordinates`, `navigate`, `task`, `screenshot`, `tokens`, `latency`,
or `retries` is `BrowserUseError::BadScript`. There is no coordinate click on
this path. The CDP press order (DOM semantic by node id, then by backend
node id, then a coordinate click) is unchanged.

`ReplayTransport` is the only transport. It records every `submit` and then
either returns a `TransportReceipt` or `BrowserUseError::Rejected`. It does
not start a process. A receipt whose id or action differs from the request is
`ParamsMismatch` after the call. Stubs still return `NotImplemented` and do
not panic. macOS stays unimplemented (`ExecutorError::NotImplemented`; no AX). `CuaStub` pixel actuation stays unimplemented. The opt-in `cua-replay` semantic handoff has landed: region id, role, label, and action through a replay fixture. It is not a live process and it is not a benchmark.

The confidence gate runs after the region id and the action match, and before
`submit`. A scored raw total below 0.55 returns
`ConfidenceBelowThreshold` and leaves the transport log empty. An unknown
region returns `UnknownRegion` and also does not submit.

Downside accepted: the replay script is the resolved target. This slice does
not observe through Browser Use, and it does not speak to a live Browser Use
process. A host that has not already located the region has nothing to hand
over. That is intentional. This is not a success rate, a token count, a
screenshot comparison, a retry policy, a latency, or a win over CDP.

Verifier: unit tests own the wire keys, the exact `Rejected` and `BadScript`
and `ParamsMismatch` variants, and "the log stays empty below 0.55".
A 16-case proptest owns the key set for generated labels, and a 16-case
proptest owns the gate for integer millis in `0..550`. CLI and MCP tests own
selection: high confidence records `browser-use-semantic`, low confidence on
a rejecting script is still `executed: false`, and `macos` is
`NotImplemented`. No Miri, Loom, Kani, or benchmark harness. No second model
of the ranker. No live Browser Use process.

## CUA semantic handoff, not a fusion benchmark

CUA is an opt-in act backend, not a fallback and not an agent.
`ExecutorKind::Cua` is absent from `DEFAULT_POLICY_ORDER`, next to
`ExecutorKind::BrowserUse`. `select_executor` therefore keeps the CDP browser
press when CUA is also listed, and returns `NoneAvailable` when CUA is the
only listed kind. `select_act_executor(None, _)` drops both opt-in kinds
before that walk. The host names `cua` with `select_requested`. A missing CDP
session does not delegate.

The request is a `hyper_use_cua::SemanticRequest`: region id, role, label, and
action. `to_wire` writes only those four keys. A fixture that carries `goal`,
`url`, `x`, `y`, `coordinates`, `navigate`, `task`, `screenshot`, `tokens`,
`latency`, or `retries` is `CuaError::BadScript`. The script kind is
`cua-replay`. A `browser-use-replay` document is rejected, so the two grammars
are not interchangeable. There is no coordinate click on this path. The CDP
press order is unchanged, and the Browser Use path is unchanged.

`ReplayTransport` in `hyper-use-cua` is the only CUA transport. It records
every `submit` and then either returns a `TransportReceipt` or
`CuaError::Rejected`. It does not start a process. A receipt whose id or
action differs from the request is `ParamsMismatch` after the call. The
region id and the action are checked before the confidence gate, so an unknown
region or a different action does not submit. A scored raw total below 0.55
returns `ConfidenceBelowThreshold` and leaves the transport log empty.
`StubExecutor` for `cua` still returns `NotImplemented` and does not panic.
macOS stays `NotImplemented`. `CuaStub::status` stays the pixel-driver
sentence. That stub is not a fusion score.

Downside accepted: the replay script is the resolved target. This slice does
not observe pixels, does not fuse a screenshot with the manifold, and does not
speak to a live computer-use process. A host that has not already located the
region has nothing to hand over. Removing CUA from the old last-resort slot
means a host that only lists `cua` as available no longer receives that kind
from `select_executor`; it must name it. That is intentional. This is not a
success rate, a token count, a screenshot comparison, a retry policy, a
latency, or a win over CDP or Browser Use.

`serde_json` parses the replay document inside `hyper-use-cua`, as it does for
Browser Use. `to_wire` returns a `String`. `serde_json::Value` is not part of
the public signature. No ranker crate depends on it.

Verifier: unit tests own the wire keys, the exact `Rejected`, `BadScript`, and
`ParamsMismatch` variants, and "the log stays empty below 0.55". A
16-case proptest owns the key set for generated labels, and a 16-case proptest
owns the gate for integer millis in `0..550`. CLI and MCP tests own selection:
high confidence records `cua-semantic`, low confidence on a rejecting script
is still `executed: false`, an unnamed executor does not accept a `cua-replay`
script, and `macos` is `NotImplemented`. No Miri, Loom, Kani, or benchmark
harness. No second model of the ranker. No live CUA process. No fusion metric.

Error litmus, 2026-10-05: the empty-label `return Err` in
`hyper_use_cua::SemanticRequest::new` was temporarily replaced with
`let _skipped = CuaError::BadScript { message: "label must not be empty".into() };`.
`cargo test -p hyper-use-cua --lib -- --exact tests::empty_label_and_unknown_role_are_script_errors`
failed:

```
called `Result::unwrap_err()` on an `Ok` value: SemanticRequest { region_id: RegionId("n100"), role: Button, label: "", action: Click }
```

The return was restored. The test then asserts
`Err(CuaError::BadScript { message: "label must not be empty" })`, not `is_err()`.


## Box model errors omit a node

Chrome answers `DOM.getBoxModel` with "Could not compute box model." for a
`display:none` node. Hidden menus and dialogs are on most real pages, so
propagating that error made `observe` fail on them. A CDP `error` on that call
now means the node has no box and is left out, which is the same outcome as a
box model with no content quad. `NoScriptedResponse`, `ParamsMismatch`,
`BadJson`, and `Transport` stay fatal, so a short replay script still fails.

Downside accepted: a `display:none` control is absent from the manifold, so
`verify --expect-absent` passes for it. That is the intended meaning of gone.

Verifiers: `observe_omits_a_node_whose_box_model_is_a_protocol_error` on
`fixtures/hidden-node.cdp.json`, `observe_still_fails_when_the_box_model_step_is_missing`,
and `box_model_params_mismatch_is_still_fatal`.

## Press has no focus tier

`press` used to return `CdpElement` after `DOM.focus` succeeded. The caller was
told a click happened when only a focus did. A click now tries
`Runtime.callFunctionOn` by node id, then by backend node id (a backend id
survives node id invalidation, which is the usual reason `DOM.resolveNode`
fails), then a coordinate click. `ActMechanism::CdpElement` is removed. A
click function that reports `exceptionDetails` (an SVG element has no
`click`) is a tier failure, not a `DomSemantic` success.

Downside accepted: when both semantic tiers fail, a coordinate click can land
on whatever is at that point now. Returning an error instead was rejected to
keep the documented third tier. This is a judgement call.

Verifiers: `node_id_failure_retries_by_backend_id_before_coordinates`,
`both_semantic_tiers_fail_then_coordinates`, and
`click_exception_is_a_tier_failure`. Each asserts that `DOM.focus` is never
sent.

## Label precision in the weighted text term

The weighted text term was `token_recall(query, label)`: the share of query
tokens found in the label. Extra label tokens cost nothing, so "Send",
"Send feedback", and "Send to device" all scored 1.0 for "Send" and the id
tie-break picked "Send feedback". The term is now
`recall * (0.5 + 0.5 * precision)`, where precision is the share of label
tokens found in the query (`token_precision`). On
`fixtures/send-buttons.manifold` the totals are 1.0, 0.875, and 0.8333, in
that order.

The formula was picked over F1 so that a full-recall superset label keeps at
least half the text credit: "Account Settings" for "Settings" scores 0.75,
not 0.667. `token_recall` is unchanged because `verify` and the HGRA semantic
term use it. The HGRA semantic term is not changed; it already orders this
fixture correctly and there is no benchmark to justify moving its totals.

Downside accepted: a long query against a slightly longer label differs by
little (a 5-token query against a 6-token superset is about 42 millis apart),
so the act margin gate refuses that case rather than guessing.

Verifiers: `exact_label_outranks_superset_labels_with_lower_ids` asserts the
three totals, the 16-case proptest
`extra_label_tokens_strictly_lower_the_weighted_total`, the CLI test
`locate_send_prefers_the_exact_label`, and the regression pin
`hgra_send_order_is_unchanged`. No second model of `WeightedMatcher::rank`.

## Act ambiguity margin

A high top total is not enough when a second candidate is almost as high. Act
now takes an optional runner-up total from the same ranking and refuses with
`ExecutorError::AmbiguousTarget` when the raw gap is below
`MIN_ACT_MARGIN` (0.05). `MIN_ACT_MARGIN_MILLIS` (50) is display only. `ActConfidence` gains a `Ranked { top,
runner_up }` variant beside `Inspected` and `Scored`; it stays an enum, not a
score with an optional runner-up. `gate_confidence` is the single gate, and
all four executors call it. The order is non-finite, then the 0.55 threshold,
then the margin, so a low top is reported as low confidence even when it is
also ambiguous. A runner-up above the top also refuses. The journal fallback is
`FallbackReason::Ambiguous` ("ambiguous").

MCP `act` takes `runner_up: {id, confidence}`, the same shape as locate
`candidates[1]`. A `runner_up` without `confidence` is
`RunnerUpNeedsConfidence`, so it cannot fall to the ungated inspected path.
A runner-up naming the pressed region is `RunnerUpIsTarget`. The CLI flag is
`--runner-up`. The fixture comparison passes the real runner-up, so it applies
the product gate.

Downside accepted: a caller that omits `runner_up` bypasses the margin. The
MCP session in the next phase can derive it. 0.05 is not calibrated across
matchers; the HGRA order on `send-buttons.manifold` is about 0.02 apart
and is refused. This is not `RegionFlags::ambiguous`, which is a ranking
penalty that the browser observer does not set.

Verifiers: `ambiguous_ranked_act_does_not_click_and_refusal_is_typed` (empty
transport log), `low_confidence_wins_over_ambiguity`,
`inspected_act_ignores_the_margin`, the 16-case proptests
`ranked_within_margin_does_not_submit` (Browser Use and CUA) and
`ranked_outside_margin_passes_the_gate`, MCP and CLI act tests, and
`below_margin_does_not_execute_and_does_not_call_transport`.

## MCP session and snapshot ring

The MCP server used to be free functions: every call reconnected, observed
from scratch, and dropped the result. Nothing could compare the state before
an act with the state after it. `serve_stdio` now owns one `Server`:

- a `SnapshotRing` (in `hyper-use-observe::history`) of the last 16
  observations, with ids that only increase and are never reused. Every tool
  that observes returns `snapshot`. `diff` accepts `before_snapshot` and
  `after_snapshot` as well as file paths, but not a mix
  (`MixedDiffSources`). An evicted id is `SnapshotEvicted { id, oldest }`.
- up to `MAX_LIVE_SESSIONS` (4) live CDP sessions keyed by endpoint. A live
  call that fails drops its session. There is no retry and no reconnect loop;
  the next call opens a new socket. Fixture origins open a fresh replay
  transport per call, so calling `observe` twice on one fixture still works.

`act` is now one closed loop: observe (or reuse the live session's latest
observation, so the bindings match what the caller located), gate, press,
then observe again when `observe_after` is set, diff, and verify an optional
`expect_text` or `expect_absent`. `observe_after` defaults to true with an
expectation or on a live session. The result carries `state_delta`,
`verified`, `before_snapshot`, and `after_snapshot`. A failed postcondition is
`executed: true`, `verified: false`, `fallback: "verify-failed"`, and a
`verify_error` object. `act` can also take the locate fields (text, role,
position): it ranks its own observation with the same `RegionMatcher`,
derives the ranked confidence and runner-up, and refuses with `TargetNotTop`
when `region` is not first. That closes the margin bypass for callers that use
it. It is not a second ranker.

The free `call_tool` and `handle_line` build a new `Server` per call, so they
keep their old stateless meaning.

Downside accepted: on a live page the reused before-snapshot can be stale if
the page changed between calls. Persistence is not involved: the ring is in
memory and lost when the process exits.

Verifiers: `ring_evicts_oldest_and_ids_never_repeat`, the 16-case proptest
`snapshot_ids_are_strictly_increasing`,
`act_closed_loop_reports_delta_and_verifies_welcome` on
`fixtures/sign-in-loop.cdp.json`,
`act_closed_loop_verify_failure_is_executed_true_verified_false`,
`act_with_locate_fields_derives_the_ranked_gate`,
`diff_by_snapshot_ids_matches_diff_by_paths`, `evicted_snapshot_is_exact`,
`stateless_call_tool_is_unchanged`, and the stdio subprocess test
`observe_then_act_then_diff_by_snapshot_in_one_process`.

## Session identity map

A region id used to be the backend node id, so a framework that re-rendered a
button (same control, new DOM node) produced removed plus added, the same as
a different button. `BrowserSession` now owns an `IdentityMap` and applies it
on every observe:

1. A fused id seen in the previous observation keeps its stable id.
2. The rest are paired with the previous observation by `match_regions` with
   `structural_similarity` at 0.85. A pair inherits the previous stable id.
3. Anything left is minted as its fused id, or `{fused}-{k}` when that id
   already named another control in this session. Stable ids are never reused.

A first observation has exactly the fused ids, so fixtures, the Browser Use and
CUA scripts, and existing tests are unchanged. After a re-render the id is
opaque: `n100` can name a node whose backend id is 900. The binding used to
press always holds the current node id and backend id. Parent references are
rewritten through the same map. `hyper-use-observe` is now a normal dependency
of the browser crate; observe depends only on core, so there is no cycle.

Downside accepted: a wrong similarity pair would give a different control the
old id. The 0.85 threshold already refuses the far-duplicate case, and the act
margin gate still applies.

Verifiers: `rerendered_button_keeps_its_id_with_a_new_backend_node` (the press
resolves node 90, not 10), `distant_same_label_does_not_inherit_identity`,
`single_observation_ids_are_unchanged`,
`a_rerender_inherits_and_a_reused_backend_id_is_minted_fresh`, and the 16-case
proptest `reobserving_an_identical_manifold_keeps_every_id`.

## Fusion v2: backend join first, and DOM parents

Chrome gives a DOM node a `backendNodeId` and its accessibility node a
`backendDOMNodeId`. Fusion v1 ignored that key and paired the two trees by
label overlap and geometry. A link whose accessible name adds screen-reader
text ("Read more" vs "Read more about our pricing plans") split into two
regions, `n{b}` and `ax{b}`, that then competed in the ranker. Fusion v2 joins
on the backend id first; the v1 heuristic runs only on nodes with no backend
partner. The merge itself is unchanged: the accessibility label wins when it is
not empty, a generic DOM role takes the accessibility role, and the stored rect
is the DOM rect. Fusion is still the only DOM/accessibility merge.

The DOM walk now records kept ancestors. A region's `parent` is the nearest
kept ancestor that is itself a region in the manifold (an ancestor with no box
is skipped). Accessibility-only regions have no parent. `contextual_score` and
the HGRA signature read parents; no current fixture has a nested control, so
no existing total moves.

Downside accepted: a page that reuses one backend id for two accessibility
nodes keeps the second as a separate region, as before. Occlusion and CSS
visibility still need `DOMSnapshot.captureSnapshot`, which would also replace
one `DOM.getBoxModel` round trip per node. That is a later change because it
rewrites every CDP fixture.

Verifiers: `same_backend_node_joins_even_when_labels_differ`,
`heuristic_pass_only_sees_unjoined_nodes`,
`parent_is_the_nearest_ancestor_that_is_a_region`,
`observe_records_the_nearest_dom_parent`, and the updated
`one_pixel_shift_merges_and_different_labels_do_not`.


## Page state, richer diff, and no-effect verify

An observation now ends with one `Page.getNavigationHistory` call. The current
history entry supplies `PageState` url and title. The payload has no time, so
`captured_at_ms` stays 0, the same value fusion already stored. `focused` is
the stable id of the accessibility node whose `focused` property is true, after
the identity map renames it. It is not read from the history call.

A CDP protocol error on the history call leaves the url and title unknown and
observe continues. That is the same split as `DOM.getBoxModel`:
`CdpError::Protocol` omits, and `NoScriptedResponse`, `ParamsMismatch`,
`BadJson`, and `Transport` stay fatal. Unknown is `None`, not the empty string
(superseded 2026-10-05, see "Unknown page state is not empty"). It does not
invent a timestamp. A missing script entry is still
`CdpError::NoScriptedResponse`.

`ManifoldDiff::moved` and `relabeled` read the existing id diff. Moved means
the id survived and the rectangle is the only changed field. The fingerprint may also differ, because it hashes the rectangle; that is not a second change. Relabeled means
the id survived with a different label. They are not a second diff.
`StateDelta` gains `moved`, `text_changed`, `focus_changed`, and `url_changed`
through builders, so `StateDelta::new` still takes only the three id lists.
The field is `url_changed`. There is no navigate tool and no navigate intent.

`verify_delta` checks `Appeared`, `Disappeared`, and `UrlChanged`. It returns
`VerifyError::NoEffect` when the region diff is empty and the page state is
unchanged, before the specific expectation. The host string is
`FallbackReason::NoEffect`, `"no-effect"`. An act that asked for `expect_text`
or `expect_absent` still reports `verify-failed` when that postcondition
fails, including when the page did not change.

Downside accepted: `captured_at_ms` is still not a clock. (The earlier
downside, that a protocol error and an empty URL looked the same, is removed:
see "Unknown page state is not empty".)

Verifiers: `moved_only_rect_is_moved_not_relabeled`,
`url_change_is_reported_without_a_region_change`,
`focus_moves_to_the_text_field`, `executed_act_with_no_delta_is_no_effect`,
`navigation_history_protocol_error_omits_url_and_title` (now asserts `None`), and
`missing_navigation_history_step_is_fatal`. No second formal model.

## Temporal signals are data, and the corpus names no winner

`StateSignature` is the sorted multiset of `(role, label)` on one manifold.
Rectangles, ids, and flags are not in it. `detect` compares signatures already
stored in the snapshot ring. `NoOp` means the after signature equals the before
signature. `LoopDetected` means the after signature equals an older snapshot
of the same origin among the four ring entries immediately before it, not
counting `before`. Both can be present. MCP `act` returns them as `signals`.
There is no retry, no navigate tool, and no second ranker.

An empty region diff with an unchanged page is still `VerifyError::NoEffect`
and `fallback` `"no-effect"`. A no-op signal does not replace that path and
does not schedule another press.

`eval_corpus` reads `evals/locate/cases.tsv` and ranks each row with
`WeightedMatcher` and `HgraMatcher`. The report has each top id, the margin in
millis, and whether the existing act gate would refuse. It has no winner.
The product default stays `WeightedMatcher`. `expected_id` is the region that
matcher ranks first. An HGRA disagreement is recorded and is not a corpus
failure. This does not invent a Browser Use score, a token count, or a latency.

Downside accepted: a rectangle-only move is still `NoOp`, because the signature
ignores geometry. `NoEffect` remains the diff-and-page check, so the two can
disagree. A loop older than the four preceding entries is not reported.

Verifiers: `no_op_when_after_equals_before`,
`loop_when_after_equals_an_older_snapshot`, and
`eval_corpus_reports_both_matchers_without_a_winner`. No second formal model.

## Raw gate: compare f64, not rounded millis

Status: 2026-10-05 (Asia/Manila). Replaces the millis comparison in "Phase 2
browser and matchers" and "Act ambiguity margin".

The gate compared `display_millis(confidence)` against 550, so 0.5496 rounded
to 550 and clicked, and a margin of 0.0491 rounded to 49 or 50 depending on the
inputs. `gate_confidence` now compares the raw `f64`: below
`MIN_ACT_CONFIDENCE` (0.55) is `ConfidenceBelowThreshold`, and a margin below
`MIN_ACT_MARGIN` (0.05) is `AmbiguousTarget`. The margin check subtracts
`MARGIN_EPSILON` (1e-9), because `0.6 - 0.55` is `0.04999999999999993` in
`f64`; without it a clean 0.05 gap would refuse. The threshold check has no
epsilon. A non-finite margin (for example `MAX - (-MAX)`) refuses as
ambiguous. The millis constants and `margin_millis` remain for display and
journal text. A refused display is capped (549 and 49) so a refusal never
prints the threshold it failed.

Caller confidence is a probability. MCP `confidence` and
`runner_up.confidence` outside `[0, 1]` are
`ToolError::ConfidenceOutOfRange` (`{"variant","value"}`).
`MatcherConfidence::try_unit` enforces the range; `try_new` still accepts
negatives because ranker totals can be negative.

Downside accepted: a caller that forwards a negative locate total (a penalized
candidate) as `runner_up.confidence` now gets `ConfidenceOutOfRange` instead
of a passing margin. Clamp or omit it. 0.55 and 0.05 are still not
calibrated across matchers.

Verifiers: executor gate unit tests (0.5496 refused, 0.0491 refused, 0.6/0.55
passes, extreme inputs), a 64-case gate proptest, MCP
`caller_confidence_outside_zero_to_one_is_exact` and
`raw_confidence_just_below_the_gate_does_not_press`, and the 32-case
`structured_tool_calls_are_typed` proptest.

## Stale observation after a press

Status: 2026-10-05.

`run_act` reused the session's stored manifold as `before` whenever one
existed. After an act with `observe_after: false`, the next act reused the
pre-press snapshot, so its diff compared against a page that no longer
existed. `BrowserSession` now sets `stale` once a press reaches the CDP click
calls (even if a tier fails), and `observe` clears it. Act reuses only
`fresh_manifold()`, so a stale session observes again first. `manifold()` still
returns the last view for inspect.

Downside accepted: a press whose click calls all failed before reaching the
page still marks the session stale. That costs one extra observe; it never
reuses a wrong before.

Verifiers: `press_marks_the_observation_stale_and_observe_clears_it` and
`stale_before_is_not_reused_after_a_press_without_observe_after`.

## Unknown page state is not empty

Status: 2026-10-05. Supersedes the empty-string url/title in "Page state,
richer diff, and no-effect verify".

`PageState::url` and `title` return `Option<&str>`. A history protocol error,
a missing `entries` array, or a `currentIndex` with no entry is
`PageState::unknown(focused)`. `PageState::blank()` is unknown. An empty URL in
a real history entry is a known `""`. `page_delta` reports `url_changed` or
`title_changed` only when both sides are known and differ.
`PageDelta::is_known` is true only when both sides know both fields.
`verify_delta` returns `NoEffect` only for an empty region diff, an unchanged
page, and a known page delta. MCP `state_delta` gains `title_changed`.

Downside accepted: with an unknown page state an empty diff reports the
specific expectation failure (for example `UrlUnchanged`), not `NoEffect`.
Unknown is not evidence of no effect.

Verifiers: `unknown_on_either_side_is_never_a_change_and_never_known`,
`unknown_page_state_is_never_no_effect`,
`executed_act_with_no_delta_is_no_effect` (now on a known page), and the
history-kind arm of `structured_cdp_pages_observe_with_unique_ids`.

## Duplicate AX-only backend ids

Status: 2026-10-05.

Two accessibility nodes with the same backend id and no DOM partner both
became `ax{id}`, and observe failed with `DuplicateRegion`. Fusion now keeps
the first as `ax{id}` and mints `ax{id}-2`, `ax{id}-3`. Two DOM nodes with
one backend id still fail with `DuplicateRegion`: that would be a broken
Chrome response, not a second accessibility view.

Downside accepted: the suffix follows AX tree order, so it is stable only
while that order is. The identity map still pairs regions across observations.

Verifier: `two_ax_nodes_with_one_backend_id_stay_separate`,
`two_dom_nodes_with_one_backend_id_are_the_exact_duplicate_error`.

## signature_jaccard removed

Status: 2026-10-05. `signature_jaccard` had no production caller and 11 of
the 19 surviving mutants in the audit. `detect` compares signatures by
equality. It was deleted rather than tested. Verifier: none needed; the
compiler owns the absence.

## Known limits, not fixed

Status: 2026-10-05. Recorded so they are not mistaken for verified behavior.

- `IdentityMap.minted` grows for the life of a session. Sessions are capped at
  four and dropped on failure, so this is bounded per session, not globally.
- No loopback check on the CDP endpoint. A caller can point the server at a
  remote `ws://` host. `wss://` is rejected.
- No timeout on the CDP WebSocket's TCP connect or handshake. After connect,
  reads and writes time out after 5 seconds (`ws.rs`), so a hung Chrome
  fails the call instead of hanging it, but an unreachable host can still
  stall the connect.
- `match_regions` builds an O(n*m) score table. The 2000-region smoke test
  passes; it is not a memory bound.
- Identity step 1 keeps a fused stable id across observations without
  checking role or label. A different control that reuses the same backend id
  inherits the stable id. A role check needs a cross-navigation policy and was
  deferred.
- `ToolError::Ranker` and `CompareError::TransportCalledBelowThreshold` have no
  exact test: the first is unreachable with the shipped matchers, the second
  because the gate runs before press.

Verifier: none. These are limits, not claims.

## RUSTSEC advisories in CI

Status: 2026-10-05. The MCP server opens a CDP WebSocket, so the graph is
network-facing. CI runs `cargo deny check advisories` with a `deny.toml` that
checks advisories only, with all features (including `jev`) and no ignores.
Licenses, bans, and sources are not checked: `publish = false` and there is no
policy yet. CI also runs `cargo check -p hyper-use-cli --features jev`.

Downside accepted: a new advisory can fail CI with no code change. That is the
point.

Verifier: the `deny` CI job. Local run on 2026-10-05: `advisories ok`.

## Mock environment before any live drive

Status: 2026-10-05.

Unit tests check one function at a time. They did not catch the stale
`before` or the empty-URL false delta, because both only show up across
calls. The mock environment runs caller flows across calls on one `Server`.

`Server::with_connector` takes `FnMut(&str) -> Result<Box<dyn CdpTransport>,
CdpError>`. `Server::new` passes `WebSocketTransport::connect`, so the stdio
binary is unchanged. `Box<T: CdpTransport>` implements `CdpTransport`. Tests
pass a connector that returns a `ReplayTransport` from `ScriptBuilder`,
wrapped to log each CDP method. `Server::live_sessions` exposes the kept
endpoints, oldest first.

What the mock environment owns:

- the observe, locate, inspect, act, diff, verify flow, with each call built
  from the previous reply, as a caller would;
- whether a press happened, from the CDP call log;
- session reuse, the stale flag, reconnect after a dropped session, and the
  four-session LRU cap;
- the raw 0.55 gate, the 0.05 margin, and caller `runner_up`;
- ring diff, eviction, NoEffect, and no-op signals as data;
- unknown page history.

What it does not own, and the manual live drive covers:

- Chrome's real response shapes, ordering, and timing;
- what a real page does after a click (navigation, re-render, async load);
- websocket failures and slow or hung sockets (reads and writes time out
  after 5 seconds; the connect does not);
- whether an agent driving the tool picks good queries and reads refusals
  correctly.

The live drive (`examples/live-drive/`) is manual and opt-in: JEV picks the
calls against a local Gmail-style test page in a throwaway Chrome. It is not a
benchmark. Its first run found that an `http://` CDP endpoint resolved to the
browser target, which has no `Page` domain; the resolver now picks the first
page target from `/json/list`.
It is not a benchmark of Browser Use or CUA, and no success rate is claimed
from the mock suite.

Downside accepted: a script emits only what the extractors read, in the order
the code calls. If Chrome changes a shape, the mock still passes. That is the
live drive's job.

Verifiers: the tests in `crates/hyper-use-mcp/tests/mock_env.rs`, the
Acme Mail replica in `crates/hyper-use-mcp/tests/acme_mail.rs`, and
`an_mcp_client_drives_the_closed_loop_over_stdio` in
`crates/hyper-use-cli/tests/mcp_stdio_mock.rs`. Reverting the
stale-observation fix fails
`act_without_observe_after_then_next_act_does_not_reuse_stale_before`;
reverting the unknown-page fix fails
`page_history_failure_is_unknown_not_a_false_delta`.

## Text-miss cap

Status: 2026-10-05 (Asia/Manila). From live drive t7.

JEV asked locate for "Send" as a link on the Acme Mail thread. No link is
named Send, so every candidate missed. The weighted V1 total for a miss was
still 0.50: semantic is `min(text, role)` = 0, but geometry scores 1 when no
position is asked (0.30) and actionability scores 1 when no action is asked
and the region has any action (0.20); every extracted region has at least
Focus. The role does not contribute. All 63 candidates tied at 0.50, and an
unnamed AX node (`ax11`) won on region id because `ax…` sorts before `n…`.

Both matchers now clamp a region's total to `TEXT_MISS_CAP` (0.45) when the
query has text and the label shares none of its tokens (`token_recall` is
0). WeightedMatcher applies it in `weighted_total`; HGRA applies it in
`score_parts`, so `locate`, `locate_with`, and `HgraMatcher` all carry it.
0.45 is strictly below the 0.55 act gate for any weights, so a nameless or
unrelated region can never be clicked from a text query. It is 0.05 below
the 0.50 a label hit with the wrong role gets under V1, so that control
ranks above every text miss. A `const` assert in the MCP crate ties the cap
to `MIN_ACT_CONFIDENCE`.

Eval corpus: weighted tops, margins, and gate decisions did not move. HGRA
margins on `twins.manifold` rose (Export 184 to 223, Admin 195 to 231, Undo
184 to 231 millis); HGRA tops and gate decisions did not move.

Downside accepted: a query whose text the label does not contain at all
(a typo, a synonym, an icon button with no accessible name) can no longer
reach the gate from text, even if role and position point at the right
control. The caller must fix the text or act on an inspected region. The
cap is a constant, not calibrated, and HGRA totals are still not comparable
to weighted ones.

Verifiers: the 256-case proptest
`a_nameless_region_never_reaches_the_act_gate_for_a_text_query` (arbitrary
role, query role, position, action, geometry, flag, and weights; both
matchers), `text_miss_is_not_rescued_by_a_role_hit`,
`a_label_hit_with_the_wrong_role_outranks_a_nameless_region`,
`the_cap_only_lowers_and_only_on_a_text_miss`,
`eval_corpus_reports_both_matchers_without_a_winner`, and the replica test
`t7_send_as_a_link_ranks_the_named_send_above_unnamed_nodes_and_does_not_click`.

## Region state in observe, inspect, and locate

Status: 2026-10-05. From the live drive: observe returned only id, role,
and label, so a caller could not tell the disabled Save from the enabled one
without running locate (which penalized the disabled one).

`RegionState` (hyper-use-resonance) is `Availability` (enabled, disabled)
and `Visibility`, a ladder where the most limiting condition wins: hidden,
then offscreen, then occluded, then visible. Offscreen is the offscreen flag
or a rect fully outside the viewport, the same test the locate penalty uses.
MCP observe regions, the inspect target, and locate candidates carry
`"state": {"availability": ..., "visibility": ...}`.

Downside accepted: zero size, stale, ambiguous, and detached are not in the
state; locate still penalizes them. `display:none` nodes are omitted by the
extractor, so `hidden` only shows for regions that arrive with the hidden
flag. Responses are a little larger.

Verifiers: `flags_and_geometry_map_onto_the_ladder`,
`the_most_limiting_visibility_wins`, and MCP
`observe_alone_tells_the_disabled_save_from_the_enabled_one`
(`fixtures/settings-saves.manifold`).

## Repeated locate query signal

Status: 2026-10-05. From live drive t8: after the twin Send refused as
ambiguous, JEV asked the same locate eight more times on an unchanged page.
`detect` compares snapshots around an act and could not see this.

The MCP `Server` remembers the last 8 locate calls (`REPEAT_WINDOW`) as
(origin, page `StateSignature`, normalized query). Text is normalized to its
lower-case tokens; role, position, action, matcher, and dims are kept. From
the 2nd identical call (`REPEAT_THRESHOLD`), locate's `signals` carries
`{"kind": "repeated_query", "count", "top": {"id", "suggested_position"},
"runner_up": {"id", "suggested_position"}}`. `suggested_position` is the
first zone (left, right, top, bottom, center) one candidate is in and the
other is not (`separating_zone`), or null.

N = 2 because locate is deterministic: the same query on the same state
returns the same ranking, so the first repeat already told the caller
nothing new. N = 3 would only let one more empty call through. The signal is
data: ranking, target, and candidates are unchanged, nothing is refused or
retried, and the stateless `call_tool` keeps nothing and never signals.

Downside accepted: the signature ignores geometry, so a page that only moved
counts as unchanged. A repeat older than 8 locate calls (across all origins)
is not reported. A suggested position can be null when the two candidates
share every zone.

Verifiers: `same_origin_signature_and_query_count_up`,
`text_is_normalized_but_every_other_part_counts`,
`a_changed_page_or_another_origin_starts_over`,
`calls_older_than_the_window_are_forgotten`,
`separating_zone_names_a_zone_only_the_target_is_in`, MCP
`a_repeated_identical_locate_carries_repeated_query_and_still_ranks`, and the
replica test `t8_twin_send_refuses_then_repeated_query_names_a_separating_position`.

## HGRA semantic parity and bundled query

Status: 2026-10-05 (Asia/Manila). Numbers in
`crates/hyper-use-resonance/HGRA_REMEASURE.md`.

HGRA's semantic term was `mean(token_recall, role_hit)`. It never got the
precision factor the weighted matcher gained in "Exact label over superset",
so "Send" and "Send feedback" both scored semantic 1.0 for "Send" and HGRA's
margin on `send-buttons.manifold` was 0.0277, refused by the 0.05 act margin.
HGRA now calls the weighted matcher's `weighted_semantic`:
`min(recall * (0.5 + 0.5 * precision), role_hit)`. One function, no drift.
This supersedes "The HGRA semantic term is not changed" in that section; the
pin `hgra_send_order_is_unchanged` is replaced by
`hgra_exact_label_outranks_superset_labels_with_lower_ids`.

The hypervector term was the mean of one cosine per query probe. It is now
`cosine(bundle(probes, weight 1 each), signature)`: the query is composed with
the same `bind` and `bundle` as the signature and compared once. Equal weight
keeps the probe set the mean used. The signature itself is unchanged.

Result on the 5-case locate corpus: HGRA tops unchanged (5/5, 5/5 agreement
with weighted); the Send margin goes 0.0277 to 0.0859 and the gate now allows
it, so gate decisions agree with weighted on 5/5 (was 4/5). Ablation:
parity alone gives Send 0.0777; the bundle alone gives 0.0359; the bundle adds
0.039 on the 3-constraint sidebar query.

Downside accepted: with min, a wrong-role label hit scores semantic 0 in HGRA
(it was 0.5), as it already did in weighted. A two-probe query bundles with
many zero-sum components that tie to `+1`, so the bundle helps little there.
HGRA totals are still not calibrated to weighted totals, and HGRA is still not
the product default; the live suite has not been rerun with `matcher: "hgra"`.

Verifiers: `hgra_exact_label_outranks_superset_labels_with_lower_ids`,
`bundled_query_is_one_cosine_and_widens_the_send_margin_over_probe_mean`,
`hgra_label_hit_with_the_wrong_role_outranks_a_nameless_region`,
`hgra_score_parts_are_exact_and_the_total_is_their_weighted_sum`, and the
printing test `remeasure_hgra_against_weighted_on_the_locate_corpus`.

## Harder page types (ADR 0007)

`DOM.getDocument` uses `pierce: true`. Walk order: `children` → open
`shadowRoots` → same-origin `contentDocument`. Roles `combobox` / `listbox` /
`option` are first-class. Virtualized lists expose only the visible window;
autocomplete is TYPE then ticketed option CLICK after re-observe. Still
invisible: cross-origin iframes, closed shadow, off-window recycler rows.
