# Browser-use parity

`ultra-instinct mcp` exposes a `browser_*` tool surface mirroring
[browser-use](https://docs.browser-use.com) (toolsets-for-claude and the
browser-use MCP server), plus ports from
[vercel-labs/agent-browser](https://github.com/vercel-labs/agent-browser)
([ADR 0012](adr/0012-borrow-and-prune.md)). 53 `browser_*` tools, alongside
the three product tools (`observe`, `guard`, `verify`): 56 in `TOOLS`.

**The edge over browser-use:** browser-use presses what the model says.
Every element-mutating `browser_*` tool here resolves a 1-based index on a
fresh observe, runs the hard `gate`, and takes an `ActionTicket` before the
press. Click / type / select route through `execute_ticketed`, which
re-observes and revalidates the ticket against fresh page state. Inputs the
executor does not carry (right / middle / double / triple click, file
upload, drag) route through `consume_ticket_once` — same consume-before-
press order and ledger, revalidated on the gate's own observation (a
CPU-only gap; no drift barrier between mint and press).

## Indexing contract

Every index-taking tool shares one map: interactive regions (clickable or
typeable per the observed `ActionSpace` target set) deduplicated and sorted
in reading order `(floor(y/20), x, id)`, **1-based**. The same map produces
`browser_get_state`'s `elements[]` — what the client sees is what the tools
resolve. Coordinate clicks are deliberately not offered: indices only.

## Matrix

| browser-use | ultra-instinct | notes |
|---|---|---|
| `navigate` | `browser_navigate` | direct `Page.navigate`; `new_tab: true` opens a tab instead |
| `new_tab` | `browser_new_tab` | browser-level `Target.createTarget` + rebinding |
| `list_tabs` | `browser_list_tabs` | HTTP `/json/list` filtered to pages |
| `switch_tab` | `browser_switch_tab` | rebinds the session's tab (exact id or unique non-empty prefix) |
| `close_tab` | `browser_close_tab` | falls back to a remaining page target |
| `go_back` | `browser_go_back` | `Page.getNavigationHistory` + `navigateToHistoryEntry` |
| `wait` | `browser_wait` | sleeps `seconds`; with `text` / `url` / `control` / `gone` / `load` observes every 200 ms until it holds (cap 60; blocks the single-threaded server) |
| `read_page` / state | `browser_get_state` | full observe → indexed `elements[]` |
| `get_page_text` | `browser_get_page_text`, `browser_get_text` | `innerText` via evaluate |
| `get_html` | `browser_get_html` | `outerHTML` via evaluate |
| `find` | `browser_find` | label and/or `role` filter over the index |
| `screenshot` | `browser_screenshot` | `Page.captureScreenshot` → MCP `image` content (`image/png`) plus a metadata text item; optional `path` writes the PNG |
| `scroll` | `browser_scroll` | `Input.dispatchMouseEvent` mouseWheel by `pages` (clamped 0.05–100) |
| `scroll_to` | `browser_scroll_to` | index → `DOM.scrollIntoViewIfNeeded` on the region's node |
| `left_click` | `browser_click` | **index only** — gate → ticket → executor reobserve → DOM press |
| `right_click` | `browser_right_click` | gate → ticket → `consume_ticket_once` → press |
| `middle_click` | `browser_middle_click` | same ticketed path |
| `double_click` | `browser_double_click` | `clickCount: 2`, same ticketed path |
| `triple_click` | `browser_triple_click` | `clickCount: 3`, same ticketed path |
| `hover` | `browser_hover` | pointer move, no press — no ticket needed |
| `mouse_move` | — | covered by `browser_hover` (element-indexed) |
| `left_mouse_down/up` | — | covered by `browser_drag` |
| `left_click_drag` | `browser_drag` | element→element; source gated+ticketed like clicks |
| `type` | `browser_type` | index + `text` → gate → ticket → `DOM_TYPE_FUNCTION` value-setter |
| `form_input` | `browser_form_input` | alias of `browser_type` |
| `key` | `browser_send_key` | `Input.dispatchKeyEvent` down+up; named keys + printable chars |
| `hold_key` | `browser_hold_key` | same, `hold_ms` between down and up (cap 10 s) |
| `file_upload` | `browser_file_upload` | `DOM.setFileInputFiles` on the indexed input — ticketed |
| `select` / dropdown | `browser_select_dropdown` | index + `text`; choice grounded in observed enabled options |
| `get_dropdown_options` | `browser_get_dropdown_options` | observed `options` + `selected` |
| `javascript_exec` | `browser_javascript_exec`, `browser_exec` | `Runtime.evaluate` (returnByValue, awaitPromise) |
| `extract_content` | `browser_extract_content` | deterministic DOM→markdown — no model call |
| `read_console` | `browser_read_console` | `Runtime`/`Log` events buffered by the transport; drains per read |
| `read_network` | `browser_read_network` | `Network.*` aggregated by requestId; drains per read |
| `search` (google) | `browser_search` | Google URL nav for `query` |
| `zoom` | `browser_zoom` | `Emulation.setPageScaleFactor`; `factor` or `direction` in/out/reset |
| `save_as_pdf` | `browser_save_as_pdf` | `Page.printToPDF` → base64; optional `path` writes the PDF |
| `close` (browser) | `browser_close_tab` on last tab | whole-browser close stays host-managed |
| `list_sessions` / `close_session` / agent | — | browser-use Cloud surface; not local-browser tools |
| `Bash` toolset | — | host-side process tool; out of scope by design |

## agent-browser

[agent-browser](https://github.com/vercel-labs/agent-browser) ([command reference](https://agent-browser.dev/commands), checked 2026-11-08) is a shell
CLI. Its commands map to MCP tools here and to `aui` shell verbs over the
session daemon (`aui skills get core`). Refs are `@eN` from `aui snap` /
`browser_get_state`; there are no CSS / XPath selectors and no coordinate
mouse.

| agent-browser | MCP tool | `aui` verb |
|---|---|---|
| `open <url>` | `browser_navigate` | `open <url> [--launch [--headed]]` |
| `snapshot` | `browser_get_state` (text default, `format:"json"`) | `snap [--json]` |
| `click` / `dblclick` | `browser_click` / `browser_double_click` | `click <ref>` |
| `fill` / `type` | `browser_type` / `browser_keyboard` | `type <ref> <text>`, `keyboard type\|insert <text>` |
| `press <key>` | `browser_send_key` | `key <ref> <key>` |
| `select` | `browser_select_dropdown` | `select <ref> <option>` |
| `check` / `uncheck` | `browser_check` | `check <ref>`, `uncheck <ref>` |
| `focus` / `hover` | `browser_focus` / `browser_hover` | `focus <ref>` |
| `scroll` | `browser_scroll` | `scroll [up\|down]` |
| `wait` (text, url, selector gone, load) | `browser_wait` | `wait --text\|--url\|--control\|--gone <v>`, `wait --load domcontentloaded\|load` |
| `get text\|value\|attr\|box` | `browser_get`, `browser_get_text` | `get value\|label\|role\|box <ref>`, `get attr <ref> <name>` |
| `is visible\|enabled\|checked` | `browser_is` | `is visible\|enabled\|checked\|editable <ref>` |
| `get count` / `find` | `browser_count` / `browser_find` | `count [--label L] [--role R]` |
| `screenshot [--annotate]` | `browser_screenshot` (`annotate`) | `shot [--annotate] [path]` |
| `back` / `forward` / `reload` | `browser_go_back` / `browser_go_forward` / `browser_reload` | `forward`, `reload [--ignore-cache]` |
| `dialog accept\|dismiss` | `browser_dialog` | `dialog [status]\|accept [text]\|dismiss` |
| `tab` / `tab new\|close` / `tab <id>` | tab tools | `tabs` |
| `state save\|load` | `browser_state_save` / `browser_state_load` | `state save\|load <path>` |
| `cookies` | `browser_cookies` | `cookies [list]\|set\|clear` |
| `storage local\|session` | `browser_storage` | `storage local\|session [list]\|get\|set\|clear` |
| `set viewport\|device\|media\|offline\|headers` | `browser_set` | `set viewport W H [scale]\|device\|media\|offline\|headers K=V…\|reset` |
| `download` | `browser_download` | `download <ref> <path>` |
| `network route\|unroute`, `network har start\|stop` | `browser_network` (block / unblock, HAR) | `network block\|unblock`, `network har start\|stop <path>` |
| `--max-output` | `max_chars` | `--max-output N` |
| `batch` | — | `batch "<verb args>"…` |
| `session` / `session list` | — | `session`, `session list` |
| `install` / `doctor` | — | `aui install`, `aui doctor` |
| `skills` | — | `aui skills list\|get` |
| `mouse`, CSS / XPath selectors, `eval` | — (`browser_exec` for script) | not offered |
| other engines, streaming, dashboard, auth vault, plugins, react / vitals / a11y / trace / record | — | not borrowed (ADR 0012) |

## Diagnostics caveat

One blocking CDP socket, no reader thread (the repo's concurrency invariant):
`Runtime`, `Log`, and `Network` events are buffered by the transport only
while a CDP call is in flight, capped at 1024 (transport) / 2048 (server
buffer). `browser_read_console` / `browser_read_network` lazily enable their
domains on first read, then drain. Events between calls are captured at the
next call's read — there is no passive event stream, and none is claimed.

## Tests

`crates/ultra-instinct-mcp/tests/browser_tools.rs` drives most tools through the CDP
replay harness (`ScriptBuilder` + `ReplayTransport`): call sequence, index
resolution, gate→ticket→press order, observed-option grounding, and
refusals (unobserved option, out-of-range index) are pinned without a live
Chrome. Not yet covered at wire level: `browser_type`, the tab tools
(HTTP-driven), `browser_scroll`, `browser_hold_key`,
`browser_middle_click` / `browser_triple_click`, `browser_get_html` /
`browser_get_text`, `browser_screenshot`, `browser_search`, and
`browser_navigate`'s `new_tab` path.
