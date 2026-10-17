# Handover: third-party race, ultra-instinct vs jev-ultrafast

Written 2026-10-17 for an agent picking this up on a fresh Linux machine.
Branch `bench/race-2026-10-17` of `hexuria/ultra-instinct`, cut from main `2078a41`.

## Goal

Measure ultra-instinct (`aui run --policy jev`) against
[browser-use/jev-ultrafast](https://github.com/browser-use/jev-ultrafast) on benchmarks
**neither project wrote**. The owner rejected the repo's own example-site race as biased
toward whoever built the pages. Publish the result whatever it shows: the previous
in-repo race had jev-ultrafast ahead, 48/69 vs 21/69 (`bench/results/RACE-A3q-A9-A10.md`).

Two phases:

1. **MiniWoB++.** The harness is built and smoke-tested. **Your job: run it, report it.**
2. **Online-Mind2Web live sites.** This phase is not built yet. **Your job: build it, run it, report it.**

Then write both up and open a PR. The owner merges it, so don't merge it yourself.

## Fixed decisions (owner's choices; don't change)

| What | Decision |
|---|---|
| Arms | **A3m** = jev-ultrafast, unmodified vendor code. **A10m** = `aui run --policy jev`. |
| Decision model | JEV `jev-latest` (TypeSafe) for both. |
| Text model | **Claude Haiku 5.5 (`claude-haiku-5-5`) via `ANTHROPIC_API_KEY`, for both arms. Use no other text model.** |
| Text path | Both arms call the harness text proxy (`bench/text_proxy.py`). It turns OpenAI chat requests into native Anthropic Messages calls. The proxy exists because jev-ultrafast only speaks OpenAI format, and it always sends `response_format: json_object` plus a `reasoning` object. Anthropic's OpenAI-compatible endpoint rejects `json_object`, and Groq and Gemini reject `reasoning`. |
| Scoring | The benchmark's own success signal. Neither arm's code judges success. |
| Conditions | Same goal text, seeds, caps (20 steps, 40 model calls, 180 s wall), and 1280×800 headless Chrome. Each attempt gets a fresh profile. Arm order is shuffled per (task, seed). |

## Rules that keep it fair

- Don't edit `bench/vendor/jev-ultrafast`. Bench patches go through monkeypatches in `bench/arms/a3_jev_ultrafast.py` only, as they do today.
- Don't tune `aui` code against MiniWoB or Mind2Web tasks before or during the run. That would be benchmaxing, the exact thing this race exists to avoid. Run the `aui` built from main `2078a41`.
- Make no selective retries: `max_tries = 1`, and every attempt counts. If infrastructure breaks mid-run (the API is down, the machine dies), resume the run. Don't rerun the attempts you didn't like.
- Report crashes and timeouts as failures, and break them out in their own column.
- Any harness change that touches one arm must touch the other the same way. Note it in the write-up.

## 1. Machine setup (Linux)

Install:

- Rust (the repo pins `1.99.0` via `rust-toolchain.toml`; rustup picks it up)
- [`uv`](https://docs.astral.sh/uv/)
- `git`
- Google Chrome stable (`google-chrome-stable`)

```sh
git clone https://github.com/hexuria/ultra-instinct && cd ultra-instinct
git checkout bench/race-2026-10-17

# Vendor pins (gitignored, so clone them yourself at exactly these commits)
git clone https://github.com/browser-use/jev-ultrafast bench/vendor/jev-ultrafast
git -C bench/vendor/jev-ultrafast checkout 1231850a0bf1a0c0341fe408ef1668dbbfdfac46
git clone https://github.com/Farama-Foundation/miniwob-plusplus bench/vendor/miniwob-plusplus
git -C bench/vendor/miniwob-plusplus checkout 33c3b4ddef8c6eb67c57a29663d844b1eda7e614

# Python env for the harness and the jev-ultrafast arm
(cd bench && UV_PROJECT_ENVIRONMENT=.venv uv sync --python 3.12)

# aui with JEV and the HTTP text model; the runner refuses a binary without model-text
cargo build --release -p aui-cli --features jev,model-text --locked
```

The harness assumes macOS's Chrome path unless you set `BENCH_CHROME`. Put it and the
keys in `bench/.env`, which `bench/bench` sources; never commit that file:

```sh
BENCH_CHROME=/usr/bin/google-chrome-stable
TYPESAFE_API_KEY=...      # JEV decisions
ANTHROPIC_API_KEY=...     # Haiku 5.5 text, through the proxy only
HF_TOKEN=...              # phase 2 dataset download
```

If Chrome won't start as root in a container, add `--no-sandbox` to the launch args in
`bench/chrome.py`. That change affects both arms equally; note it in the write-up.

## 2. Preflight (about 5 min; do it before the long run)

```sh
bench/bench run --mock --arms mock-oracle,mock-saboteur
bench/bench miniwob --probe-only
bench/bench miniwob --tasks click-button,click-checkboxes,enter-text,click-link,login-user --seeds 1
```

- **Mock suite:** expect oracle 23/23. The saboteur passes only the 2 no-op tasks.
- **Probe-only:** this probes all 128 tasks × 3 seeds. Every task should get an utterance, with the same utterance for the same seed. `click-pie` is upstream-nondeterministic; it is kept and flagged.
- **Smoke, macOS reference (2026-10-17):** both arms PASS click-button, click-checkboxes and enter-text. Both fail click-link (JEV answered BLOCKED). login-user is flaky: A10m passed it once and got −1 once. Typing by hand over CDP scores +1 there, so the harness is fine.

If the smoke run looks very different from this (for example 0 passes for either arm), stop and debug Chrome, the keys or the proxy first. A broken setup must not end up looking like an agent result.

## 3. Phase 1: run MiniWoB++

There are 128 tasks × 3 seeds × 2 arms = 768 attempts. That takes about 3–5 h, and it
must outlive your shell, so run it in `tmux` or under `nohup`:

```sh
nohup bench/bench miniwob --config bench/race-miniwob.toml > miniwob-race.log 2>&1 &
# the run id is printed at the top and is the folder name under bench/runs/miniwob/
bench/bench miniwob --resume <run-id>          # after any interruption; finished attempts are kept
bench/bench miniwob-report <run-id> --publish  # writes bench/results/MINIWOB-<run-id>.{md,json}
```

- **Expected spend:** roughly 2,000–3,500 JEV calls and 300–800 Haiku calls per arm. The hard ceiling is 40 calls per attempt.
- **What the harness does** (see `bench/README.md`, section "MiniWoB++ race"):
  - It serves the pinned `miniwob/html` and injects one script after `core.js`.
  - The script calls `Math.seedrandom(seed)` from the URL, MiniWoB's own seeding.
  - It raises `core.EPISODE_MAX_TIME` to 180 s, auto-starts the episode via `core.startEpisodeReal()`, and hides the reward HUD.
  - It wraps `core.endEpisode` to POST the first episode's reward. Success means reward > 0.
  - The goal is the task's own `#query` utterance.

## 4. Caveats to state in the write-up

- **HUD hidden for both arms.** Its 1 Hz countdown made jev-ultrafast treat its page as stale after every text call, so it hit the call cap. MiniWoB's own agent observation leaves the HUD out too. Still, this harness choice **helps A3m**.
- **Timing.** Process wall time includes jev-ultrafast's Python startup and teardown (about 15 s), plus the 3 s end-of-episode grace. Time-to-reward is the fairer number. It includes A3m's own page load, while `aui` starts on the already-loaded page. Report both.
- **Prompts differ.** Both arms send the same model and settings, but each keeps its own text prompt.
- **Coarse stuck detection.** It is a DOM-change counter. The `page_acts` cap isn't enforced on MiniWoB.
- **`click-pie`** is nondeterministic upstream. Report it with and without that task.

## 5. Phase 2: Online-Mind2Web live sites (to build)

The dataset is [`osunlp/Online-Mind2Web`](https://huggingface.co/datasets/osunlp/Online-Mind2Web),
a single file `Online_Mind2Web.json` with 300 tasks on real sites. It is **gated**. As of
the handover, `HF_TOKEN` got a 403: the owner still had to click "Agree" on the dataset
page while signed in as the token's account. If you get a 403, ask the owner. Don't try
to get around the gate.

```sh
curl -L -H "Authorization: Bearer $HF_TOKEN" -o bench/vendor/online-mind2web.json \
  https://huggingface.co/datasets/osunlp/Online-Mind2Web/resolve/main/Online_Mind2Web.json
```

Build `bench/run_mind2web.py`, `bench/race-mind2web.toml` and `bench/report_mind2web.py`
alongside the MiniWoB files, reusing `chrome.py`, `proxy.py`, `text_proxy.py` and the same
A3m/A10m arm specs:

- **Sample.** Draw a fixed-seed random sample (seed 42) of about 50 tasks, stratified by the dataset's `level` (easy, medium, hard). Write the task ids to the manifest **before** any arm runs.
- **Safety filter, applied before sampling.** Drop tasks whose goal needs any of the following:
  - signing in or creating an account;
  - paying, booking or checking out;
  - posting, sending or submitting anything that changes a real site.

  The rule must be mechanical: a keyword list or a single fixed Haiku classification prompt. Publish the dropped ids. Also add a harness-side guard: stop the attempt as a fail if the page reaches a payment or confirm step.
- **Same start state.** Both arms start from the task's `website` URL, with the same goal text, caps and viewport, and with arms interleaved per task. Live sites drift, so run each task's two arms back to back.
- **Judge.** Online-Mind2Web's official judge is **WebJudge**: the prompts are in the [OSU-NLP-Group/Online-Mind2Web](https://github.com/OSU-NLP-Group/Online-Mind2Web) repo. Run it with **`claude-haiku-5-5`**, because the owner allows no other model, and say so: the paper reports agreement for o4-mini.
  - Feed the judge the identical inputs for both arms: the task, the action history, and key screenshots. Capture a screenshot after every step for both arms through CDP `Page.captureScreenshot` from the harness, not through either agent.
  - Hide the arm identity from the judge.
  - Have a human (the owner) spot-check about 10 verdicts.
- **Report.** Show success, success by level, steps, calls, time, and blocked or CAPTCHA'd attempts as their own column. Neither arm may solve CAPTCHAs. A CAPTCHA ends the attempt as `blocked`.

## 6. Write-up and PR

- Run `bench/bench miniwob-report <run-id> --publish`, and add the matching Mind2Web report under `bench/results/`.
- Add a section to `bench/results/RESULTS.md`, and a short line to `README.md` only if it is accurate. Follow the existing honest style:
  - pins manifest, and pass counts with per-task tables;
  - median in-loop time and process wall time;
  - crashes, false DONEs and caveats.
- Commit on this branch, push, and open a PR to `hexuria/ultra-instinct` main. Use a "Verification impact" block in the body per `AGENTS.md`; for a bench-only change, tick "No verification architecture impact". Wait for CI: rustfmt, clippy, cargo-deny, tests, rustdoc, repo rules, zizmor, ci-ok. Don't merge; the owner merges.

## State at handover

| Item | State |
|---|---|
| Commits on branch | `112e5de` text proxy, `2ef4514` harness, `0c15b04` probe fixes + README, `c5d6a23` hide HUD, plus this doc |
| Full MiniWoB run | **not run.** One started on macOS was stopped within minutes; ignore any partial `bench/runs/miniwob/*` (gitignored, not pushed). |
| Mind2Web | not built; dataset gated (see §5) |
| Pins | ultra-instinct `2078a41`, jev-ultrafast `1231850`, MiniWoB++ `33c3b4d`, `anthropic==0.76.0` (pinned by browser-use), typesafe-sdk 0.3.0 (Rust, via `aui`) |
