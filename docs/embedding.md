# Embedding ultra-instinct over a CDP pipe

For a host that launches Chromium itself with `--remote-debugging-pipe` and
no DevTools port, and drives it from its own process (ADR 0013). There is no
stdio and no HTTP: the host hands the pipe ends to a `PipeBrowser`, wraps it
in an `ultra_instinct_mcp::Server`, and calls `Server::call_tool` with the
same `browser_*` tools the MCP server serves.

## Crates and features

| Crate | Why | Features |
|---|---|---|
| `ultra-instinct-mcp` | `Server`, the `browser_*` tools, `run_goal`, `ToolError`, `ToolLimits`, `Downloads`, `Uploads`, `Policy` (re-exported) | none |
| `ultra-instinct-cdp` | `PipeBrowser` / `PipeLimits` (and `PipeBackend` if you build the server with `with_backend`) | none |
| `ultra-instinct-guard` | `Policy` if you prefer it from its home crate | none |

Unix only (`cfg(unix)`). No async runtime, HTTP client or model SDK is
linked: the whole path is blocking Rust.

## Minimal example

```rust,ignore
use std::time::{Duration, Instant};

use serde_json::json;
use ultra_instinct_cdp::{PipeBrowser, PipeLimits};
use ultra_instinct_mcp::{Downloads, Policy, RunLimits, Server, ToolLimits, Uploads};

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
    .with_uploads(Uploads::Dir("/srv/box/uploads".into()))
    .with_tool_allowlist([
        "browser_navigate", "browser_get_state", "browser_click", "browser_type",
        "browser_select_dropdown", "browser_check", "browser_send_key",
        "browser_scroll", "browser_wait", "browser_screenshot", "browser_get_text",
        "browser_dialog", "browser_new_tab", "browser_list_tabs",
        "browser_switch_tab", "browser_close_tab",
        "run_goal", // Server::run_goal below; not a tool, but the list gates it
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

A run the policy stops ends `RunOutcome::Blocked { label, reason, kind }`
with nothing pressed. `kind` is the guard's refusal, typed (never parsed
from `reason`): `Some(BlockKind::NeedsConfirm)` (`label` is the press to
confirm), `Some(BlockKind::DomainNotAllowed { host })` (`host` is `None`
when the page URL could not be read), or `None` when the loop blocked
itself (repeated no-effect presses, a remote policy's BLOCKED).
`BlockKind::code()` is the matching error code (`needs_confirm` /
`domain_not_allowed`). `RunOutcome::Failed { error, code }` carries the
error code when `Server::run_goal` refused before the loop started
(`tool_not_allowed`, `invalid_arguments` for an invalid policy, a session
error's code); the loop's own failures (`max steps exceeded`, `deadline
exceeded`, a browser error) have `code: None`.

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
the page, the press refuses `stale_ref` and nothing is pressed. Only the
elements `browser_get_state` showed are indexed in its snapshot: an index
past `max_elements` refuses `unknown_ref` (`indexed` is the shown count),
even though the page has an element there. Focus is
taken from the page now (your own typing moves it). `browser_check` reads
the checked state from the page now, after the element still matches the
snapshot, so it never answers `changed: false` from a stale state. With
`require_snapshot(true)` a bare index refuses `snapshot_required`.

## Which tools to allow-list

Allow the reading and pressing tools a task needs (the example above).

A tool that runs another inside it needs that one on the list too
(`RUNS_INSIDE`): `browser_download`'s press is a click, so it needs
`browser_click`; `browser_search` is a navigation, so it needs
`browser_navigate`. Without it the call refuses ``tool_not_allowed`` naming
the inner tool (`tool: "browser_click"`) before any CDP call. The host sees
the list through `BrowserHost::allows` (default: everything allowed).

`Server::run_goal` is not a tool, but an allow-list gates it by the name
`run_goal`: with a list set and `run_goal` not on it, the run ends
`RunOutcome::Failed` with ``tool_not_allowed: tool `run_goal` is not allowed
on this server`` (`code: Some("tool_not_allowed")`) before any CDP call. Naming it allows the whole loop, which
presses whatever the page offers (click, type, select, hover, drag, scroll,
key), still under the server's `Policy`, the hard gate and tickets, whichever
press tools the list names. `tools/list` never shows it.

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
  absolute; downloads follow `with_downloads`, uploads `with_uploads`).

## Files the page may get or write

Every file path a tool takes is absolute or the call refuses
`path_not_absolute {path}` before any CDP call; nothing resolves a
relative path against the server's working directory (the `aui` verbs
absolutize against the shell's before they call).

- `Server::with_downloads(Downloads::Anywhere | Off | Dir(dir))`:
  `browser_download` writes anywhere (default), refuses `downloads_off`, or
  only directly in `dir` (else `path_not_allowed {path, dir}`).
- `Server::with_uploads(Uploads::Anywhere | Off | Dir(dir))`:
  `browser_file_upload` hands the page any absolute path (default; a page
  can be given any file the host process can read), refuses `uploads_off`,
  or only files under `dir`. Under `Dir` the file and the folder are both
  canonicalized first (`..` and symlinks resolved), the file must be a
  regular file inside the folder, else `path_not_allowed {path, dir}`, and
  Chrome gets the canonical path. A missing file refuses the same way.

## Secrets

`browser_get_state` JSON elements carry `input_type` (when observe read it)
and `sensitive`. With `values: true` (default false) each element that is
not sensitive also carries the state observe read: `value` (a text-like
input's or text area's value, or a select's selected option), `checked`
and `expanded`. A sensitive element never carries any of them. A value
longer than `max_state_chars` / `max_text_chars` (the smaller) is cut with
`… [truncated N chars]` and `value_truncated: true`; observe itself reads
at most 200 chars of a value. Values are masked like every output (below),
and labels are never taken from values. The text format already prints
non-sensitive values and is unchanged. A field is sensitive when it is a password, hidden or file
input, when its label names a secret (password, passcode, PIN, CVV, CVC,
OTP, SSN, card, security, one-time), or when its input type could not be
read. A sensitive field's value is never shown, and text typed into one
(any length) is replaced by `••••` in every later tool output and error
(every tool: `observe`, `guard` and `verify` too, through the one exit in
`Server::call_tool`) and in HAR files, so a page that echoes it does not
leak it. The forms a URL or HTML writes it in are masked too: percent
encoding (either hex case, `%20` or `+` for a space: `p@ss word` as
`p%40ss+word`) and HTML entities (`&amp;`, `&lt;`, `&gt;`, `&quot;`,
`&#39;`). A secret shorter than 4 chars (a CVV, a PIN) is masked only as a
whole token: `cvv=123` hides it, `41123` does not (a `%XX` next to it
counts as the char it encodes). Sensitive values are
never evicted; past 64 distinct ones a press that would type a new one
refuses `secrets_full` with nothing typed. Only the binary fields are left
as they are: `browser_screenshot`'s `base64` (and the MCP image item built
from it) and `browser_save_as_pdf`'s `data`; any other `data` or `base64`
(a `browser_exec` result, say) is masked.

Masking cannot reach pixels, so captures hide instead. Once the server has
typed a sensitive value, `browser_screenshot` and `browser_save_as_pdf`
first observe the page and draw every sensitive text field (a CVV or PIN in
a `type=text` box, say; password inputs are dotted already) as dots
(`-webkit-text-security: disc`, inline, through CDP arguments), capture,
then put each field's `style` attribute back exactly as it was, also when
the capture failed (a failed restore shows as `restore_error`). Values are
never changed. A field that cannot be hidden refuses `secret_on_screen`
(the fields already hidden are put back) and nothing is captured. Cross-origin iframes and closed shadow roots are
not covered (observe cannot see them), nor text the page copies elsewhere
from a field: keep `browser_screenshot` off the allow-list if that matters.
Before any secret is typed a capture is exactly one CDP call.

Whether typed text is a secret is decided on the page as the input lands:
`browser_type` checks the field on the executor's own fresh observation as
well as the one the caller saw (a text box that became a password input in
between is a secret), and `browser_keyboard` reads the focus again when a
press ran since the last observation (on every call when a policy is set).
A Tab inside `browser_keyboard` text moves the focus after it was read, so
every piece typed after the first Tab is treated as a secret. When neither can tell, the text is
treated as a secret.

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
| `unknown_ref` | `index`, `indexed` | no element at that index (among those the given snapshot showed, or on the page) |
| `snapshot_required` | | the server requires `snapshot` with an index press |
| `target_gone` | `target_id` | the tab detached or was closed |
| `dialog_open` | `type`, `message` (the dialog's), `detail` | a JavaScript dialog is open; handle it with `browser_dialog` |
| `timeout` | `method`, `after_ms` | a CDP call got no answer in time |
| `pipe_closed` | `message` | the pipe is closed (Chromium exited); every later call fails the same way |
| `frame_too_large` | `limit` | this call's reply went past `max_frame_bytes`; it was skipped and the pipe stays usable |
| `tool_not_allowed` | `tool` | not on the allow-list |
| `path_not_absolute` | `path` | file paths must be absolute |
| `path_not_allowed` | `path`, `dir` | `Downloads::Dir` / `Uploads::Dir` keeps files in another folder |
| `downloads_off` | | `Downloads::Off` |
| `uploads_off` | | `Uploads::Off` |
| `output_too_large` | `limit`, `bytes` | a screenshot over `max_screenshot_bytes` even as JPEG |
| `secrets_full` | `limit` | the server already masks 64 typed secrets; the new one was not typed |
| `secret_on_screen` | `message` | a screenshot / PDF could not hide the page's sensitive fields; nothing was captured |
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

Input with no target (`browser_keyboard`, and `browser_send_key` /
`browser_hold_key` without an `index`) goes to whatever has focus, so it
runs under the policy like a press. With a policy set, the call observes
the page first and, before any `Input.*` call:

- checks the page's host exactly as a press does: off the allow-list is
  `domain_not_allowed {host}`, an unreadable URL while the list is set
  fails closed (`ticket_invalid`, `domain-not-allowed`, as for a press), a
  URL with no host (`about:blank`, `data:`) passes;
- runs the key on the focused control through the guard's press rule
  (below), as a ticketed KEY on that control would be. With confirm
  patterns set, an Enter / Space to a focus no observation names (no
  observation, nothing focused, or focus in a cross-origin frame) refuses
  `needs_confirm` with the label `(unobserved focus)`, lifted only by
  confirming exactly that label; and `browser_keyboard` text with an Enter
  / Space after a Tab refuses `invalid_arguments` (the Tab moves the focus
  to a control not observed yet): send it in its own call.

Keys are presses for the confirm rule, on every path (the agent loop and
`Server::run_goal`, a ticketed `browser_send_key` with an index, and the
keyboard tools above), checked by the guard on the observation the input
lands on:

- Enter, Space or NumpadEnter (with any modifiers) on a button-like
  control (it can be clicked or toggled and takes no text) presses it: its
  label is checked.
- Enter in a field (or another form control) submits its form, which
  presses the form's first submit button: that button's accessible name
  (`aria-labelledby`, then `aria-label`, then its text or value, as the
  click check reads it) is checked, and `needs_confirm` names it (`label:
  "Pay $18"`, though the target was the field). Observe records the form
  evidence. A form with no submit button still submits on Enter when at
  most one of its fields blocks implicit submission (text, search, url,
  tel, email, password, number, date / time inputs): that refuses with the
  label `(form submit)`, as does form evidence observe could not read;
  confirm that label to send it. A field in no form, a button-less form
  with two or more such fields, a text area and a select submit nothing.
- Space in a field is text, and other keys (Tab, arrows, Escape) press
  nothing.

With no policy set these tools send their input as before.
