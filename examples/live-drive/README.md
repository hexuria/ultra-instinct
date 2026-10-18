# Live drive (manual, opt-in)

A hand-run check that drives a real Chrome tab through `ultra-instinct mcp`, with
JEV (typesafe.ai) choosing each next tool and its arguments. It is not part of
CI and it is not a benchmark: seven or eight tasks on one static page say
nothing statistical. Use it to read transcripts and find bugs.

## What is here

- The page now lives at `examples/acme-mail/site/` (it is also the bench's
  `acme-mail` scenario): a static, Gmail-style test page ("Acme Mail", inline SVG, no
  network). `index.html` has a hash-routed inbox list (`#inbox`) and thread
  view (`#thread/<id>`), a docked Compose sheet with "Add Cc", and a quick
  reply box whose "Send" twins the Compose "Send". The thread toolbar has a
  hidden and an offscreen "Archive" twin. `settings.html` has a disabled and
  an enabled "Save". `help.html` is the link target that changes URL and title.
- `crates/ultra-instinct/examples/live_drive.rs`: the harness. It needs the
  `jev` feature (add `hgra` when remasuring with `ULTRA_INSTINCT_MATCHER=hgra`).
  Thread tasks (`t6`–`t8`) start already on `#thread/q3` (blank tab via
  `/json/new`, then `Page.navigate` to the full URL so the hash and query
  survive). `t8` also uses `?preset=twin` so Compose and quick-reply `Send`
  are both visible.

## Tools

Product tools are **observe**, **guard**, and **verify**. Deprecated `act` is
not offered.

- `guard` decides Allow / Refuse / Escalate and **never clicks**.
- After Allow, the harness inspects the allowed region for its rectangle and
  presses the center through its own CDP connection (`Input.dispatchMouseEvent`).
- After Refuse, the harness journals the reason and does not click.

## Run

```sh
# serves examples/ (pages load /_shared/bench.js); the site base is /acme-mail/site
python3 bench/server.py --port 8765 &
"/Applications/Google Chrome.app/Contents/MacOS/Google Chrome" \
  --remote-debugging-port=9333 --remote-debugging-address=127.0.0.1 \
  --user-data-dir=/tmp/ultra-instinct-live/chrome-profile --window-size=1280,800 &
cargo build --release -p ultra-instinct --features "jev,hgra" --bin ultra-instinct
cargo build --release -p ultra-instinct --features "jev,hgra" --example live_drive
set -a && source /path/to/bench/.env && set +a   # TYPESAFE_API_KEY; never echo
ULTRA_INSTINCT_JEV=1 \
  ./target/release/examples/live_drive --bin target/release/ultra-instinct \
    --site http://127.0.0.1:8765/acme-mail/site
```

Flags: `--site` (default `http://127.0.0.1:8765`; pass `/acme-mail/site` as above), `--cdp` (default
`http://127.0.0.1:9333`), `--out` (default `/tmp/ultra-instinct-live`), `--only
<task id>`, `--screenshot-only` (inbox and thread at 1280 and 1440 wide,
Compose open, settings, help).

Set `ULTRA_INSTINCT_MATCHER=hgra` (and build `ultra-instinct` / `live_drive` with `--features hgra`) to force the experimental HGRA ranker through MCP; default remains weighted.

Each task gets a fresh tab and a fresh `ultra-instinct mcp` child attached to that
tab's page websocket. The harness writes one JSONL transcript per task to
`<out>/transcripts/` and a `summary.json`, and records page ground truth
(URL, title, snackbar text, Compose/thread/Cc visibility) read straight from
the page after the run.

## What JEV is shown

The state JEV answers from includes the observed regions (with their
`state`), the last locate's top three candidates and its `signals`, the last
guard decision, and a history of call summaries. When the last locate carried
`repeated_query`, the position options name the candidate each suggested
position would pick. The option order does not change, and the harness does
not pick for JEV.

After **two consecutive identical ambiguous** guard refuses (same twin pair or
same text/role/proposed), the harness injects `within` / `near` choices into
the next locate/guard prompts, preferring a front_layer Compose / "New
Message" container. The gate still refuses until the query is scoped — this
is a harness nudge only, not a product change.

## Built-in Acme tasks

| id | start | notes |
| --- | --- | --- |
| `t1-compose-send` | `index.html` | Compose → Send |
| `t2-nav-settings` | `index.html` | left nav Settings |
| `t3-reveal-cc` | `index.html` | Compose → Add Cc |
| `t4-enabled-save` | `settings.html` | enabled Save |
| `t5-help-link` | `index.html` | top-bar Help |
| `t6-thread-archive` | `index.html#thread/q3` | toolbar Archive (thread already open) |
| `t7-thread-reply` | `index.html#thread/q3` | quick-reply Send |
| `t8-twin-send` | `index.html?preset=twin#thread/q3` | twin Send probe |

## Limits

- The caller is the harness plus JEV, not a full agent. JEV only picks from
  options the harness offers: strings quoted in the task and labels from the
  last observe. A run stops after 14 steps.
- The test page is ours, so it can be tuned to pass. Read failures as hints,
  not as rates.
- Use a throwaway Chrome profile bound to 127.0.0.1. Shut it down afterwards.

## Bench mode

The uniform benchmark (`bench/`, arm A5) runs this harness once per task with
`--task-json <file>` (`{id, start_url, text}`; `start_url` must be on 127.0.0.1),
`--trace <path>` (every log event streamed as JSONL, ending in
`{"kind":"final","outcome":"done|give_up|max-steps|jev-error"}`) and
`--max-steps N`. The bench's page journal, not this harness, decides pass or fail.
See `bench/README.md` and `RESULTS.md`.
