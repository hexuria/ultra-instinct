# MCP (optional adapter)

Indexed from [AGENTS.md](../../AGENTS.md).

`ultra-instinct mcp` is a newline-delimited JSON-RPC server: observe, guard,
verify. The locate, inspect, act and diff helpers are removed:
`observe` carries every region's id, label, parent and state, and `verify`
compares two snapshots. It owns a 16-entry snapshot ring and up to four live
CDP sessions; a failed live call drops its session, no retry. Guard finds its
target by exact label (and `role` / `within`; `proposed` picks one of several
matches), refuses `near` / `position` / `matcher` / `dims` by name, runs
`gate::check` before Allow, and issues `of_target` tickets like the agent. MCP
never bypasses tickets and is never required to run the agent.

`browser_*` tools are the browser-use-compatible host surface (crate
`ultra-instinct-browser-tools`, run on the MCP `Server` through a
crate-private `BrowserHost` adapter; drop-in MCP names: `browser_navigate`, `browser_get_state`, `browser_click`,
`browser_type`, `browser_scroll`, `browser_screenshot`, tab tools). Element
indexes come from `browser_get_state` in reading order; `browser_click` /
`browser_type` issue an `of_target` ticket and run `execute_ticketed`, so a
stale page refuses instead of clicking the wrong element. Page-level calls
(navigate, scroll, screenshot, tabs) are plain CDP like browser-use's. Over MCP, `browser_screenshot` answers with an `image` content item (`image/png`) the client can show, then the metadata as text; `Server::call_tool` keeps returning the base64 in its body. `browser_screenshot` and `browser_save_as_pdf` take an optional `path` that writes the decoded file.
Coordinate clicks are deliberately not offered. Diagnostics:
`browser_exec` (Runtime.evaluate),
`browser_extract_content` (deterministic DOM→markdown, no model call),
`browser_read_console`/`browser_read_network` (CDP events buffered by the
transport, domains enabled lazily, drained per read). The `CdpTransport`
trait gains `drain_events` (default empty); the ws transport buffers
unsolicited frames. Advanced input: `browser_right_click` /
`browser_middle_click` / `browser_double_click` / `browser_triple_click` /
`browser_file_upload` go gate → ticket → `consume_ticket_once` before the
press runs; `browser_hover`, `browser_drag` (index→index), `browser_zoom`,
`browser_send_key`/`browser_hold_key`, `browser_save_as_pdf`,
`browser_get_dropdown_options`/`browser_select_dropdown`. Every tool and `aui` verb, one line
each: [`docs/tools.md`](../tools.md).

`browser_get_state` defaults to text: a title / url line, then one line per
control, `@eN role "label"  state`; `format:"json"` returns `elements[]`
(with `input_type` and `sensitive`; `values:true` adds `value` / `checked` /
`expanded`, never on a sensitive element). Both formats record the observation and
return its `snapshot` id. Index arguments take `5`, `"5"` or `"@e5"`; an
index press given `snapshot` resolves the index in that snapshot and mints
its ticket there, so an element that moved, changed, or left refuses
`stale_ref` and nothing is pressed (`Server::require_snapshot(true)` refuses
a bare index, `snapshot_required`). Over MCP the text format also carries
the id in the result's `_meta.snapshot`, and a cut text keeps its
`· snapshot N` line. Input tools (`STATE_AFTER`) take `state: true`: the
reply adds the page after the input (`state`, text) and its `snapshot`,
or `state_error` (the input still ran). `Server::bind_to_latest_look(true)`
binds a press that names no snapshot to its tab's latest look (a
`get_state` or a `state: true` reply); the `aui` daemon sets it with
`require_snapshot(true)`, so a verb press binds to the latest `aui snap`
(or `--snap`) and refuses `snapshot_required` before any. A sensitive field (password / hidden /
file, a secret label, or an unreadable type) never shows its value, and text
typed into one is masked in every later output, error, and HAR. Every error
carries a stable `code`; the table is in [`docs/embedding.md`](../embedding.md).
`Server::with_pipe` serves the same tools over a caller-owned pipe (no HTTP
endpoint; `cdp` is ignored); `with_tool_allowlist`, `with_limits`,
`with_downloads` and `set_policy` are the host's knobs. File paths
(screenshot, PDF, state, download, HAR) must be absolute.

`browser_wait` with `text` / `url` / `control` / `gone` (no control has that
exact label) / `load` (`domcontentloaded` | `load` on `document.readyState`,
never network idle) observes about every 200 ms until every given condition
holds or `seconds` (default 10, cap `ToolLimits::max_wait`, 60 s) runs out; it never acts.

Policy: `ULTRA_INSTINCT_ALLOW_DOMAINS` / `ULTRA_INSTINCT_CONFIRM_PRESS`
(comma lists) set the server's `Policy`, and `ULTRA_INSTINCT_REFUSE_BLOB=1`
refuses every `blob:` page under the list; a press tool's `confirm` argument
names the label the user confirmed, for that call only (`Server::set_policy`
replaces the policy between calls). Ticketed inputs return `ticket`;
`verify:true` adds `verification`. `browser_send_key` with `index` is a
ticketed KEY. Input with no target (`browser_keyboard`, `browser_send_key`
/ `browser_hold_key` without `index`) checks the page host on an
observation taken now, before any `Input.*` call, the way presses do;
the focused region then goes through the guard's press rule (Enter /
Space on a button-like control, Enter in a field whose form submits with
a confirm label, needs `confirm`), as a ticketed KEY does. `browser_file_upload` paths
must be absolute; `Server::with_uploads(Anywhere | Off | Dir)` limits
which files (`Dir`: canonical path inside the canonical folder).

`browser_extra` ports (from agent-browser): `browser_dialog`,
`browser_go_forward`, `browser_reload`, `browser_check`, `browser_focus`,
`browser_keyboard`, `browser_get`, `browser_is`, `browser_count`,
`browser_screenshot` `annotate`, and `max_chars` on the text tools. Index
tools read the index from the tab's last observation when no input ran
since (or from `snapshot` when given), and act on that region in a fresh
observation. `browser_check` and `browser_focus` run under the policy and a
per-call `confirm` like `browser_click`. Page is enabled once
per tab outside observe for dialog tracking; a failing call while a dialog
is open says so. A call that opens one (a press whose handler calls
`alert()`) stops waiting at `Page.javascriptDialogOpening` and fails at once
with that hint (`CdpError::DialogOpened`); the press was dispatched, its
ticket stays consumed, and nothing retries it.

Login state (`browser_state_save` / `_load`, `browser_cookies`,
`browser_storage`): values are masked unless `reveal`; state files are 0600
and load refuses a file with group or other permissions; storage is written
only for the current origin, other origins come back as pending.

`browser_set` (viewport / device / media / offline / headers / reset),
`browser_download` (a ticketed press, then the file moves to `path`) and
`browser_network` (block / unblock via `Network.setBlockedURLs`, never Fetch
interception, because the blocking server cannot answer paused requests;
`har_start` / `har_stop` writes HAR without bodies, secret headers `•••`,
0600).
