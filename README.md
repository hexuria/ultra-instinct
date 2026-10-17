<p align="center">
  <a href="https://hdqwalls.com/wallpaper/3440x1440/goku-jiren-masterd-ultra-instinct">
    <img
      src="https://images.hdqwalls.com/download/goku-jiren-masterd-ultra-instinct-2e-3440x1440.jpg"
      alt="Ultra Instinct Goku"
      width="100%"
    >
  </a>
</p>

# ultra-instinct

**Ultra-Instinct is a Rust-native browser-agent runtime with a deterministic
decision kernel (Instinct) and executor-bound action tickets.** It observes the
page (DOM + accessibility fusion, stable identity, stacking / hit-test
occlusion, modal front layer), builds a finite action space, lets Instinct choose
or abstain, gates the choice, and executes exactly the ticketed action — or
nothing — then verifies the effect.

```text
goal
 │
 ▼  observe (CDP) ─► ActionSpace (front layer applied; hidden/disabled/covered excluded)
 │
 ▼  policy: Instinct ─► abstain? ─► optional remote tier (feature `remote`, same finite menu)
 │
 ▼  TYPE_TEXT / SELECT payload: TextResolver (never Instinct)
 │
 ▼  hard gate ─► ActionTicket (action + target + world fingerprint)
 │
 ▼  executor: one-shot ledger → fresh observe → revalidate → gate → consume → input
 │
 ▼  observe → diff / value read-back → success | no-effect | wrong-effect | navigation
 │
 └─► history → next tick (DONE / BLOCKED / abstain / bounds)
```

No LLM and no MCP are required to run the loop. See `docs/PRD.md`,
`docs/adr/0001…0003`, and `docs/EVAL.md` for status and honest gaps.

Toolchain: Rust 1.99.0. Versions are 0.1.0 and `publish = false`. Public API
is unstable until 1.0. HGRA is an experiment under `experiments/hgra/` and is
not on any default path.

Crates are `aui-*` (Autonomous Ultra Instinct, e.g. `aui-agent`,
`aui-browser`). The binary is `ultra-instinct`, with `aui` as a short
alias for the same CLI.

## Install

```bash
scripts/install-aui.sh   # idempotent; installs aui + ultra-instinct to ~/.cargo/bin
```

Agents: `skills/ultra-instinct/SKILL.md` turns a plain-language goal into one
`aui run` call — install, CDP check, policy pick, and run are all covered there.

## Run the agent

```bash
# Live: attach to a Chrome started with --remote-debugging-port
cargo run -p aui-cli -- run --cdp http://127.0.0.1:9222 \
  --url https://example.com --goal 'Type "rust" into Search'

# Offline: predict-only dry run on a manifold (a --fixture *.cdp.json replay
# runs the full loop, but must script every observe/input/observe CDP call)
cargo run -p aui-cli -- run --goal "Delete" --fixture fixtures/modal-confirm.manifold
# Full offline agent loop (type / click / select CDP replays)
cargo run -p aui-cli -- run --goal 'Type "rust" into Search' \
  --fixture fixtures/agent-type-search.cdp.json
cargo run -p aui-cli -- run --goal "Click Go" \
  --fixture fixtures/agent-click-go.cdp.json
cargo run -p aui-cli -- run --goal 'Select "Business" in Cabin class' \
  --fixture fixtures/agent-select-cabin.cdp.json
```

Each `ultra-instinct run --cdp ... --url ...` opens its own background tab and
closes it when the run ends, so multiple agents can run against one Chrome.
`--cdp` without `--url` continues to drive the first existing tab.

Optional model payloads (feature `model-text`, ADR 0006): Instinct still picks the
target; a model only extracts a TYPE_TEXT / SELECT value that must occur in the
goal, else deterministic fallback, else abstain. Tests use scripted models.

```bash
cargo run -p aui-cli --features model-text -- run --cdp http://127.0.0.1:9222 \
  --goal "type rust ownership in the Search box" --text-model-cmd ./my-text-model.sh
```

Without `--text-model-cmd`, `TEXT_MODEL_API_KEY` turns on the built-in HTTP model.
`TEXT_MODEL_BASE_URL`, `TEXT_MODEL`, and `TEXT_MODEL_API` (`openai` | `anthropic` |
`gemini`; unset, the base URL host picks it) select the provider wire shape:

```bash
TEXT_MODEL_API=anthropic TEXT_MODEL=claude-haiku-5-5 TEXT_MODEL_API_KEY=... \
  cargo run -p aui-cli --features model-text -- run --cdp http://127.0.0.1:9222 \
  --goal "type rust ownership in the Search box"
```

```rust
let mut agent = AgentBuilder::new(BrowserSession::new(transport), InstinctPolicy::default())
    .max_steps(20)
    .build(r#"Select "Business" in Cabin class"#);
let outcome = agent.run(); // Done | Blocked | Abstained | Failed, with verified steps
```

## MCP tools (optional adapter)

`observe`, `guard`, `verify` for hosts that keep their own executor. `act` is a
deprecated alias of `guard` and never clicks. Locate, inspect, and diff remain
debug helpers.

## Benchmark: browser tools for an LLM agent

The same Claude Code subagent (Claude Haiku 5.5) checked and drove one page with
different browser tools, 2 runs each. With `ultra-instinct mcp` it was as
accurate as obscura + pageshot (10 / 10), used **30% fewer tokens**, ran **2.2×
faster**, and screenshotted every state, including after clicks. Small sample,
one site: see [`RESULTS.md`](RESULTS.md) and the
[writeup](bench/results/AGENT-MCP-COMPARE-2026-10-17.md) for method and caveats.

## Commands

```bash
cargo test --workspace
cargo run -p aui-cli -- run --goal "Delete" --fixture fixtures/modal-confirm.manifold
# Full offline agent loop (type / click / select CDP replays)
cargo run -p aui-cli -- run --goal 'Type "rust" into Search' \
  --fixture fixtures/agent-type-search.cdp.json
cargo run -p aui-cli -- run --goal "Click Go" \
  --fixture fixtures/agent-click-go.cdp.json
cargo run -p aui-cli -- run --goal 'Select "Business" in Cabin class' \
  --fixture fixtures/agent-select-cabin.cdp.json
cargo run -p aui-cli -- observe --fixture fixtures/sign-in.cdp.json
cargo run -p aui-cli -- guard \
  --fixture fixtures/sign-in.cdp.json \
  --action click --target "Sign in" --role button --json
cargo run -p aui-cli -- verify \
  --fixture fixtures/welcome.cdp.json --expect-text Welcome
```

See `docs/PRD.md`, `docs/DECISIONS.md`, and `docs/EVAL.md`. Historical A1–A8 benchmark evidence that motivated
this pivot lives on the `bench/uniform` branch (`RESULTS.md`, PR #2).
