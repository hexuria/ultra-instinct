# Eval / parity status (agent + Instinct runtime)

Date: 2026-10-06 (Asia/Manila). See ADR 0002 and ADR 0003.

## Arms

| Arm | Meaning | Status |
|---|---|---|
| A | upstream `jev-ultrafast` (Python reference) | **not run** here |
| B | Ultra-Instinct + remote/Jev policy only | wire + transport trait (`remote` feature); no live model wired |
| C | Ultra-Instinct + Instinct only | **offline e2e (mock + CDP replay) + live Chrome smoke** |
| D | Ultra-Instinct + Instinct → escalation | **offline e2e** with `ScriptedRemote`; no live model |

No arm has multi-run, multi-site numbers yet. Do not quote any of this as
general performance evidence (n is tiny and the tasks are single-intent).

## How to run arm C offline (no Chrome, no network, no model)

```bash
# Everything CI runs
cargo test --workspace

# Just the owned loop
cargo test -p aui-agent                    # all agent tests
cargo test -p aui-agent --test adversarial # mock adversarial e2e (20)
cargo test -p aui-agent --test replay_cdp  # real BrowserSession over a CDP replay (7)
cargo test -p aui-agent --test props       # property tests (ticket substitution, stale, abstain)
cargo test -p aui-agent --test mock_loop   # original ADR 0002 loop tests
cargo test -p aui-guard gate               # hard gate unit tests
cargo test -p aui-policy                   # Instinct policy, evidence, resolver
```

From the CLI, a CDP replay script runs the full loop offline:

```bash
cargo run -p aui-cli -- run --goal 'Type "rust" into Search' \
  --fixture path/to/script.cdp.json            # ScriptBuilder JSON
cargo run -p aui-cli -- run --goal "Delete" \
  --fixture fixtures/modal-confirm.manifold    # static manifold: predict-only dry run
cargo run -p aui-cli -- run --goal 'Type "rust" into Search' \
  --fixture fixtures/agent-type-search.cdp.json
cargo run -p aui-cli -- run --goal 'Click Go' \
  --fixture fixtures/agent-click-go.cdp.json
cargo run -p aui-cli -- run --goal 'Select "Business" in Cabin class' \
  --fixture fixtures/agent-select-cabin.cdp.json
```

The dry run on `modal-confirm.manifold` predicts `CLICK:confirm-delete`: the
background `Delete project` is behind the modal and is not offered.

## Arm D offline (Instinct → remote)

```bash
cargo test -p aui-policy -p aui-agent \
  --features aui-agent/remote,aui-policy/remote
```

`remote_e2e.rs`: twins make Instinct abstain → scripted remote picks `CLICK:b` →
same gate / ticket / executor path → only `b` is pressed. Off-menu replies
(`CLICK:#delete-all`) fail without input. `UnconfiguredRemote` keeps the Instinct
abstain.

## Live Chrome smoke (arm C, manual)

```bash
"/Applications/Google Chrome.app/Contents/MacOS/Google Chrome" --headless=new \
  --remote-debugging-port=9333 '--remote-allow-origins=*' \
  --user-data-dir=/tmp/hu-smoke about:blank &
ULTRA_INSTINCT_CDP=http://127.0.0.1:9333 \
  cargo test -p aui-agent --test live_smoke -- --ignored --nocapture
```

Or by hand: `ultra-instinct run --cdp http://127.0.0.1:9333 --url <page> --goal …`.

Last run 2026-10-06 ~00:10 (Asia/Manila) (Chrome headless, macOS, inline `data:` page):

| Goal | Step | Verification | Outcome |
|---|---|---|---|
| `Type "rust ownership" into Search` | TYPE_TEXT `Search` | success (value read back) | done |
| `Select "Business" in Cabin class` | SELECT `Cabin class` | success (option read back) | done |
| `Click Go` | CLICK `Go` | state-changed (result text appeared) | done |
| `scroll down` | SCROLL_DOWN | state-changed | done |
| `Select "Premium" in Cabin class` (CLI) | — | page rejected: 0 matching options | failed, nothing changed |
| `scroll down` at page bottom (CLI) | SCROLL_DOWN ×3 | no-effect | blocked (bound; pre-ADR-0008 — today habituation abstains after 2 repeats) |

n = 1 per goal, one static page. It proves the paths work end to end on a real
browser; it measures nothing.

## Adversarial coverage (offline)

| Case | Where | Result asserted |
|---|---|---|
| twin buttons / twin rows | `adversarial`, `mock_loop`, `props` | abstain, no input |
| modal appears after prediction | `adversarial`, executor unit | stale, background never pressed |
| background behind open modal | `adversarial`, gate unit | not in action space; gate `front-layer` |
| covered (hit-test) control | `replay_cdp` | marked occluded, not offered, gate `occluded` |
| disabled / hidden / offscreen | `adversarial`, gate unit | never executed |
| rerender replaces node (new id) | `adversarial` | stale, no input |
| rerender relabels target (same id) | executor unit, `props` | stale, no input |
| cookie / toast added (world change) | `replay_cdp`, `props` | stale, no input |
| focus change between predict/act | `adversarial` | stale, no input |
| page changes during text resolution | `adversarial` | stale, nothing typed |
| readonly / rejected input | `adversarial`, `replay_cdp` | `InputRejected`, ticket consumed, nothing typed |
| unresolvable TYPE_TEXT value | `adversarial` | failed, nothing typed |
| `model-text`: goal-grounded model payload (type / select) | `model_text` (agent), policy unit | typed / selected via ticket, verified |
| `model-text`: invented / multiline / label-echo / empty / too-long reply | `model_text`, policy unit | refused; fallback or abstain, nothing typed |
| `model-text`: reply bound to stale context fingerprint | `model_text`, policy unit | refused; abstain, nothing typed |
| `model-text`: model outage | `model_text`, policy unit | deterministic fallback, else abstain |
| `model-text`: page moves during model call | `model_text` | ticket stale, nothing typed; model re-asked with new fingerprint |
| `model-text`: feature off / default builder | `model_text`, CLI unit | deterministic resolver; `--text-model-cmd` refused |
| wrong effect (page mangles value) | `adversarial` | `wrong-effect` ×2 → Abstained (ADR 0008 habituation freeze) |
| repeated no-effect click | `adversarial`, `mock_loop` | ×2 → Abstained (ADR 0008) |
| page never settles | `adversarial` | stale bound → failed, no input |
| off-menu / selector / kind-mismatch policy output | `adversarial`, `remote` unit, `remote_e2e` | hard error, no input |
| ticket reuse / operation swap | executor unit, `props` | refused |
| multi-step `then` / `and then` | `adversarial` | both clauses execute; connective limits documented |
| readonly observed before TYPE | `gate` unit, `adversarial` | hard refuse / no input |
| unrelated far banner vs nearby twin | `world` unit, `ticket`, `adversarial` | far OK; nearby/modal stale |

Covered offline (ADR 0007): same-origin iframe pierce, open shadow pierce,
ARIA combobox/option autocomplete (TYPE → re-observe → ticketed CLICK),
virtualized list visible window + scroll → re-observe → click.
Still not covered: cross-origin iframe, closed shadow, infinite-scroll
inventory, checkbox/radio "already satisfied", navigation between prediction
and execution on a live page.

## Honest gaps / out of scope

- Arms A/B/D have no live runs; no jev-ultrafast Wikipedia / travel parity
  (upstream unpaid path unavailable). Documented as blocked below.
- Multi-step is **only** plain `then` / `and then` outside quotes
  (`split_sequential_clauses`). No branching, conditionals, or LLM planner.
  Each clause remains one Instinct single-intent.
- `model-text` TextResolver ([ADR 0006](adr/0006-model-text-resolver.md)) is
  tested offline only, with `ScriptedTextModel` and a local `sh` command
  model. No live LLM numbers are claimed. Values must be grounded in the goal
  clause (extraction, not generation); SELECT is grounded in the goal, not in
  the page's option list (the manifold has no options yet). Default remains
  `DeterministicTextResolver` (quoted / `type X into` / `fill X with`).

### Enabling model-text (optional, live LLM not required)

```bash
# Offline tests (what CI runs; scripted models, no network, no keys)
cargo test -p aui-policy -p aui-agent -p aui-cli \
  --features aui-policy/model-text,aui-agent/model-text,aui-cli/model-text

# Live: plug any model in via a program you own (it holds its own API key)
cargo run -p aui-cli --features model-text -- run \
  --cdp http://127.0.0.1:9222 --goal "type rust ownership in the Search box" \
  --text-model-cmd ./my-text-model.sh
```

The program reads one JSON line
`{"goal","field_label","field_role","context_fingerprint","max_chars"}` on
stdin and prints `{"text":"…","context_fingerprint":<same number>}` or
`{"declined":"reason"}`. Library: `AgentBuilder::model_text(model)` with any
`TextModel`, or `.text_resolver(ModelTextResolver::new(m).without_fallback())`
to abstain instead of falling back.

Without the flag, `TEXT_MODEL_API_KEY` enables the built-in HTTP model
(`crates/aui-cli/src/http_text.rs`): OpenAI-compatible chat completions,
Anthropic Messages, or Gemini `generateContent`, chosen by `TEXT_MODEL_API` or
the `TEXT_MODEL_BASE_URL` host. Every wire shape is reduced to the same
`{"text","context_fingerprint"}` / `{"declined"}` reply before vetting. Wire
contracts are tested against a local server only
(`crates/aui-cli/tests/text_model_http.rs`); no live provider numbers are claimed.
- Harder page types (ADR 0007): same-origin iframe + open shadow pierced;
  autocomplete TYPE→option CLICK; virtualized visible window only.
  Still out: cross-origin iframe, closed shadow, off-window inventory.
- MCP `guard` still uses the float `0.55` / `0.05` ranking gate for host
  preflight (A5/A6 / combo benches). Agent path uses hard `gate` only.
  Allow tickets now share `gate::check` + target-scoped world fingerprints.
- RESULTS.md A1–A8 remain historical (pre-pivot); see the agent+Instinct section.

## Offline arm C repeats (n = 3)

Scripted: `cargo test -p aui-agent` three times on this tip. All three
runs green (same deterministic offline suite). See RESULTS.md § "Agent + Instinct
pivot (offline arm C)".

Arms A / B / D: **blocked / out of scope** here — need upstream
`jev-ultrafast` and/or a paid remote model. Offline D (`ScriptedRemote`) still
passes under `--features remote`.

## Instinct pin

`a42d16b6f5ccc3273939c8e3d3f462d78765bfea`
