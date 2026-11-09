<p align="center">
  <a href="https://hdqwalls.com/wallpaper/3440x1440/goku-jiren-masterd-ultra-instinct">
    <img
      src="https://github.com/user-attachments/assets/5d173f2a-a61f-4069-ae76-66d0be44ece9"
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
is unstable until 1.0.

Crates are `ultra-instinct-*` (Autonomous Ultra Instinct, e.g. `ultra-instinct-agent`,
`ultra-instinct-browser`). The binary is `ultra-instinct`, with `aui` as a short
alias for the same CLI.

## Install

```bash
scripts/install-aui.sh   # idempotent; installs aui + ultra-instinct to ~/.cargo/bin
```

Agents: `skills/ultra-instinct/SKILL.md` turns a plain-language goal into one
`aui run` call — install, CDP check, policy pick, and run are all covered there.

### Claude Code: `screen-driver` subagent

[`agents/screen-driver.md`](agents/screen-driver.md) is the Claude Code subagent
from the [benchmark](#benchmark-browser-tools-for-an-llm-agent): it starts
`aui mcp` for itself, drives a private headless Chrome (never your own), clicks
by element index, and checks visuals from `browser_screenshot` images. It needs
`aui` on `PATH` and Chrome or Chromium installed.

```bash
scripts/install-aui.sh
mkdir -p ~/.claude/agents && cp agents/screen-driver.md ~/.claude/agents/
```

Copy it to `.claude/agents/` instead to scope it to one project. Claude Code
reads agent files and their MCP servers when a session starts, so restart the
session after installing it or after rebuilding `aui`. Then delegate page checks
to it, for example: "use screen-driver to check the checkout page in dark mode".

## Run the agent

```bash
# Live: attach to a Chrome started with --remote-debugging-port
cargo run -p ultra-instinct -- run --cdp http://127.0.0.1:9222 \
  --url https://example.com --goal 'Type "rust" into Search'

# Offline: predict-only dry run on a manifold (a --fixture *.cdp.json replay
# runs the full loop, but must script every observe/input/observe CDP call)
cargo run -p ultra-instinct -- run --goal "Delete" --fixture fixtures/modal-confirm.manifold
# Full offline agent loop (type / click / select CDP replays)
cargo run -p ultra-instinct -- run --goal 'Type "rust" into Search' \
  --fixture fixtures/agent-type-search.cdp.json
cargo run -p ultra-instinct -- run --goal "Click Go" \
  --fixture fixtures/agent-click-go.cdp.json
cargo run -p ultra-instinct -- run --goal 'Select "Business" in Cabin class' \
  --fixture fixtures/agent-select-cabin.cdp.json
```

Each `ultra-instinct run --cdp ... --url ...` opens its own background tab and
closes it when the run ends, so multiple agents can run against one Chrome.
`--cdp` without `--url` continues to drive the first existing tab.

Optional model payloads (feature `model-text`, ADR 0006): Instinct still picks the
target; a model only extracts a TYPE_TEXT / SELECT value that must occur in the
goal, else deterministic fallback, else abstain. Tests use scripted models.

```bash
cargo run -p ultra-instinct --features model-text -- run --cdp http://127.0.0.1:9222 \
  --goal "type rust ownership in the Search box" --text-model-cmd ./my-text-model.sh
```

Without `--text-model-cmd`, `TEXT_MODEL_API_KEY` turns on the built-in HTTP model.
`TEXT_MODEL_BASE_URL`, `TEXT_MODEL`, and `TEXT_MODEL_API` (`openai` | `anthropic` |
`gemini`; unset, the base URL host picks it) select the provider wire shape:

```bash
TEXT_MODEL_API=anthropic TEXT_MODEL=claude-haiku-5-5 TEXT_MODEL_API_KEY=... \
  cargo run -p ultra-instinct --features model-text -- run --cdp http://127.0.0.1:9222 \
  --goal "type rust ownership in the Search box"
```

```rust
let mut agent = AgentBuilder::new(BrowserSession::new(transport), InstinctPolicy::default())
    .max_steps(20)
    .build(r#"Select "Business" in Cabin class"#);
let outcome = agent.run(); // Done | Blocked | Abstained | Failed, with verified steps
```

### Chrome

`aui doctor` checks what `aui` needs: the Chrome it would launch, whether
`--cdp` answers, a test launch (skipped by `--quick`), and the `$TMPDIR/aui`
socket dir. `aui install` prints
`npx @puppeteer/browsers install chrome@stable --path ~/.cache/ultra-instinct/chrome`
(Chrome for Testing); `aui install --run` runs it, then `doctor`. Set
`ULTRA_INSTINCT_CHROME=<path>` to pick a binary. `run --launch [--headed]`
starts a headless (or headed) Chrome for the run when `--cdp` is not given,
and kills it at exit:

```bash
aui run --launch --url https://example.com --goal 'Type "rust" into Search'
```

### Reuse a login

`--save-state <path>` writes the run's cookies and local / session storage
when it ends (a 0600 file; the run prints counts, never values).
`--load-state <path>` sets them before the first observe:

```bash
aui run --launch --url https://shop.example/login --goal "..." --save-state shop.json
aui run --launch --url https://shop.example/orders --goal "..." --load-state shop.json
```

Storage is written for the origin the tab is on; other origins are reported
as pending.

### Policy flags

`--allow-domain shop.example` (or `*.shop.example`, repeatable) allows input
only on those hosts, and `--url` must match. `--confirm-press "Pay*"`
(repeatable glob) makes a matching click end `blocked` with
`rerun with --confirm "<label>"`; only the user's `--confirm "<label>"` lets
it through. MCP reads the same lists from `ULTRA_INSTINCT_ALLOW_DOMAINS` /
`ULTRA_INSTINCT_CONFIRM_PRESS`, and press tools take a `confirm` argument.

## Drive from a shell

The shell verbs drive one browser, one step at a time, through a small
session daemon (`$TMPDIR/aui/<session>.sock`). Every press still goes
through the ticketed guard.

```bash
aui open --launch https://shop.example/join   # daemon + its own Chrome (or --cdp <url>)
aui snap                                      # @e1 textbox "Email"  value="" …
aui type @e2 "kid@example.com"
aui click @e5                                 # ✓ ticket 41 · navigation
aui wait --gone "Loading" --timeout 10
aui close                                     # stops the daemon and its Chrome
```

`@eN` refs come from the latest `aui snap` (`N` works too). Other verbs:
`select`, `key`, `scroll`, `shot [--annotate]`, `tabs`, `dialog`, `forward`,
`reload`, `check` / `uncheck`, `focus`, `keyboard`, `get`, `is`, `count`,
`state save|load`, `cookies`, `storage`, `set viewport|device|media|offline|headers|reset`,
`download`, `network block|unblock|har`. A refusal or error exits non-zero.
`aui batch "snap" "click @e5" --bail` runs several verbs in order on one
session (`--json` for `[{cmd, ok, output}]`; no args reads a JSON array on
stdin). `--session <name>` picks the browser; `aui session` prints the
current one and `aui session list` the live ones. The daemon (and a
`--launch` Chrome) exits after 60 minutes without a verb; `aui open --idle
<secs>` sets that for the session (1 s to 24 h). `aui skills get core` prints
the full guide.

## MCP tools (optional adapter)

`observe`, `guard`, `verify` for hosts that keep their own executor. `guard`
finds its target by exact label (and role / `within`), unique or refuse, runs
the hard gate, and returns an `ActionTicket`; it never clicks. The locate,
inspect, act and diff helpers and the matcher are removed
([ADR 0012](docs/adr/0012-borrow-and-prune.md)).

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
cargo run -p ultra-instinct -- run --goal "Delete" --fixture fixtures/modal-confirm.manifold
# Full offline agent loop (type / click / select CDP replays)
cargo run -p ultra-instinct -- run --goal 'Type "rust" into Search' \
  --fixture fixtures/agent-type-search.cdp.json
cargo run -p ultra-instinct -- run --goal "Click Go" \
  --fixture fixtures/agent-click-go.cdp.json
cargo run -p ultra-instinct -- run --goal 'Select "Business" in Cabin class' \
  --fixture fixtures/agent-select-cabin.cdp.json
cargo run -p ultra-instinct -- observe --fixture fixtures/sign-in.cdp.json
cargo run -p ultra-instinct -- guard \
  --fixture fixtures/sign-in.cdp.json \
  --action click --target "Sign in" --role button --json
cargo run -p ultra-instinct -- verify \
  --fixture fixtures/welcome.cdp.json --expect-text Welcome
```

See `docs/PRD.md`, `docs/DECISIONS.md`, and `docs/EVAL.md`. Historical A1–A8 benchmark evidence that motivated
this pivot lives on the `bench/uniform` branch (`RESULTS.md`, PR #2).
