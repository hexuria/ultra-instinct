# Uniform ultra-instinct benchmark

One harness, eight arms, six local scenario sites, the same Chrome, the same loopback
server, the same 1280×800 viewport, and the same caps for everyone. Per-scenario
results: `examples/<scenario>/RESULTS.md`. Earlier result files live in the private
stow repo.

**n = 1 per arm per task** (up to 3 fresh tries). It is a side-by-side comparison,
not statistics.

## Arms

Product-direction ablation (ActionTicket): see [`arms/B01.md`](arms/B01.md) (B0 Browser Use alone · B1 invisible guard ticket). A1–A8 below are the historical uniform suite. A5–A8 (and B0/B1) call the removed `locate` / `inspect` / `act` tools and cannot be re-run on the current build.


| Arm | Wiring | Model calls |
|---|---|---|
| A1 | Browser Use 0.13.10 `Agent` on the harness Chrome (`cdp_url`) | GPT 6 Luna (OpenCodex) |
| A2 | Luna tool loop over `cua-driver mcp` typed browser tools (`get_browser_state` semantic_v2, `browser_click`, `browser_type`, `browser_pointer`) on a driver-launched isolated Chrome | Luna |
| A3 | jev-ultrafast (browser-harness + JEV), monkeypatched: JEV URL to the proxy, 1120×780 override to 1280×800, text helper = Luna | JEV + Luna for text |
| A4 | Cua's jev-use recipe with a generic task (one candidate per actionable ref, quoted literals for typing, reobserve/abstain/done), no declared steps | JEV |
| A5 | ultra-instinct `live_drive` example in bench mode (`--task-json`, `--trace`, `--max-steps`) | JEV |
| A6 | Luna tool loop over `ultra-instinct mcp` (observe, locate, inspect, act, verify, diff; CDP endpoint injected) | Luna |
| A7 | Combined: Luna plans; ultra-instinct observes/locates/presses over CDP on CUA's own isolated Chrome; JEV breaks ranking ties (with row context, NONE option, ultra-instinct's act gate on its probabilities); CUA driver types, scrolls, reads, and does a gated fallback click ([`arms/COMBO.md`](arms/COMBO.md)) | Luna + JEV |
| A8 | Combined: same loop on the harness Chrome; Browser Use 0.13 `Tools` (no Browser Use agent) types, selects, scrolls, reads, and does the gated fallback click by element index ([`arms/COMBO.md`](arms/COMBO.md)) | Luna + JEV |

A6 shows a trimmed schema (same tools, only the knobs the default path needs, `position`
enum from the CLI help): with the shipped MCP schemas Luna filled every optional field
and hit `UnknownPosition`, `DimsRequireHgra`, `ConfidenceWithQuery`, `EmptyText`.
ultra-instinct `act` is press/click only, so tasks tagged `needs: [type]` or `[select]` are
out of reach for A5/A6; the leaderboard has a press-only column.

## Fairness rules

- Same goal text for every free-text arm, plus one shared suffix (`arms/common.py`):
  the page is open, use only this site, give up without changing anything if impossible.
- Every attempt starts fresh: new Chrome profile (or new isolated cua profile), journal
  and meter reset.
- Caps enforced from outside every arm (`bench.toml [caps]`): 20 steps (arm actions),
  40 model calls (the counting proxy answers 429 and flags the attempt), 180 s wall,
  `stuck` after 3 identical actions with no page-state change, 40 page acts backstop.
  At a cap the harness kills the arm's process group: `cap_hit` (fail).
- Ground truth is the page's own journal (`/_shared/bench.js` → `server.py`): events,
  state, and every click/fill/select/submit the page saw. Agents' claims are ignored,
  except that target-missing tasks need a refusal final status.
- Harness Chrome is headless (same binary): this Mac runs AeroSpace, a tiling window
  manager that resizes every new headed window. CUA arms need a native window, so they
  float only their throwaway window (`aerospace layout floating --window-id`, no config
  change) and size it until the page reports 1280×800.

## Run

```sh
cd bench
UV_PROJECT_ENVIRONMENT=.venv uv sync --python 3.12
(cd envs/cua && UV_PROJECT_ENVIRONMENT=.venv uv sync --python 3.12)
cargo build --release -p aui-cli && cargo build --release -p aui-cli --features jev --example live_drive
# vendor pins (gitignored): see the report's manifest
bench/bench run --mock --arms mock-oracle,mock-saboteur     # no keys: validates pages, checkers, harness
bench/bench run --config bench/bench.toml --seed 42         # the real run
bench/bench report                                           # RESULTS.md files
```

`TYPESAFE_API_KEY` (JEV arms A3–A5, A7, A8) is read from the environment or a gitignored
`bench/.env`; without it those arms are recorded as "not run". OpenCodex must be
listening on `127.0.0.1:8080`. Useful flags: `--arms`, `--tasks`, `--scenarios`,
`--max-tries`, `--reps`, `--resume <run-id>`.

## Mock arms

`mock-oracle` replays each task's `oracle` steps with trusted CDP input (passes all 23),
`mock-saboteur` replays `saboteur` steps (trips the forbidden event), `mock-spinner`
ends `stuck`, `mock-wanderer` ends `cap_hit` (steps), `mock-sleeper` ends `cap_hit` (wall).

## Task schema (`examples/<scenario>/tasks.yaml`)

`id`, `class` (normal, target-missing, no-op), `needs` ([], type, select), `start`
(path on the server), `goal` (quoted literals are the values to type), `success`
(`event`, `no_event`, `state` with dotted keys, `url_contains`, `final`), `forbidden`
(event matchers), `oracle` / `saboteur` steps (`click`, `type`, `select`, `key`, `seek`,
`eval`, `final`). Matchers: plain string = case-insensitive exact, `~x` contains, `!x`
present and not equal, a list = any of.

## Layout

`run.py` harness · `checker.py` scoring · `tasks.py` loader · `server.py` sites + journal ·
`proxy.py` counting model proxy · `chrome.py` throwaway Chrome · `arms/` wrappers ·
`report.py` writes RESULTS.md files · `probe.py` debug helper · `runs/` raw artifacts (gitignored) ·
`results/<run-id>.json` compact results.
