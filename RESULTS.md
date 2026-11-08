# ultra-instinct results

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

Pre-ticket A1–A8 bench tables and remasures (historical, not evidence for the current product): [`bench/results/HISTORICAL-A1-A8.md`](bench/results/HISTORICAL-A1-A8.md).

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
remote). The historical A1–A8 tables ([archive](bench/results/HISTORICAL-A1-A8.md))
are pre-pivot and not agent+Instinct evidence.
