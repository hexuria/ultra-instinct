# ultra-instinct

The product is `ultra-instinct`; crates are `ultra-instinct-*` (Autonomous Ultra
Instinct) and binaries are `ultra-instinct` plus its short alias `aui`.
The HGRA matcher experiment and the weighted matcher are removed. A crate,
binary or feature
named `hgra` or `resonance` is a bug.

**Product:** Ultra-Instinct is a **Rust-native browser-agent runtime**.
The library `Agent` **owns the loop**: it observes the page, builds a finite
`ActionSpace`, lets a policy choose (Instinct by default; a remote decider —
JEV, Clef, or the `TEXT_MODEL_*` model — only when chosen with `--decision`),
hard-gates the choice, executes only through a one-shot `ActionTicket`, then
re-observes, diffs, and verifies.

MCP (`ultra-instinct mcp`) is an **optional adapter**, not the orchestration
surface. Nothing on the product path requires MCP, Browser Use, CUA, or Jev.

Public API is 0.1 and unstable until 1.0. Toolchain pin: Rust 1.99.0
(`rust-toolchain.toml`). Product crates publish to crates.io as
`ultra-instinct-*`. `#![forbid(unsafe_code)]` on every crate.

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
>    into "top candidate wins". Habituation only lowers the urge of
>    a repeated `dispatched-no-visible-change` / wrong-effect action; it never
>    adds candidates or bypasses the gate. A click whose only change is a
>    target's control state (checked / expanded / value / selected /
>    aria-selected / aria-current) verifies
>    as state-changed, without that state entering the fingerprint.
> 3. **Hard invalidity is guard evidence, not a score.** Disabled, readonly
>    (TYPE / SELECT), hidden / zero-area, occluded, front-layer, offscreen,
>    missing target, unsupported action → `ultra_instinct_guard::gate` refuses.
>    MCP / CLI `guard` finds its target by exact label (trimmed, whitespace
>    collapsed, case ignored; plus role and `within`), unique or refuse
>    (no match → `missing-target`, several visible → `ambiguous`, a
>    `proposed` id must be a match), then runs the same hard gate and issues
>    an `of_target` ticket. There is no score and no float gate.
> 4. **One executor boundary, one staleness barrier.** `execute_ticketed` is
>    the only path from a ticket to page input. Order: ledger (one-shot) →
>    input kind == `ticket.action` → **fresh observe** → `revalidate`
>    (target-scoped world fingerprint + target role / label / fingerprint) →
>    hard gate on the fresh region → **mark consumed before dispatch** →
>    dispatch to `ticket.target_id`. The agent's CLICK is a trusted pointer
>    press at the region center (`Input::Click`); the DOM `this.click()` tiers
>    are its CDP-refusal fallback and the retry (`Input::ScriptClick`) after a
>    pointer click shows no visible change. The MCP host's
>    `browser_click` (and `browser_check` / `browser_download`) presses the
>    same trusted pointer by default; `mouse: false` sends the DOM script
>    click. The host
>    helper `consume_ticket_once` uses
>    the same consume-before-press order. `Predicted` carries no fingerprint of
>    its own; do not add a "pre-ticket check" that is not wired into a refusal.
>    The user's `Policy` (guard `policy.rs`: allowed domains, confirm-before-press)
>    is checked on the page the input starts on, and on where a press that
>    submits a form sends it (`ElementState::form` / `submit_action`; other
>    navigation is checked at the next input there), by `execute_ticketed_with`
>    on the fresh observe's URL and region
>    after `revalidate` and the hard gate, before mark-consumed;
>    `execute_ticketed` runs it with an empty policy. The agent also checks it
>    after the gate; `consume_ticket_once_checked` runs it after `revalidate`,
>    before consume. An empty policy allows everything. With an allow-list,
>    only `http` / `https` on a listed host passes (plus `about:blank`, and
>    a `blob:` by its origin, none with `Policy::refuse_blob`); `data:`,
>    `javascript:`, `file:` and other
>    schemes refuse, and a host is read only after a real `scheme://`
>    (`Policy::check_url`, `host_of`). A confirm applies to
>    a press (`Policy::press_label`): the CLICK family on the target, an
>    activating KEY (Enter / Space / NumpadEnter, any modifiers) on a
>    button-like target (Enter on a checkbox / radio submits its form
>    instead), and Enter in a form control, which presses its
>    form's implicit-submission button by its accessible name (observe's
>    `ElementState::form` evidence; a button-less form Enter still submits,
>    or unread evidence → `FORM_SUBMIT_UNKNOWN` refuses while any pattern is
>    set; an activating key to an unnamed focus in the keyboard tools →
>    `UNOBSERVED_FOCUS`; a nameless submit button is `FORM_SUBMIT_UNKNOWN`).
>    The executor's refusal (`ExecError::Policy { reason, label }`) names the
>    label its fresh look checked. Space in a field and other keys press
>    nothing. One rule for the agent, the executor's fresh re-check and the
>    host tools.
> 5. **Stale ≠ spent.** `WorldChanged` / `TargetChanged` / `TargetGone` are
>    stale → discard the prediction, re-observe, decide again (bounded by
>    `max_consecutive_stale`). `TicketConsumed` / `TicketMismatch` / gate
>    refusal / page rejection / dispatch failure are **not** stale and are
>    never silently retried with the same lease. `DomainNotAllowed` and
>    `NeedsConfirm` are not stale: the agent ends BLOCKED
>    (`Agent::policy_refusal`), `run` prints `rerun with --confirm "<label>"`,
>    never retried with the same lease.
> 6. **Ticket worlds are target-scoped.** `WorldSnapshot::of_target`: front
>    layer and focus are global; clickable / occluded sets are the target
>    neighborhood (ancestors, same-parent siblings, children, root peers within
>    `NEIGHBOR_RADIUS_PX` = 160, inclusive). MCP `seen_world` stays whole-page
>    (`WorldSnapshot::of`). A DRAG ticket binds two ends: the source world as
>    above and a `TicketDest` with the destination's own neighborhood (and,
>    for a drop on a surface point, its grid `cell`); a surface-point ticket
>    binds its grid `cell`; a KEY ticket binds its one key. The executor
>    refuses a swapped destination, another cell, another key, or a two-end /
>    cell / key ticket on any other input (`TicketMismatch`), and re-checks
>    the drop target.
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
>    A literal right after a press verb (`press "Submit"`) names a control,
>    not a value.
> 7a. **Model text is payload-only and grounded** (feature
>    `model-text`). The model fills the payload of an action Instinct already chose;
>    replies must echo the context fingerprint, pass shape checks, and occur
>    in the goal clause and not be a described phrase ("your answer"), else
>    deterministic fallback, else abstain. One exception (on by default in
>    the CLI whenever a text model is configured, off with
>    `--no-derived-text`): a value read or worked out
>    from page text may be typed when the goal quotes no untyped value, the
>    field is not secret (password / hidden / file inputs; PIN /
>    CVV / card / one-time-code labels), and the value is one line of ≤ 64
>    chars, or copied word for word from the page text; every derived value
>    is printed. The model sees the page's reading text (rendered text plus
>    text-area contents) and the field's near text. CI uses scripted models only; no
>    provider SDK or key handling in the tree.
> 7b. **Harder page types.** Observe pierces open shadow roots and
>    same-origin iframes (`pierce: true`). ComboBox / ListBox / Option are
>    first-class roles. Virtualized lists are the visible window only (scroll
>    + re-observe). Autocomplete is TYPE then ticketed option act after
>    re-observe. After combobox TYPE, read-only option settling is limited to
>    10 polls at 25 ms intervals and reads its `aria-controls` / `aria-owns`
>    popup when present. The document fallback ends early only after visible
>    options change from the first poll and stabilize; input is never
>    re-dispatched. A plain element the page made clickable (an `onclick`
>    attribute or handler property, `tabindex` ≥ 0, or a pointer cursor it
>    sets itself rather than inherits) is offered CLICK, and so is
>    a plain box Chrome reports `isClickable` (one `DOMSnapshot` call: listeners
>    added by script). Also kept: SVG shapes (named by title / text / fill; not an icon inside
>    a click control), `LI` with text,
>    text-less boxes only with a click hint, `canvas` / `svg` surfaces (claim
>    `point` only with click evidence), scrollable boxes (claim `scroll`), popup owners (claim
>    `hover`), and drag-marked elements (claim `drag`). Icon-only controls
>    are named by title / alt / class or id words plus their row's rendered
>    text; unrendered text names nothing, and neither does a text area's
>    own text (its value). Observe
>    also records control state (value, checked,
>    expanded, `selected_item` (aria-selected), `aria_current`, select
>    options) as evidence only: it is not in the region fingerprint, and
>    password values are never read. Labels drop zero-width characters
>    (U+200B-D, U+2060, U+FEFF, U+034F, U+00AD; `ultra_instinct_core::INVISIBLE`)
>    once, as fusion builds each DOM and AX node; every label match (guard
>    exact label, confirm globs, `browser_wait`) uses
>    `ultra_instinct_core::normalize_label` on both sides. A label (`for`,
>    wrapping, or `aria-labelledby` text) folds into the one offered control
>    it names, in fusion, with compact hit evidence; a zero-size, transparent,
>    or covered checkbox / radio folds the other way, onto its label's box.
>    Do not claim cross-origin
>    iframe or closed-shadow coverage.
> 8. **Remote escalation is explicit and closed.** Feature `remote`;
>    reply is exactly a choice id + kind from the offered menu or abstain.
>    Selectors, coordinates, scripts, extra fields → hard error. The request
>    may carry evidence a transport asks for (`wants_evidence`):
>    the viewport JPEG beside it (`jev`: a System One image, `--no-screenshot`),
>    `page` {url, title, ≤ 6000 chars of visible text} (`jev`, `clef`,
>    `--no-page-text`), per-target `box` / `near_text`, per-action `tried`
>    {times, last} when the action was already dispatched with no effect
>    (evidence only — the reply may still pick it), and the run's
>    `progress` (clauses, which are satisfied, values typed; secrets masked).
>    Evidence never widens the reply. New kinds are still offered
>    ids: `HOVER:<id>`, `SCROLL_IN:<id>`, `DRAG:<src>><dst>` (up to 8
>    sources × 96 destinations, sent as one compact `drag` block) including
>    `DRAG:<src>><surface>@c,r` onto a cell of the drawing a source sits on,
>    surface cells `CLICK:<surface>@c,r` (8×8), `KEY:<id>#<key>` from the
>    closed `KEY_NAMES` set, and `SELECT_TEXT:<id>` (a trusted triple-click,
>    verified by the page's selection). JEV's DONE carries a
>    same-call `goal_satisfied` self-check; DONE below 0.7 (0.5 once every
>    clause is satisfied) is re-asked without DONE, and BLOCKED below 0.6 is
>    re-asked without BLOCKED: at most three calls, a second
>    refused DONE abstains. With `--second-opinion` the text model is asked
>    when JEV ends on BLOCKED, abstains, or gives its operation < 0.5; its
>    reply must be exactly an offered id + kind, else JEV's answer stands.
>    Targets come before page controls in the request;
>    near text and row text that echo the goal are not sent.
>    `--decision model` (feature `model-text`, `decide.rs`) sends the same
>    request to the `TEXT_MODEL_*` model with the viewport JPEG and page
>    text; a reply that is not an offered id + kind is asked once more with
>    the reason, and the second reply goes to `parse_reply` unchanged (both
>    share `reply_shape`). Its DONE is checked by a second question (goal
>    met?); a refused DONE is asked again without it, a second one abstains.
> 9. **No matcher.** HGRA and the weighted matcher are removed.
>    Do not reintroduce label ranking on the agent path or in `guard`.
> 10. **One blocking CDP socket per agent.** Parallelism means separate agents
>    in separate tabs with no shared state. Formal tools stay unjustified for
>    this path (no `unsafe`, no atomics shared across threads, no recovery
>    protocol): Loom / Kani / TLA+ / Miri / Lean are **NOT JUSTIFIED**. The
>    owners are unit tests, `proptest`, adversarial fixtures, replay fixtures,
>    cargo-mutants (nightly), and cargo-fuzz on the untrusted-input parsers
>    (nightly). `aui daemon --session <name>` (unix, std sockets, single
>    thread) owns one MCP `Server`: one session is one CDP socket; one
>    connection and one line at a time, no thread pool. `aui open --launch`
>    makes the daemon own its Chrome for the session's life.
>
> Any change to observable semantics on this path names the boundary it
> affects (gate, ticket, executor, world, policy, text) and the test that owns
> it.

## Crates (product graph)

| Crate | Owns |
|---|---|
| `ultra-instinct-core` | `InteractionManifold`, regions, `ActionSpace`, fixture grammar |
| `ultra-instinct-cdp` | CDP transport trait, websocket, replay transport, replay script builder; unix `--remote-debugging-pipe` transport (`PipeBrowser` / `PipePage`, caller-owned fds, one thread); tab backends: `TargetBackend` (list / create / activate / close pages, connect a session key), `HttpBackend` (the `/json` endpoints + a connector, the default) and unix `PipeBackend` (`Target.*` on a `PipeBrowser`, keys `pipe:<targetId>`) |
| `ultra-instinct-chrome` | Find / start a local Chrome (`find_chrome`, `launch`, `ChromeProcess` killed on drop; headless at `DEFAULT_WINDOW` 1440×900, headed maximized, `LaunchOptions::window_size` overrides); std only, re-exported as `ultra_instinct_browser::launch` |
| `ultra-instinct-browser` | CDP observe (DOM/AX fusion, identity, stacking), raw CDP inputs; `launch` (re-export of `ultra-instinct-chrome`); `storage_state` (save / load cookies + storage, the 0600 state file and its parser) |
| `ultra-instinct-observe` | Observation history and id diff |
| `ultra-instinct-geometry` | Geometry helpers; `RegionState` (availability / visibility of a region) |
| `ultra-instinct-policy` | `BrowserPolicy` + `PolicyContext` (`set_situation`, default no-op), `InstinctPolicy` (`instinct-*` 0.1.0 from crates.io) + `TRUST_CAP_MILLIS`/`set_evidence_adjustments` (bounded learned-trust evidence), `TextResolver`, `ModelTextResolver` (feature `model-text`), `RemotePolicy` (feature `remote`), multi-step clause split |
| `ultra-instinct-guard` | Hard `gate`, `ActionTicket` issue / `revalidate`, `TicketLedger`, `consume_ticket_once` / `consume_ticket_once_checked`, front layer + `WorldSnapshot`; the user's `Policy` (`HostPattern` allowed domains, `LabelGlob` confirm-before-press); exact-label `guard()` for MCP / CLI preflight |
| `ultra-instinct-agent` | `Agent` state machine, `execute_ticketed` / `execute_ticketed_with` (policy), `AgentBuilder::guard_policy`, `AgentBuilder::deadline` (checked between steps), `BrowserRuntime`, `BeforeDispatch` (forwards every `BrowserRuntime` method and runs a hook just before `dispatch`; handed only to `execute_ticketed_with`), verification mapping, `MockBrowser` |
| `ultra-instinct-protocol` | Guard / ticket / verify wire types |
| `ultra-instinct-browser-tools` | The `browser_*` host surface as MCP tool families (`call` / `spec`, the tool-name lists, `ToolError` with a stable `code` on every variant) over a `BrowserHost` trait (sessions, tab binding, ledgers, events, snapshot record and lookup, `require_snapshot`, `Policy`, the tab `backend`, `ToolLimits`, `Downloads`, `Uploads`, the tools' own `ToolState`): snapshot-bound index refs (`bound.rs`), secret masking (`secrets.rs`: `sensitive`, typed secrets scrubbed from every output), browser-use names, `browser_wait` conditions, `browser_extra` (dialog, forward / reload, check, focus, keyboard, get / is / count, annotate, `max_chars`), `browser_state` (state save / load, cookies, storage), `browser_env` (set, download, network) |
| `ultra-instinct-mcp` | Optional JSON-RPC adapter (observe / guard / verify), `TOOLS`, and the `Server` that keeps sessions and the snapshot ring and serves the `browser_*` tools through a crate-private `BrowserHost` adapter (`Server` itself does not implement it); host options (`with_pipe` / `with_backend`, `require_snapshot`, `bind_to_latest_look`, `set_policy`, `with_limits`, `with_downloads`, `with_uploads`, `with_tool_allowlist`); `run_goal` / `run_goal_on` / `Server::run_goal` (the agent loop with InstinctPolicy and the deterministic resolver, no model); re-exports `ToolError`, `Policy` and the tool-name lists ([`docs/embedding.md`](docs/embedding.md)) |
| `ultra-instinct-remote` | Remote decision transports and the HTTP text model, the only crate with a model SDK / HTTP client: `typesafe` (feature `jev`), `clef` (feature `clef`), `http_text` (feature `model-text`, OpenAI-compatible / Anthropic / Gemini, with an optional JPEG), `decide` (feature `model-text`: `ModelDecider`, the same model choosing every step over the closed wire); re-exported by `ultra-instinct` |
| `ultra-instinct` | `ultra-instinct run` (agent loop, `--decision instinct|jev|clef|clef-flash|dojo|model` (`--policy` is an alias), `--diary <dir>`); `replay --diary` (offline arena: agree / would-abstain / regress per recorded decision); `observe` / `guard` / `verify` preflight commands; `mcp` (stdio); the shell verbs over the session daemon (`verbs.rs`, `daemon.rs`), `batch`, `session`, `skills`, `doctor`, `install`; `run --launch/--load-state/--save-state/--allow-domain/--confirm-press/--confirm`; `TypesafeTransport` (feature `jev`: JEV-primary over the closed remote wire); `ClefTransport` (feature `clef`: Cloudflare Workers AI Clef / Clef-Flash over the same wire through `typesafe-sdk`'s Clef provider, one decision call, single-candidate heads answered locally) |
| `ultra-instinct-dojo` | Battle diary schema v1 (`DiaryLine` JSONL), `DiaryWriter` / `read_diary`, site + situation keys, decision→`ActionSpace` rebuild for the replay arena; versioned `LessonStore` (words / places / moves / trust, keyed per situation, carrying proving diary ids); lessons only adjust evidence inside the finite `ActionSpace` |

Instinct (`instinct-core` / `-text` / `-lexicon`) is pinned to 0.1.0 from
crates.io in the workspace `Cargo.toml` (`INSTINCT_VERSION_REQ`); bump only with a
deliberate eval.

## Verification owners

- Agent loop, stale discards, multi-step clauses: `ultra-instinct-agent` unit tests,
  `tests/mock_loop.rs`, `tests/adversarial.rs`, `tests/replay_cdp.rs`.
- Executor boundary: `executor.rs` unit tests, `tests/props.rs`
  (substitution, staleness, consumed-beats-stale, `is_stale` classification,
  drag destination and surface cell substitution).
- Gate / ticket / world: `ultra-instinct-guard` unit tests,
  `tests/ticket_props.rs` (radius boundary 159 / 160 / 161, focus-only change,
  consumed never stale), `tests/world_context.rs`.
- User policy (allowed domains, confirm-before-press): guard
  `tests/policy_props.rs`; the `executor.rs` policy tests; the
  `tests/mock_loop.rs` policy tests (`needs_confirm_ends_blocked_without_pressing`,
  `domain_off_the_allowlist_ends_blocked`,
  `enter_in_a_field_whose_form_submits_with_a_confirm_label_blocks_unpressed`,
  `a_blob_page_mid_run_ends_blocked_with_refuse_blob`);
  the guard's `activating_keys_are_presses_of_the_target_or_its_form`; the
  `refuse_blob` switch: guard `domains_exact_and_subdomain_patterns`, mcp
  `browser_tools.rs` `navigate_to_a_blob_is_refused_with_refuse_blob` and
  `host_options.rs` `the_env_policy_parses_refuse_blob_and_fails_closed`.
- Shell verbs and the session daemon: `verbs.rs` / `batch.rs` unit tests and
  the `daemon.rs` socket tests (round trip, spawn / stale socket, idle exit,
  `--idle`, failed launch, session list; a press after the page changed
  since `snap` refuses unpressed, a press before any snap refuses;
  `bind_to_latest_look` in `snapshot_refs.rs`). `browser_*` families over CDP
  replay: `crates/ultra-instinct-mcp/tests/browser_tools.rs`,
  `browser_extra.rs`, `browser_state.rs`, `browser_env.rs`, and the
  `ultra-instinct-browser-tools` unit tests. A call that
  opens a JS dialog returns `CdpError::DialogOpened` at once: ws.rs
  `a_dialog_opening_ends_the_wait_and_stays_buffered`.
- Instinct evidence: `ultra-instinct-policy` unit tests. Do not add a second model of
  Instinct scoring.
- MCP / CLI `guard` (exact label, unique or refuse): `ultra-instinct-guard`
  unit tests and `tests/world_context.rs`; over the wire,
  `ultra-instinct-mcp` `tests/server.rs`, `tests/world_context.rs`,
  `tests/acme_mail.rs`.
- `write_fixture` / `parse_fixture` own the manifold fixture grammar. CDP
  replay is a different grammar. Fusion is the only DOM/accessibility merge.
  Region identity across observations is owned by the browser `IdentityMap`.
- Untrusted input (fixture grammar, CDP replay scripts, CDP pipe frames, MCP stdin lines,
  remote-model replies, model-text replies): `fuzz/` cargo-fuzz targets on a
  dated nightly (`scripts/fuzz-smoke.sh`, `.github/workflows/nightly.yml`),
  backed by the structured/garbage proptests at small case counts. Saved
  state files: the parser proptests in
  `crates/ultra-instinct-browser/tests/storage_state.rs`.
- CDP pipe transport (framing, routing, dialogs, target gone, fail-closed
  limits): `crates/ultra-instinct-cdp/tests/pipe.rs` against a fake Chrome on
  `std::io::pipe()`, with the frame-splitter chunking proptests; the
  `pipe_frame` fuzz target; live Chrome in `tests/pipe_live.rs` (`#[ignore]`).
- Embedding over a pipe ([`docs/embedding.md`](docs/embedding.md)):
  `crates/ultra-instinct-mcp/tests/pipe_mode.rs` (`Server::with_pipe` against
  a fake Chrome: tabs as `Target.*`, `target_gone`, `pipe_closed`),
  `snapshot_refs.rs` (a shifted page refuses `stale_ref` with no press;
  `require_snapshot`), `secrets.rs` (a typed password appears in no output,
  error, or HAR), `host_options.rs` (runtime policy, confirm not sticky,
  limits, absolute paths, downloads, allow-list), `uploads.rs`
  (`with_uploads`, canonical paths), `keyboard_policy.rs` (the keyboard
  tools under the policy), `run_goal.rs`; the code
  table: `crates/ultra-instinct-browser-tools/tests/error_codes.rs`; live
  Chrome: `crates/ultra-instinct-mcp/tests/pipe_live.rs` (`#[ignore]`).
- Observe protocol cost: `crates/ultra-instinct-browser/tests/call_budget.rs`
  pins the CDP call count per observe as a regression tripwire.
- Repo rules (toolchain pin, `forbid(unsafe_code)`, crate ceilings, pinned
  nightlies): `scripts/check_repo_rules.sh`. Dependency direction and the
  no-async/no-model-SDK ban below `ultra-instinct`:
  `scripts/architecture.txt` + `scripts/check_architecture.py`. Both run in
  `ci.yml`'s `repo-rules` job.
- Workflow integrity: `zizmor --persona=auditor` in `ci.yml`. Dependency
  stagnation: `.github/dependabot.yml`.
- Mutation testing: `.github/workflows/mutants.yml` (nightly + manual) on
  `gate.rs`, `ticket.rs`, `executor.rs`, and `world.rs` (`of_target` /
  `neighborhood_of` / `nearby`). Equivalent mutants are excluded in
  `.cargo/mutants.toml`. Run locally (`cargo install cargo-mutants --locked`
  once): `cargo mutants --no-shuffle -j 3 --timeout 180 -o /tmp/mut-a -f
  crates/ultra-instinct-guard/src/gate.rs -f crates/ultra-instinct-guard/src/ticket.rs
  -f crates/ultra-instinct-agent/src/executor.rs`, and `-f
  crates/ultra-instinct-guard/src/world.rs --re 'of_target|neighborhood_of|nearby'`
  for the world set. Exit 0 = all caught, 2 = missed (`mutants.out/missed.txt`).
- Label folding: `crates/ultra-instinct-browser/tests/label_folding.rs` and
  `crates/ultra-instinct-mcp/tests/label_folding.rs`.

Live Chrome tests stay `#[ignore]`; CI uses CDP replay fixtures. The live
tests are `crates/ultra-instinct-agent/tests/live_smoke.rs` (the owned loop)
and `crates/ultra-instinct/tests/live_borrow.rs` (the shell verbs against
their own `aui open --launch` Chrome and a 127.0.0.1 test server: state,
cookies, set, download, network / HAR, dialog, close / idle lifecycle,
`--headed`, stdin batch; run with `-- --ignored --test-threads=1`).

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
verify. The locate, inspect, act and diff helpers are removed:
`observe` carries every region's id, label, parent and state, and `verify`
compares two snapshots. It owns a 16-entry snapshot ring and up to four live
CDP sessions; a failed live call drops its session, no retry. Guard finds its
target by exact label (and `role` / `within`; `proposed` picks one of several
matches), refuses `near` / `position` / `matcher` / `dims` by name, runs
`gate::check` before Allow, and issues `of_target` tickets like the agent. MCP
never bypasses tickets and is never required to run the agent.

`browser_*` tools are the browser-use-compatible host surface (crate
`ultra-instinct-browser-tools`, run on the MCP `Server` through a
crate-private `BrowserHost` adapter; drop-in MCP names: `browser_navigate`, `browser_get_state`, `browser_click`,
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
`browser_search`, `browser_form_input` (type alias). Every tool and `aui` verb, one line
each: [`docs/tools.md`](docs/tools.md).

`browser_get_state` defaults to text: a title / url line, then one line per
control, `@eN role "label"  state`; `format:"json"` returns `elements[]`
(with `input_type` and `sensitive`; `values:true` adds `value` / `checked` /
`expanded`, never on a sensitive element). Both formats record the observation and
return its `snapshot` id. Index arguments take `5`, `"5"` or `"@e5"`; an
index press given `snapshot` resolves the index in that snapshot and mints
its ticket there, so an element that moved, changed, or left refuses
`stale_ref` and nothing is pressed (`Server::require_snapshot(true)` refuses
a bare index, `snapshot_required`). Over MCP the text format also carries
the id in the result's `_meta.snapshot`, and a cut text keeps its
`· snapshot N` line. Input tools (`STATE_AFTER`) take `state: true`: the
reply adds the page after the input (`state`, text) and its `snapshot`,
or `state_error` (the input still ran). `Server::bind_to_latest_look(true)`
binds a press that names no snapshot to its tab's latest look (a
`get_state` or a `state: true` reply); the `aui` daemon sets it with
`require_snapshot(true)`, so a verb press binds to the latest `aui snap`
(or `--snap`) and refuses `snapshot_required` before any. A sensitive field (password / hidden /
file, a secret label, or an unreadable type) never shows its value, and text
typed into one is masked in every later output, error, and HAR. Every error
carries a stable `code`; the table is in [`docs/embedding.md`](docs/embedding.md).
`Server::with_pipe` serves the same tools over a caller-owned pipe (no HTTP
endpoint; `cdp` is ignored); `with_tool_allowlist`, `with_limits`,
`with_downloads` and `set_policy` are the host's knobs. File paths
(screenshot, PDF, state, download, HAR) must be absolute.

`browser_wait` with `text` / `url` / `control` / `gone` (no control has that
exact label) / `load` (`domcontentloaded` | `load` on `document.readyState`,
never network idle) observes about every 200 ms until every given condition
holds or `seconds` (default 10, cap `ToolLimits::max_wait`, 60 s) runs out; it never acts.

Policy: `ULTRA_INSTINCT_ALLOW_DOMAINS` / `ULTRA_INSTINCT_CONFIRM_PRESS`
(comma lists) set the server's `Policy`, and `ULTRA_INSTINCT_REFUSE_BLOB=1`
refuses every `blob:` page under the list; a press tool's `confirm` argument
names the label the user confirmed, for that call only (`Server::set_policy`
replaces the policy between calls). Ticketed inputs return `ticket`;
`verify:true` adds `verification`. `browser_send_key` with `index` is a
ticketed KEY. Input with no target (`browser_keyboard`, `browser_send_key`
/ `browser_hold_key` without `index`) checks the page host on an
observation taken now, before any `Input.*` call, the way presses do;
the focused region then goes through the guard's press rule (Enter /
Space on a button-like control, Enter in a field whose form submits with
a confirm label, needs `confirm`), as a ticketed KEY does. `browser_file_upload` paths
must be absolute; `Server::with_uploads(Anywhere | Off | Dir)` limits
which files (`Dir`: canonical path inside the canonical folder).

`browser_extra` ports (from agent-browser): `browser_dialog`,
`browser_go_forward`, `browser_reload`, `browser_check`, `browser_focus`,
`browser_keyboard`, `browser_get`, `browser_is`, `browser_count`,
`browser_screenshot` `annotate`, and `max_chars` on the text tools. Index
tools read the index from the tab's last observation when no input ran
since (or from `snapshot` when given), and act on that region in a fresh
observation. `browser_check` and `browser_focus` run under the policy and a
per-call `confirm` like `browser_click`. Page is enabled once
per tab outside observe for dialog tracking; a failing call while a dialog
is open says so. A call that opens one (a press whose handler calls
`alert()`) stops waiting at `Page.javascriptDialogOpening` and fails at once
with that hint (`CdpError::DialogOpened`); the press was dispatched, its
ticket stays consumed, and nothing retries it.

Login state (`browser_state_save` / `_load`, `browser_cookies`,
`browser_storage`): values are masked unless `reveal`; state files are 0600
and load refuses a file with group or other permissions; storage is written
only for the current origin, other origins come back as pending.

`browser_set` (viewport / device / media / offline / headers / reset),
`browser_download` (a ticketed press, then the file moves to `path`) and
`browser_network` (block / unblock via `Network.setBlockedURLs`, never Fetch
interception, because the blocking server cannot answer paused requests;
`har_start` / `har_stop` writes HAR without bodies, secret headers `•••`,
0600).

## Signals

Signals (`no-op`, `loop-detected`) are data for the MCP host
journal. They do not retry and do not select an executor.
