# Embedding ultra-instinct over a CDP pipe

For a host that launches Chromium itself with `--remote-debugging-pipe` and
no DevTools port, and drives it from its own process (ADR 0013). There is no
stdio and no HTTP: the host hands the pipe ends to a `PipeBrowser`, wraps it
in an `ultra_instinct_mcp::Server`, and calls `Server::call_tool` with the
same `browser_*` tools the MCP server serves.

## Crates and features

| Crate | Why | Features |
|---|---|---|
| `ultra-instinct-mcp` | `Server`, the `browser_*` tools, `run_goal`, `ToolError`, `ToolLimits`, `Downloads`, `Policy` (re-exported) | none |
| `ultra-instinct-cdp` | `PipeBrowser` / `PipeLimits` (and `PipeBackend` if you build the server with `with_backend`) | none |
| `ultra-instinct-guard` | `Policy` if you prefer it from its home crate | none |

Unix only (`cfg(unix)`). No async runtime, HTTP client or model SDK is
linked: the whole path is blocking Rust.

## Minimal example

```rust,ignore
use std::time::{Duration, Instant};

use serde_json::json;
use ultra_instinct_cdp::{PipeBrowser, PipeLimits};
use ultra_instinct_mcp::{Downloads, Policy, RunLimits, Server, ToolLimits};

// `reader` is the end Chrome writes (its fd 4), `writer` the end it reads
// (its fd 3): File, OwnedFd, PipeReader / PipeWriter, ChildStdout / ChildStdin.
let browser = PipeBrowser::with_limits(
    reader,
    writer,
    PipeLimits { max_frame_bytes: 32 << 20, ..PipeLimits::default() },
);
let mut server = Server::with_pipe(browser)
    .require_snapshot(true)
    .with_limits(ToolLimits::default())
    .with_downloads(Downloads::Dir("/srv/box/downloads".into()))
    .with_tool_allowlist([
        "browser_navigate", "browser_get_state", "browser_click", "browser_type",
        "browser_select_dropdown", "browser_check", "browser_send_key",
        "browser_scroll", "browser_wait", "browser_screenshot", "browser_get_text",
        "browser_dialog", "browser_new_tab", "browser_list_tabs",
        "browser_switch_tab", "browser_close_tab",
    ]);
server.set_policy(Policy::from_lists("*.shop.example", "Pay*, Delete*")?);

server.call_tool("browser_navigate", &json!({"url": "https://shop.example/"}))?;
let state = server.call_tool("browser_get_state", &json!({"format": "json"}))?;
let snapshot = &state["snapshot"];
match server.call_tool("browser_click", &json!({"index": 3, "snapshot": snapshot})) {
    Ok(done) => println!("pressed {}", done["region"]),
    Err(err) if err.code() == "stale_ref" => { /* the page moved: read the state again */ }
    Err(err) if err.code() == "needs_confirm" => {
        // Ask the user; then the same call with `"confirm": <label>` (that call only).
    }
    Err(err) => return Err(err.into()),
}

// The agent loop, no model: Instinct + the deterministic text resolver.
let report = server.run_goal(
    "click Checkout",
    RunLimits { max_steps: 10, deadline: Some(Instant::now() + Duration::from_secs(60)) },
);
println!("{:?}", report.outcome);
```

`ultra_instinct_mcp::run_goal(transport, goal, policy, limits)` runs the same
loop on any `CdpTransport` (a `PipePage` you attached yourself, say);
`Server::run_goal` uses the server's current tab and policy and keeps the
session.

## Threading

One OS thread owns the `PipeBrowser`, every `PipePage`, and the `Server`.
They share the connection through `Rc<RefCell<..>>`, so they are `!Send`:
build them on the thread that uses them. There is no reader thread; a call
pumps the pipe until its own answer arrives (AGENTS.md rule 10). Parallel
screens mean separate threads, each with its own Chromium, pipe and server.

Chromium exits about 1 s after the host's end of the pipe closes (dropping
the `Server` drops every handle). Keep the server alive as long as the
browser should live.

## Pipe mode

- `cdp` arguments are ignored: every call drives the browser on the pipe.
- The first call binds the first page tab; `browser_new_tab` /
  `browser_switch_tab` move the binding; closing the bound tab falls back to
  another page, if any. Session keys are `pipe:<targetId>`.
- Tab tools are `Target.getTargets` / `createTarget` / `activateTarget` /
  `closeTarget`. `Target.setDiscoverTargets` is turned on, so a tab that
  goes away answers `target_gone` instead of a timeout.
- Set `PipeLimits::max_frame_bytes` to at least 32 MiB: a base64 full-page
  screenshot gets big. A frame over the cap costs that call only: it
  answers `frame_too_large` and the pipe stays usable for the next call and
  every other tab (an oversize event, or a reply to a call that already
  gave up, is dropped and counted in `PipeBrowser::dropped_frames`).
- Writes never block reads: a large request is written while Chrome's
  events keep being read, within the call deadline (`timeout`). A request
  cut off by its deadline part way closes the pipe (`pipe_closed`).

## Presses bound to what the caller saw

`browser_get_state` returns `snapshot`. Pass it with every index press
(click, type, select, check, focus, key with an index, right / middle /
double / triple click, file upload, hover, drag, download). The index is
read in that snapshot and the ticket binds the element it showed (role,
label, box, and its neighbourhood). If that element moved, changed, or left
the page, the press refuses `stale_ref` and nothing is pressed. Focus is
taken from the page now (your own typing moves it). With
`require_snapshot(true)` a bare index refuses `snapshot_required`.

## Which tools to allow-list

Allow the reading and pressing tools a task needs (the example above).
Leave out, unless you need them and trust the caller:

- `browser_exec` / `browser_javascript_exec`: arbitrary script in the page
  (it can read any field the page holds).
- `browser_get_html`: the whole DOM (text inputs' `value` attributes are
  dropped, but everything else is there).
- `browser_cookies` / `browser_storage` with `reveal`, `browser_state_save`:
  session tokens.
- `browser_set` `headers`, `browser_network` `block`: change what the page
  sends.
- `browser_file_upload`, `browser_download`, `browser_save_as_pdf`,
  `browser_screenshot` `path`: touch the host's file system (paths must be
  absolute; downloads follow `with_downloads`).

## Secrets

`browser_get_state` JSON elements carry `input_type` (when observe read it)
and `sensitive`. A field is sensitive when it is a password, hidden or file
input, when its label names a secret (password, passcode, PIN, CVV, CVC,
OTP, SSN, card, security, one-time), or when its input type could not be
read. A sensitive field's value is never shown, and text typed into one
(any length) is replaced by `••••` in every later tool output and error
(every tool: `observe`, `guard` and `verify` too, through the one exit in
`Server::call_tool`) and in HAR files, so a page that echoes it does not
leak it. A secret shorter than 4 chars (a CVV, a PIN) is masked only as a
whole token: `cvv=123` hides it, `41123` does not. Sensitive values are
never evicted; past 64 distinct ones a press that would type a new one
refuses `secrets_full` with nothing typed. Only the binary fields are left
as they are: `browser_screenshot`'s `base64` (and the MCP image item built
from it) and `browser_save_as_pdf`'s `data`; any other `data` or `base64`
(a `browser_exec` result, say) is masked.

`Server::run_goal` remembers every TYPE / SELECT payload before it is
dispatched: a sensitive field's at any length, any other field's from 4
chars (typed text is user data), the newest 32 of those. Its report's
labels and reasons are masked too. `RunReport::typed` still holds the
payloads (its `Debug` prints only a count): never log it. The free
`run_goal` / `run_goal_on` have no server to mask with; masking their
`typed` is the caller's. State files hold cookies and storage, never field
values.

## Limits

`Server::with_limits(ToolLimits { .. })`:

| Field | Default | When hit |
|---|---|---|
| `max_state_chars` | 100 000 | `browser_get_state` text is cut with `… [truncated N chars]`; `truncated: true` |
| `max_elements` | 1 000 | the element list stops there; `dropped_elements: N`, `truncated: true` |
| `max_screenshot_bytes` | 16 MiB (decoded) | taken once more as JPEG (quality 70); still over: `output_too_large` |
| `max_wait` | 60 s | `browser_wait` never waits longer |
| `max_text_chars` | 1 000 000 | `browser_get_text` / `browser_get_html` are cut inside the page script (the rest never crosses the pipe), end `… [truncated N chars]`; `truncated: true` |

`max_elements` also caps `observe`'s `regions` (`dropped_regions: N`,
`truncated: true`). These keep ordinary huge pages far below
`max_frame_bytes`.

`browser_screenshot` returns `base64` (and `format`) unless it fails; with
`path` it also writes the file.

## Error codes

Every error's JSON (`ToolError::to_value`) has `code` next to the `variant`
and `message` it always had. Typed refusals keep their old `variant`
(`TicketInvalid`, `InvalidArguments`, `Browser`) so older clients still
match. Pinned by `crates/ultra-instinct-browser-tools/tests/error_codes.rs`;
codes are only ever appended.

| `code` | Fields | Meaning |
|---|---|---|
| `needs_confirm` | `label` | the policy needs a confirm for this press; repeat with `confirm: label` (that call only) |
| `domain_not_allowed` | `host` | the page or URL is off the policy's allow-list |
| `stale_ref` | `reason` (`world-changed`, `target-changed`, `target-gone`, `other-tab`) | the element the ref named is not what the caller saw; nothing pressed |
| `unknown_ref` | `index`, `indexed` | no element at that index (in the snapshot given, or on the page) |
| `snapshot_required` | | the server requires `snapshot` with an index press |
| `target_gone` | `target_id` | the tab detached or was closed |
| `dialog_open` | `type`, `message` (the dialog's), `detail` | a JavaScript dialog is open; handle it with `browser_dialog` |
| `timeout` | `method`, `after_ms` | a CDP call got no answer in time |
| `pipe_closed` | `message` | the pipe is closed (Chromium exited); every later call fails the same way |
| `frame_too_large` | `limit` | this call's reply went past `max_frame_bytes`; it was skipped and the pipe stays usable |
| `tool_not_allowed` | `tool` | not on the allow-list |
| `path_not_absolute` | `path` | file paths must be absolute |
| `path_not_allowed` | `path`, `dir` | `Downloads::Dir` forces another folder |
| `downloads_off` | | `Downloads::Off` |
| `output_too_large` | `limit`, `bytes` | a screenshot over `max_screenshot_bytes` even as JPEG |
| `secrets_full` | `limit` | the server already masks 64 typed secrets; the new one was not typed |
| `invalid_arguments` | `message` | a malformed or missing argument |
| `ticket_invalid` | `message` | the hard gate refused (disabled, hidden, occluded, …) or the ticket was spent |
| `browser` | `message` | any other browser / CDP failure |
| `unknown_snapshot`, `snapshot_evicted`, `bad_snapshot` | `id` / `oldest` / `value` | the `snapshot` argument names nothing usable (the ring keeps 16) |
| `unknown_tool`, `missing_tool_name`, `goal_not_accepted`, `coordinates_not_accepted` | | the call itself is not accepted |
| `io`, `fixture` | `path`, `message` | a local file failed |

The rest (`missing_fixture`, `missing_before`, `missing_after`,
`duplicate_source`, `empty_text`, `unknown_region`, `unknown_role`,
`unknown_action`, `unsupported_action`, `non_finite_confidence`,
`runner_up_needs_confidence`, `runner_up_is_target`, `bad_runner_up`,
`target_not_top`, `confidence_with_query`, `expect_needs_observe_after`,
`expected_text_missing`, `region_still_present`, `bad_confidence`,
`confidence_out_of_range`, `missing_expect`, `both_expectations`,
`act_needs_cdp`, `unknown_executor`, `not_implemented`,
`browser_use_is_replay`, `browser_use_rejected`, `browser_use_script`,
`cua_is_replay`, `cua_rejected`, `cua_script`, `verify_no_effect`) belong to
observe / guard / verify and the legacy executors.

## Policy at runtime

`Server::set_policy(Policy)` replaces the policy between calls; sessions
stay open. A press's `confirm` argument lifts a confirm for that call only:
the next call is refused again.
