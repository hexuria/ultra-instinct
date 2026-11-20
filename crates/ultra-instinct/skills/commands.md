# ultra-instinct: every shell verb

A reference of `aui skills get core`. Read the core guide first.

| verb | does |
|---|---|
| `open <url>` | navigate the session's tab (spawns the daemon) |
| `snap [--json]` | read the page |
| `diff` | look again and print only what changed since your last `snap` or `diff`: `+` a new control (with its ref), `-` one that left, `~` one whose state changed, `page:` a new title or URL; the refs are the new look's |
| `click <ref> [--confirm <label>] [--script]` | ticketed click: a real pointer press at the center, like a person; `--script` sends a DOM script click instead |
| `type <ref> <text>` | set a field's text; the value is read back |
| `select <ref> <option>` | pick one option of a `<select>` |
| `key <ref> <key>` | one key on that control: `Enter`, `Escape`, arrows, `Home`, `End`, `PageUp`, `PageDown`; an Enter that would press or submit through a `--confirm-press` label is refused like a click |
| `scroll [up\|down]` | scroll one viewport |
| `wait --text\|--url\|--control <v> [--timeout s]` | look about every 200 ms until the page shows that text, the URL contains it, or a control label contains it (default 10 s, max 60) |
| `wait --gone <label>` | until no control has exactly that label (a spinner, a "Loading" button) |
| `wait --load domcontentloaded\|load` | until `document.readyState` gets there (not network idle) |
| `shot [--annotate] [path]` | save a PNG screenshot; `--annotate` draws the `@eN` boxes on it |
| `tabs` | list tabs |
| `dialog [status]` / `dialog accept ["text"]` / `dialog dismiss` | see or answer an alert / confirm / prompt |
| `back`, `forward`, `reload [--ignore-cache]` | history back / forward, reload |
| `hover <ref>` | move the pointer over a control (opens hover menus; presses nothing) |
| `drag <ref> <ref> [--confirm <label>]` | ticketed drag from the first control onto the second |
| `upload <ref> <path>… [--confirm <label>]` | ticketed: set a file input's files |
| `dblclick <ref>`, `rightclick <ref>`, `midclick <ref>`, `tripleclick <ref>` `[--confirm <label>]` | ticketed click with that button / count (`tripleclick` selects a line of text) |
| `hold <key> [--ms N] [--confirm <label>]` | hold a key on whatever has focus (default 500 ms): `focus` or `click` the control first |
| `scrollto <ref>` | scroll a control into view |
| `zoom in\|out\|reset\|<factor>` | page zoom |
| `options <ref>` | a `<select>`'s options, one per line, `*` on the selected ones |
| `text` | the page's visible text |
| `read` | the page as markdown (headings, paragraphs, list items, links) |
| `html` | the page's HTML, field values removed |
| `find [text] [--role R]` | controls whose label contains the text, as `@eN role "label"` lines |
| `pdf <path> [--landscape] [--background] [--scale N]` | save the page as a PDF |
| `tab new [url]`, `tab switch <tab>`, `tab close <tab>` | open, switch to or close a tab; `<tab>` is the id `tabs` prints; later verbs act on the tab you are on |
| `eval <script>` | evaluate JavaScript in the tab and print the result |
| `console`, `requests` | console entries / requests since your last read; the first call starts recording, so call it before the step you want to see |
| `check <ref>`, `uncheck <ref>` | set a checkbox (a pointer press; `--script` for a DOM script click) |
| `focus <ref>` | focus a control (ticketed) |
| `keyboard type\|insert <text> [--confirm <label>]` | keys into whatever has focus (`insert` sends the text in one go); refused off `--allow-domain`; with `--confirm-press` set, an Enter / space on a focused matching button, Enter in a field whose form submits through one, or Enter / space when no control is seen focused (`(unobserved focus)`) needs `--confirm`; an Enter after a Tab in the same text, or a space right after one, is refused: send it in its own call |
| `get value\|label\|role\|box <ref>`, `get attr <ref> <name>` | read one fact; never a password value |
| `is visible\|enabled\|checked\|editable <ref>` | prints `true` or `false` |
| `count [--label L] [--role R]` | how many controls match, and their refs |
| `state save\|load <path>` | save / reuse a login (cookies + storage, 0600 file); load prints origins still pending |
| `cookies [list] [--name N] [--domain D] [--reveal]` | cookie values print as `•••` unless `--reveal` |
| `cookies set <name> <value> [--domain D] [--path P] [--expires SECS] [--http-only] [--secure] [--same-site Strict\|Lax\|None]`, `cookies clear [--name N] [--domain D]` | change cookies |
| `storage local\|session [list] [--reveal]`, `… get <key>`, `… set <key> <value>`, `… clear [<key>]` | the page origin's storage |
| `set viewport W H [scale]`, `set device "<name>"`, `set media dark\|light\|no-preference`, `set offline on\|off`, `set headers K=V…`, `set reset` | emulate a screen / device / color scheme / network |
| `download <ref> <path> [--overwrite] [--timeout-ms N]` | ticketed click, then save the file |
| `network block <pattern>…`, `network unblock [<pattern>…]`, `network har start`, `network har stop <path>` | block URLs; record a HAR (no bodies, secrets masked) |
| `close` | stop the daemon (and the Chrome it launched) |

`click`, the other click verbs, `drag`, `upload`, `hold` and `keyboard` take
`--confirm`. If `key`, `check`, `focus` or
`download` refuses `NeedsConfirm "<label>"`, ask the user, then press that
control with `aui click <ref> --confirm "<label>"`.

`--max-output N` on any verb cuts long text output (`snap`, `text`, `read`,
`html`, `console`, `requests`) to N chars.
`--boundary` on a reading verb (or `AUI_CONTENT_BOUNDARIES=1` for all of
them) prints the page content between `<<<page-content <id>>>>` and
`<<<end page-content <id>>>>`: see `aui skills get core --ref safety`.
Relative paths are yours: they resolve against your cwd.

- `--snap` on an input verb (`click`, `type`, `select`, `key`, `check`,
  `uncheck`, `focus`, `keyboard`, `open`, `scroll`, `forward`, `reload`,
  `download`) prints the page after it, like `aui snap`, and the next refs
  come from that look: one call instead of two.
- State after the label: `value="…"`, `checked` / `unchecked`, `expanded` /
  `collapsed`, `selected` (aria-selected: the selected tab), `current=page`
  (aria-current, also `step`, `date`, ...), `selected="…"` (a select's
  option). Password fields never show a value.
- `aui snap --json` gives the `elements[]` JSON instead.

Several steps at once: `aui batch "snap" "click @e5" "wait --text Done" --bail`
runs them in order on one session (`--bail` stops at the first failure;
`--json` prints `[{cmd, ok, output}]`; with no commands it reads a JSON array
of strings on stdin). Exit is non-zero if any failed. `aui session` prints the
session name in use; `aui session list` lists live sessions and removes stale
sockets.

