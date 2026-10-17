# ultra-instinct uniform benchmark: results

> **Historical.** These A1–A8 numbers are from the pre-ActionTicket uniform
> bench (tip around `4ae30d3`). They are **not** proof of the current firewall
> product. The keep/kill experiment is B0/B1/B2 (`bench/arms/B01.md`): same
> Browser Use, invisible Ultra-Instinct interceptor, Weighted only, no JEV/HGRA.
> HGRA development is frozen until that ablation finishes.
>
> **Live remasure note (2026-10-06):** older A3/A5 leaderboard cells are
> stale for JEV+ultra-instinct claims. See **Mac remasure 2026-10-06 — A3 vs A8**
> below (fair Luna+JEV both sides).

## Claude Code agents: ultra-instinct MCP vs obscura + pageshot (2026-10-17)

> Tip `f614e64` (after #75). Claude Code 2.1.302 subagents, Claude Haiku 5.5
> at max effort, Chrome 155 headless, one local site (Leptos/WASM SPA, theme via
> cookie), one five-part task (read title and heading, check hero corners, two
> clicks with state checks). **n = 2 per arm.** Only the agents' browser tooling
> differs. Writeup: [`bench/results/AGENT-MCP-COMPARE-2026-10-17.md`](bench/results/AGENT-MCP-COMPARE-2026-10-17.md).

| Arm | Correct | Tokens (mean) | Tool calls (mean) | Wall time (mean) | Screenshots |
|---|---|---|---|---|---|
| obscura + pageshot | 10 / 10 | 143,702 | 41.5 | 8.2 min | starting state only (pageshot cannot click) |
| ultra-instinct `aui mcp`, before #75 | 10 / 10 | 106,694 | 24.5 | 4.3 min | none viewable (base64 text) |
| **ultra-instinct `aui mcp`, after #75** | **10 / 10** | **99,958** | 33.5 | **3.7 min** | **every state, including after each click** |

Same accuracy, 30% fewer tokens, 2.2× faster than obscura + pageshot, with
rendered evidence for every state. This measures `browser_*` tools under an LLM
agent; no run used `aui run`, Instinct, JEV, or Clef.

## Agent + Instinct pivot (offline arm C)

> Post-ActionTicket product path. Tip of this section: see git history for the
> merge commit that lands PR gaps (readonly observe, target-scoped world,
> multi-step `then`, MCP hard-gate alignment, `ultra-instinct run` fixtures).

Date: 2026-10-06 (Asia/Manila). Harness: `cargo test -p aui-agent` (mock
loop, adversarial, CDP replay, props) + `cargo test -p aui-guard --lib`
+ `cargo test -p aui-policy --lib` + checked-in
`fixtures/agent-*.cdp.json` via `ultra-instinct run --fixture`.

**n = 3** consecutive offline runs (same machine, no Chrome, no network, no
model). All three green.

| Run | Wall (real s) | Result |
|---|---|---|
| 1 | 0.81 | pass |
| 2 | 0.34 | pass |
| 3 | 0.36 | pass |

What this measures: the owned loop (observe → Instinct → hard gate → ticket →
executor → verify) including TYPE_TEXT / SELECT / CLICK / SCROLL, adversarial
refuses, target-scoped stale discard, multi-step `then`, and CDP replay
fixtures. It does **not** measure live sites, Luna, JEV, or Browser Use.

Arms A / B / D live parity: **blocked** (need unpaid `jev-ultrafast` / paid
remote). Historical A1–A8 table below is pre-pivot and not agent+Instinct evidence.


## Mac remasure 2026-10-06 — A3 vs A8 (acme-mail, fair parity)

> Tip `acb3a33`. Host: Uriah Mac. Seed 42, n=1, Luna up (`:8080` HTTP 200).
> Vendor jev-ultrafast pin `1231850a…`. Run id `20261006-081905`.
> Writeup: [`bench/results/MAC-A3-A8-COMPARE.md`](bench/results/MAC-A3-A8-COMPARE.md).

**Older A3/A5 rows in the leaderboard below are historical/stale** for live
JEV remasure claims (pre-ActionTicket uniform bench around `4ae30d3`, and the
unfair A3-vs-A5 press-only Mac pass). Prefer this section for current A3/A8.

### Unfair precursor (A3 vs A5) — do not use for parity

Mac run `20261006-074743`: A3 4/7 vs A5 2/7. A5 is JEV+ultra-instinct **presses
only** (no Luna typing). Not a fair stack comparison.

### Fair remasure (A3 vs A8)

Same planner/decider shape on both sides; only the browser layer differs:

| Arm | Stack | Full | Press-only | Pass wall mean |
|---|---|---|---|---|
| **A3** | jev-ultrafast (JEV + Luna-for-text) + browser_harness | 5/7 | 5/5 | ~26.2s |
| **A8** | Luna planner + JEV + ultra-instinct + Browser Use | **7/7** | 5/5 | ~33.6s |

- **Accuracy:** A8 wins (7/7 vs 5/7). A3 missed both type tasks
  (`am-compose-send`, `am-reply`) on `browser_harness` `_IPCResponseTimeout`
  (daemon flakiness), not Luna HTTP failure.
- **Speed:** Mixed. A8 presses are often ~10–15s; `am-tm-print` safe-refusal
  (~103s) skews A8 mean. Excluding that outlier, A8 press walls are competitive
  or faster than A3.
- **Harness:** This PR bounds Mac `restart_daemon` hangs so completed attempts
  are not dropped (`bench/run.py` 60s best-effort; A3 finally 15s).



Branch: https://github.com/hexuria/ultra-instinct/tree/bench/uniform. Merged runs: `20261005-131001`, `20261005-141625`, `20261005-153648`. Harness: `bench/` (see `bench/README.md`).

**n = 1 run per arm per task** (up to 3 fresh tries, stopping at the first pass). This is a side-by-side comparison, not statistics: no confidence intervals or significance tests are claimed, and one flipped task moves an arm by several points.

## Leaderboard (all scenarios)

| Rank | Arm | First-try accuracy | Pass within 3 | Press-only tasks, first try | Wrong actions (first try / all tries) | Safe refusals (target-missing) | cap_hit | stuck | crashed | Steps | Model calls | Tokens (first / all) | Wall first-try s | Wall total s | Wall p50 s (first try) | Untrusted clicks |
|---:|---|---|---|---|---|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| 1 | **A8** Luna + JEV + ultra-instinct + Browser Use | 91% (21/23) | 100% (23/23) | 94% (17/18) | 0 / 0 | 3/4 | 0 | 0 | 1 | 73 | 146 | 325,991 / 373,825 | 477.9 | 539.7 | 21.1 | 42 |
| 2 | **A1** Luna + Browser Use | 91% (21/23) | 100% (23/23) | 94% (17/18) | 0 / 0 | 4/4 | 0 | 0 | 0 | 59 | 100 | 804,749 / 899,877 | 770.0 | 861.5 | 35.8 | 5 |
| 3 | **A2** Luna + CUA driver | 83% (19/23) | 91% (21/23) | 94% (17/18) | 0 / 0 | 4/4 | 0 | 0 | 0 | 97 | 203 | 521,358 / 762,828 | 1177.3 | 1651.3 | 48.3 | 36 |
| 4 | **A7** Luna + JEV + ultra-instinct + CUA | 78% (18/23) | 87% (20/23) | 83% (15/18) | 0 / 0 | 3/4 | 0 | 0 | 0 | 83 | 170 | 296,517 / 486,968 | 827.9 | 1218.1 | 33.5 | 36 |
| 5 | **A4** CUA jev-use (generic task) | 70% (16/23) | 70% (16/23) | 72% (13/18) | 4 / 13 | 4/4 | 0 | 8 | 0 | 103 | 136 | 202,208 / 337,116 | 686.7 | 1148.3 | 24.6 | 48 |
| 6 | **A6** Luna + ultra-instinct (MCP) | 43% (10/23) | 61% (14/23) | 56% (10/18) | 0 / 0 | 4/4 | 0 | 0 | 0 | 50 | 290 | 693,085 / 1,838,115 | 505.1 | 1159.8 | 17.0 | 29 |
| 7 | **A3** jev-ultrafast (BU + JEV) | 39% (9/23) | 48% (11/23) | 44% (8/18) | 1 / 1 | 2/4 | 4 | 0 | 21 | 87 | 228 | 454,033 / 1,004,831 | 400.4 | 872.1 | 17.8 | 0 |
| 8 | **A5** JEV + ultra-instinct (live_drive) | 30% (7/23) | 30% (7/23) | 39% (7/18) | 1 / 2 | 3/4 | 21 | 3 | 0 | 45 | 1403 | 2,961,164 / 8,676,362 | 209.0 | 580.8 | 8.6 | 30 |

Scoring. A task passes strictly when every success predicate holds in the page's own journal (events and
state the page recorded, never the agent's claims), no forbidden event fired, and the attempt did not end in
`cap_hit` or `stuck`. Target-missing tasks also need the arm to end with a refusal (give up / abstain / blocked by
choice). An arm that crashes after leaving the page in the right state still passes. Ranking is by first-try
accuracy; ties break on lower total first-try wall time. "Press-only" is the subset of tasks with no typing or select,
which is all ultra-instinct's `act` can do today (press/click). "Untrusted clicks" counts page clicks that were
synthetic DOM events (`isTrusted=false`), which can reach controls a person could not click (for example under a modal).

## Per scenario

- [acme-mail](examples/acme-mail/RESULTS.md): A1 7/7, A2 7/7, A3 3/7, A4 6/7, A5 3/7, A6 4/7, A7 6/7, A8 7/7
- [shop-checkout](examples/shop-checkout/RESULTS.md): A1 4/4, A2 3/4, A3 3/4, A4 3/4, A5 2/4, A6 2/4, A7 4/4, A8 4/4
- [admin-table](examples/admin-table/RESULTS.md): A1 3/3, A2 3/3, A3 0/3, A4 1/3, A5 1/3, A6 1/3, A7 1/3, A8 2/3
- [booking-calendar](examples/booking-calendar/RESULTS.md): A1 3/3, A2 1/3, A3 3/3, A4 2/3, A5 0/3, A6 0/3, A7 2/3, A8 3/3
- [hard-dom](examples/hard-dom/RESULTS.md): A1 3/4, A2 4/4, A3 0/4, A4 3/4, A5 1/4, A6 2/4, A7 4/4, A8 4/4
- [travel-search](examples/travel-search/RESULTS.md): A1 1/2, A2 1/2, A3 0/2, A4 1/2, A5 0/2, A6 1/2, A7 1/2, A8 1/2

## Per task, all scenarios

| Task | Class | Needs | A1 | A2 | A3 | A4 | A5 | A6 | A7 | A8 |
|---|---|---|---|---|---|---|---|---|---|---|
| `am-compose-send` | normal | type | PASS | PASS | fail(crashed) → fail(crashed) → fail(crashed) | PASS | fail → fail → fail | fail → fail → fail | PASS | PASS |
| `am-reply` | normal | type | PASS | PASS | fail(crashed) → fail(crashed) → fail(crashed) | PASS | fail → fail → fail | fail → fail → fail | PASS | PASS |
| `am-twin-send` | normal | press | PASS | PASS | fail(steps) → fail(steps) → fail(steps) | PASS | fail → fail(model_calls) → fail | PASS | PASS | PASS |
| `am-enabled-save` | normal | press | PASS | PASS | PASS | PASS | PASS | PASS | fail → PASS | PASS |
| `am-archive` | normal | press | PASS | PASS | PASS | PASS | PASS | PASS | PASS | PASS |
| `am-star` | normal | press | PASS | PASS | fail → PASS | fail → fail → fail | fail → fail → fail | fail → fail → PASS | PASS | PASS |
| `am-tm-print` | target-missing | press | PASS | PASS | PASS | PASS | PASS | PASS | PASS | PASS |
| `shop-add-qty` | normal | press | PASS | PASS | PASS | fail → fail → fail | fail(model_calls) → fail(model_calls) → fail(model_calls) | fail → fail → PASS | PASS | PASS |
| `shop-checkout` | normal | type | PASS | fail → PASS | fail(crashed) → fail(crashed) → fail(crashed) | PASS | fail → fail → fail | fail → fail → fail | PASS | PASS |
| `shop-noop-shipping` | no-op | press | PASS | PASS | PASS | PASS | PASS | PASS | PASS | PASS |
| `shop-tm-coupon` | target-missing | press | PASS | PASS | PASS | PASS | PASS | PASS | PASS | PASS |
| `admin-suspend` | normal | press | PASS | PASS | fail(crashed) → fail(crashed) → fail(crashed) | fail → fail → fail | fail → fail → fail(model_calls) | fail → fail → fail | PASS | PASS |
| `admin-filter-page` | normal | press | PASS | PASS | fail → fail → fail | fail(stuck) → fail(stuck) → fail | fail(model_calls) → fail(model_calls) → fail(model_calls) | fail → fail → PASS | fail → fail → fail | PASS |
| `admin-tm-delete` | target-missing | press | PASS | PASS | fail(crashed) → PASS | PASS | PASS | PASS | fail → PASS | fail → fail → PASS |
| `book-slot` | normal | press | PASS | fail → PASS | PASS | PASS | fail(model_calls) → fail(model_calls) → fail(model_calls) | fail → fail → fail | PASS | PASS |
| `book-timezone` | normal | select | PASS | fail → fail → fail | PASS | fail(stuck) → fail(stuck) → fail(stuck) | fail → fail → fail | fail → fail → fail | fail → fail → fail | PASS |
| `book-noop` | no-op | press | PASS | PASS | PASS | PASS | fail → fail(model_calls) → fail | fail → fail → fail | PASS | PASS |
| `hd-star` | normal | press | PASS | PASS | fail(model_calls) → fail(crashed) → fail(crashed) | fail → fail → fail | fail(model_calls) → fail(model_calls) → fail(model_calls) | fail → fail → PASS | PASS | PASS |
| `hd-shadow` | normal | press | PASS | PASS | fail → fail → fail | PASS | fail(stuck) → fail(stuck) → fail(stuck) | PASS | PASS | PASS |
| `hd-iframe` | normal | press | fail → PASS | PASS | fail → fail → fail | PASS | fail → fail → fail | fail → fail → fail | PASS | PASS |
| `hd-overlay` | normal | press | PASS | PASS | fail → fail → fail | PASS | PASS | PASS | PASS | PASS |
| `travel-lisbon` | normal | type, select | fail → fail → PASS | fail → fail → fail | fail(crashed) → fail(crashed) → fail(crashed) | fail(stuck) → fail(stuck) → fail(stuck) | fail(model_calls) → fail(model_calls) → fail(model_calls) | fail → fail → fail | fail → fail → fail | fail → PASS |
| `travel-tm` | target-missing | press | PASS | PASS | fail(crashed) → fail(crashed) → fail(crashed) | PASS | fail(model_calls) → fail(model_calls) → fail(model_calls) | PASS | PASS | PASS |

## Manifest

| Item | Value |
|---|---|
| Report id | `20261005-131001+20261005-141625+20261005-153648` |
| Run `20261005-131001` started | 2026-10-05 13:10:04 +0800 |
| Run `20261005-131001` bench HEAD | `456b9c062a06` |
| Run `20261005-131001` ultra-instinct merged | `4ae30d3` |
| Run `20261005-131001` TYPESAFE_API_KEY present | False |
| Run `20261005-141625` started | 2026-10-05 14:16:28 +0800 |
| Run `20261005-141625` bench HEAD | `984ac2c6a515` |
| Run `20261005-141625` ultra-instinct merged | `4ae30d3` |
| Run `20261005-141625` TYPESAFE_API_KEY present | True |
| Run `20261005-153648` started | 2026-10-05 15:36:51 +0800 |
| Run `20261005-153648` bench HEAD | `570d00f0368f` (dirty) |
| Run `20261005-153648` ultra-instinct merged | `4ae30d3` |
| Run `20261005-153648` TYPESAFE_API_KEY present | True |
| jev-ultrafast pin | `1231850a0bf1` |
| cua (jev-use) pin | `9ccafc981412` |
| Chrome | Chrome/154.0.8037.98 (headless for harness-launched arms; CUA arms use a driver-launched headed window sized to the same viewport) |
| cua-driver | cua-driver 0.23.2 |
| browser-use | 0.13.10 |
| Models | Luna `gpt-6-luna` (reasoning effort low) via OpenCodex; JEV `jev-latest` |
| Caps per attempt | 20 steps, 40 model calls, 180 s wall, stuck after 3 identical no-change actions, 40 page acts backstop |
| Viewport | 1280×800 |
| Seed / reps / max tries | 42 / 1 / 3 |
| Config hash | `c7242d275cf8` |

## Honesty notes

- Luna pass `20261005-131001` ran A1/A2/A6; A3/A4/A5 were skipped there (`TYPESAFE_API_KEY not available`).
- JEV pass `20261005-141625` ran A3/A4/A5 with the key present. This report merges both.
- ultra-instinct arms (A5, A6) can only press/click today. Tasks that need type/select show under Needs and in the press-only column; unmet body/form fields are expected failures for those arms until typing lands.
- A3 (jev-ultrafast) recorded many `crashed` outcomes in this pass; treat those as harness/arm failures, not page successes.
- Combined arms A7 (Luna + JEV + ultra-instinct + CUA) and A8 (Luna + JEV + ultra-instinct + Browser Use) ran in their own pass with the key present; protocol and limits in `bench/arms/COMBO.md`. A1 to A6 were not re-run.
- A7 cannot set a `<select>`: cua-driver 0.23.2 has no select tool and refuses its trusted input route on the background window (A2 hit the same wall). A7's ~15 s browser launch and sizing counts toward its wall time, as for A2 and A4.
- Untrusted clicks in A7/A8 come from ultra-instinct's `dom-semantic` press (same as A5/A6) and A7's CUA `dom_event` fallback; Browser Use clicks in A8 go through CDP mouse input.
