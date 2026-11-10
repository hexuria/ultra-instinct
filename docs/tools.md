# Tools and verbs

`ultra-instinct mcp` (or `aui mcp`) serves 56 tools over newline-delimited
JSON-RPC on stdio: the three product tools and 53 `browser_*` tools (the
`TOOLS` list in `crates/ultra-instinct-mcp/src/lib.rs`). An embedding host
calls the same tools through `Server::call_tool` ([`embedding.md`](embedding.md)).
The `aui` shell verbs at the end are thin wrappers over the `browser_*` tools,
run through a per-session daemon.

## Common arguments

| Argument | Meaning |
|---|---|
| `cdp` | CDP HTTP endpoint (default `http://127.0.0.1:9222`). Ignored over a pipe (`Server::with_pipe`). |
| `index` | A `browser_get_state` ref: `5`, `"5"` or `"@e5"`. 1-based, reading order. |
| `snapshot` | The id `browser_get_state` returned. The index is read in that snapshot and the press is bound to the element it showed; if it moved, changed, or left, the call refuses `stale_ref` and nothing is pressed. Without it the index resolves on a fresh observation (unless the host set `require_snapshot`). |
| `confirm` | The label the user confirmed, for this call only; lifts a `needs_confirm` refusal. |
| `verify` | `true` adds a `verification` (the effect the press had) to a ticketed input. |
| `state` | Input tools: `true` adds the page after the input (`state`, as `browser_get_state` text) and its `snapshot`, so the next press binds to it with no extra call. A failed read is `state_error`; the input still ran. |
| `mouse` | Clicks (`browser_click`, `browser_check`, `browser_download`): default `true`, a real pointer press at the center; `false` sends a DOM script click. |
| `max_chars` | Text tools: cut the output, ending `… [truncated N chars]`. |

Every index press runs the hard gate, takes a one-shot `ActionTicket`, and
consumes it before the press. Every error carries a stable `code`
([`embedding.md`](embedding.md#error-codes)). There are no coordinate clicks
and no CSS / XPath selectors.

## Product tools

These never click. They are for hosts that keep their own executor.

| Tool | Does | Key arguments |
|---|---|---|
| `observe` | Read the page into regions: id, role, label, parent, state, focus, open dialogs, and a snapshot id | `fixture` or `cdp` |
| `guard` | Find the target by exact label (trimmed, case ignored), unique or refuse, run the hard gate, return Allow with an `ActionTicket`, or refuse / escalate | `target` (or `text`), `role`, `within`, `proposed`, `seen_snapshot` |
| `verify` | Check a postcondition on a fresh observe, or between two snapshots | `expect_text`, `expect_absent`; `before_snapshot` + `after_snapshot` with `expect_appeared` / `expect_disappeared` / `expect_url_changed`, `ticket` |

## Read the page

| Tool | Does | Key arguments |
|---|---|---|
| `browser_get_state` | Title, URL and one line per control (`@eN role "label"  state`), and the `snapshot` id | `format` (`text` default, `json` for `elements[]` with `input_type` / `sensitive`), `values` (json: adds `value` / `checked` / `expanded` / `current`, never on a sensitive element), `max_chars` |
| `browser_find` | Interactive elements whose label and / or role match, with their indexes | `text`, `role` |
| `browser_count` | How many interactive elements match an exact label and / or role, and their indexes | `label`, `role` |
| `browser_get` | One fact about an element: `value` (never a password's), `attr`, `box`, `label`, `role` | `index`, `what`, `name` (for `attr`) |
| `browser_is` | `visible` / `enabled` / `checked` / `editable`, from a fresh observation | `index`, `what` |
| `browser_get_text` | `document.body.innerText` | `max_chars` |
| `browser_get_page_text` | Alias of `browser_get_text` | `max_chars` |
| `browser_get_html` | `outerHTML` of the document (text inputs' `value` attributes dropped) | `max_chars` |
| `browser_extract_content` | Readable content as markdown (a deterministic DOM walk, no model) | `max_chars` |
| `browser_get_dropdown_options` | Observed enabled options and selected labels of a `<select>` | `index` |
| `browser_screenshot` | Viewport PNG, as an MCP `image` item plus metadata | `path` (absolute; also writes the file), `annotate` (draw the `@eN` boxes) |
| `browser_save_as_pdf` | `Page.printToPDF` as base64 | `path`, `landscape`, `print_background`, `scale` |

## Press

| Tool | Does | Key arguments |
|---|---|---|
| `browser_click` | Left click through the ticketed executor (re-observes, refuses on drift). A real pointer press at the center by default | `index`, `snapshot`, `mouse` (`false`: a DOM script click), `confirm`, `verify`, `state` |
| `browser_right_click`, `browser_middle_click`, `browser_double_click`, `browser_triple_click` | That button / click count, through gate, ticket and consume-before-press | `index`, `snapshot`, `confirm` |
| `browser_check` | Set a checkbox / radio / switch; already so: `changed: false`, no press; else a click and a read-back | `index`, `checked`, `snapshot`, `mouse`, `confirm` |
| `browser_hover` | Move the pointer to an element (no press, no ticket) | `index`, `snapshot` |
| `browser_drag` | Press on one element, drag to another, release; both ends bound in the ticket | `from_index`, `to_index`, `snapshot`, `confirm` |
| `browser_focus` | Move keyboard focus to an element (gate, ticket, then `DOM.focus`) | `index`, `snapshot`, `confirm` |
| `browser_scroll_to` | Scroll an element into view | `index` |
| `browser_dialog` | Status of an open alert / confirm / prompt, or accept / dismiss it | `action` (`status`, `accept`, `dismiss`), `text` (prompt answer) |

## Type and keys

| Tool | Does | Key arguments |
|---|---|---|
| `browser_type` | Set a field's text through the ticketed executor; the value is read back | `index`, `text`, `snapshot`, `verify` |
| `browser_form_input` | Alias of `browser_type` | `index`, `text` |
| `browser_select_dropdown` | Select exactly one option of a `<select>`, grounded in its observed options | `index`, `text`, `snapshot`, `verify` |
| `browser_send_key` | One key. With `index`: a ticketed KEY on that element (`ArrowLeft`, `ArrowRight`, `ArrowUp`, `ArrowDown`, `Home`, `End`, `PageUp`, `PageDown`, `Enter`, `Escape`). Without: to the focus | `key`, `index`, `snapshot`, `confirm`, `verify` |
| `browser_hold_key` | Hold a key down (capped at 10 s), to the focus | `key`, `seconds` or `hold_ms`, `confirm` |
| `browser_keyboard` | Type at the focus with no target: `type` sends key events per char, `insert` one `Input.insertText` | `text`, `mode`, `confirm` |
| `browser_file_upload` | Set files on a file input (`DOM.setFileInputFiles`); paths must be absolute | `index`, `path` or `paths`, `snapshot`, `confirm` |

Keys count as presses for the confirm rule: Enter / Space / NumpadEnter on a
button-like control, and Enter in a field whose form would submit through a
matching button. Keys with no target run under the policy like a press. The
full rules are in [`embedding.md`](embedding.md#policy-at-runtime).

## Tabs and navigation

| Tool | Does | Key arguments |
|---|---|---|
| `browser_navigate` | Load a URL in the current tab (the domain allow-list is checked first) | `url`, `new_tab` |
| `browser_new_tab` | Open a tab (default `about:blank`) and make it current | `url` |
| `browser_list_tabs` | Page tabs: id, url, title | |
| `browser_switch_tab` | Drive another tab from now on (exact id or unique prefix) | `tab_id` |
| `browser_close_tab` | Close a tab; falls back to another one if it was current | `tab_id` |
| `browser_go_back`, `browser_go_forward` | One history entry back / forward | |
| `browser_reload` | Reload the current tab | `ignore_cache` |
| `browser_search` | Navigate to a Google results URL (runs `browser_navigate` inside it) | `query` |
| `browser_scroll` | Scroll by viewport heights | `down`, `pages` (default 1) |
| `browser_zoom` | Page scale factor | `factor` (0 to 10), or `direction` (`in`, `out`, `reset`) |

## Wait

| Tool | Does | Key arguments |
|---|---|---|
| `browser_wait` | With a condition: observe about every 200 ms until every given condition holds or `seconds` (default 10) runs out; returns `seen`, `after_ms`, `looks` and the last URL. With none: sleep `seconds` (default 3). Capped by `ToolLimits::max_wait` (60 s). Never acts | `text` (page text contains), `url` (URL contains), `control` (a control label contains), `gone` (no control has exactly this label), `load` (`domcontentloaded` or `load` on `document.readyState`, not network idle), `seconds` |

## Login state

| Tool | Does | Key arguments |
|---|---|---|
| `browser_state_save` | Every cookie plus the current origin's local / session storage to a 0600 JSON file; returns counts only | `path` |
| `browser_state_load` | Set a saved state's cookies and the current origin's storage; other origins come back as `pending_origins`. Refuses a file group or others can read | `path` |
| `browser_cookies` | `list` (values masked unless `reveal`), `set`, `clear` | `action`, `name`, `value`, `domain`, `path`, `expires`, `http_only`, `secure`, `same_site`, `reveal` |
| `browser_storage` | The current origin's storage: `list` (masked unless `reveal`), `get`, `set`, `clear` | `area` (`local`, `session`), `action`, `key`, `value`, `reveal` |

## Network

| Tool | Does | Key arguments |
|---|---|---|
| `browser_network` | `block` / `unblock` URL patterns (`Network.setBlockedURLs`, never Fetch interception); `har_start` / `har_stop` writes HAR 1.2 without bodies, cookie and authorization values redacted, 0600 | `action`, `patterns`, `path` |
| `browser_read_network` | Network entries buffered since the last read, by request id (enables `Network` on first use) | `max_chars` |

## Browser settings and files

| Tool | Does | Key arguments |
|---|---|---|
| `browser_set` | `viewport`, `device` (viewport + mobile + touch + user agent), `media` (color scheme), `offline`, `headers` (extra HTTP headers), `reset` | `what`, `width`, `height`, `scale`, `name`, `color_scheme`, `on`, `map` |
| `browser_download` | A ticketed click (like `browser_click`, which must be allowed too), then wait for the download and move it to `path` | `index`, `path`, `snapshot`, `confirm`, `overwrite`, `mouse`, `timeout_ms` (default 30000) |

## Diagnostics

| Tool | Does | Key arguments |
|---|---|---|
| `browser_exec`, `browser_javascript_exec` | `Runtime.evaluate` (awaits promises, returns by value) | `script` or `expression` |
| `browser_read_console` | Console entries and JS exceptions buffered since the last read (enables `Runtime` / `Log` on first use) | `max_chars` |

Events are buffered only while a CDP call is in flight (one blocking socket,
no reader thread), so events between calls show up at the next read.

## `aui` shell verbs

Each verb talks to the session daemon on `$TMPDIR/aui/<session>.sock`, which
owns one MCP `Server`. Every verb takes `--session <name>` (default
`default`), `--cdp <url>` and `--max-output N`. `<ref>` is `@eN` from the
latest `aui snap` (or `N`): the daemon binds each ref press to that snap, so
a page that changed since refuses (`· run aui snap`) instead of pressing
another element, and a press before any snap refuses. `--snap` on an input
verb prints the page after it and binds the next refs to that look. Relative paths resolve against the shell's
directory. A refusal or error exits non-zero. `aui skills get core` prints
the full guide.

| Verb | Tool | Does |
|---|---|---|
| `open <url> [--launch [--headed]] [--idle <secs>] [--allow-domain <host>]... [--confirm-press <glob>]...` | `browser_navigate` | Start the daemon (and with `--launch` its own Chrome: headless at 1440×900, or maximized with `--headed`), then navigate. Session flags are fixed when the daemon starts; `--idle` is 1 to 86400 s, default 3600 |
| `snap [--json]` | `browser_get_state` | Read the page |
| `click <ref> [--confirm <label>] [--script]` | `browser_click` | Ticketed click: a pointer press; `--script` for a DOM script click |
| `type <ref> <text>` | `browser_type` | Set a field's text |
| `select <ref> <option>` | `browser_select_dropdown` | Pick one option |
| `key <ref> <key>` | `browser_send_key` | One key on that control |
| `check <ref>`, `uncheck <ref>` `[--script]` | `browser_check` | Set a checkbox |
| `focus <ref>` | `browser_focus` | Focus a control |
| `keyboard type\|insert <text> [--confirm <label>]` | `browser_keyboard` | Keys into whatever has focus |
| `scroll [up\|down]` | `browser_scroll` | One viewport |
| `wait --text\|--url\|--control\|--gone <v>`, `wait --load domcontentloaded\|load`, `[--timeout s]` | `browser_wait` | Wait for a condition |
| `shot [--annotate] [path]` | `browser_screenshot` | Save a PNG |
| `tabs` | `browser_list_tabs` | List tabs |
| `dialog [status]`, `dialog accept [text]`, `dialog dismiss` | `browser_dialog` | See or answer a dialog |
| `forward`, `reload [--ignore-cache]` | `browser_go_forward`, `browser_reload` | History forward, reload |
| `get value\|label\|role\|box <ref>`, `get attr <ref> <name>` | `browser_get` | Read one fact |
| `is visible\|enabled\|checked\|editable <ref>` | `browser_is` | Prints `true` or `false` |
| `count [--label L] [--role R]` | `browser_count` | Count matching controls |
| `state save\|load <path>` | `browser_state_save` / `_load` | Save or reuse a login |
| `cookies [list] [--name N] [--domain D] [--reveal]`, `cookies set <name> <value> [--domain D] [--path P] [--expires SECS] [--http-only] [--secure] [--same-site Strict\|Lax\|None]`, `cookies clear [--name N] [--domain D]` | `browser_cookies` | Read or change cookies |
| `storage local\|session [list] [--reveal]`, `… get <key>`, `… set <key> <value>`, `… clear [<key>]` | `browser_storage` | The origin's storage |
| `set viewport W H [scale]`, `set device <name>`, `set media dark\|light\|no-preference`, `set offline on\|off`, `set headers K=V...`, `set reset` | `browser_set` | Emulation |
| `download <ref> <path> [--overwrite] [--timeout-ms N]` | `browser_download` | Ticketed click, then save the file |
| `network block <pattern>...`, `network unblock [<pattern>...]`, `network har start`, `network har stop <path>` | `browser_network` | Block URLs, record a HAR |
| `close` | | Stop the daemon and its Chrome |

`key`, `check` / `uncheck`, `type`, `select`, `focus` and `download` take no
`--confirm`. When one of them refuses `NeedsConfirm` (an Enter that would
submit a form through a "Pay" button, say), press the named control with
`aui click <ref> --confirm "<label>"` instead.

Other commands:

| Command | Does |
|---|---|
| `aui batch [--bail] [--json] "<verb args>"...` | Run verbs in order on one session (no args: a JSON array of strings on stdin); `--json` prints `[{cmd, ok, output}]` |
| `aui session`, `aui session list` | The current session name; live sessions (stale sockets removed) |
| `aui skills list`, `aui skills get <name>` | Guides that ship in the binary (`core`) |
| `aui doctor [--json] [--quick] [--cdp <url>]` | Check Chrome, the endpoint, a test launch, and `$TMPDIR/aui` |
| `aui install [--run]` | Print (or run) the Chrome for Testing install command |
| `aui run --goal <text> ...` | The agent loop for runs nobody checks step by step: the runtime picks every step (`--decision instinct`: Instinct, no model; `--decision model`: your `TEXT_MODEL_*` endpoint; `jev` / `clef`; every model is limited to offered actions) and abstains rather than guess; see `aui --help` |
| `aui replay --diary <dir>`, `aui lessons ...`, `aui exams ...` | Offline diary tools |
| `aui observe`, `aui guard`, `aui verify` | The product tools from the command line (`--fixture` or `--cdp`) |
