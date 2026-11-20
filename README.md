# ultra-instinct

**A browser an agent can drive from the shell, where a press cannot land on
the wrong thing.** `aui` reads a page as a short list of controls (`@e1`,
`@e2`, ...) and presses one only through a one-shot ticket that is checked
again on a fresh look at the page: if the page changed since you looked, the
press refuses. It can also take a goal and do the steps itself, with no model
and no key.

## Install

```bash
curl -fsSL https://raw.githubusercontent.com/hexuria/ultra-instinct/main/scripts/install.sh | sh
```

This downloads the release binary for this machine (macOS or Linux, x64 or
arm64), checks it against the release's `SHA256SUMS` and puts `aui` and
`ultra-instinct` (the same program) in `~/.local/bin`. No Rust toolchain is
needed. `AUI_INSTALL_DIR` changes the folder, `AUI_VERSION=<tag>` pins a
release, and `aui upgrade` replaces the binary with the latest release later.

`aui` needs a Chrome or Chromium on the machine. `aui doctor` says whether it
found one; if not, `aui install --run` downloads Chrome for Testing
(`ULTRA_INSTINCT_CHROME=<path>` picks a binary yourself). To build from
source instead, run `scripts/install-aui.sh` in a checkout.

## Quick start

```bash
aui open --launch 'data:text/html,<input aria-label="Name"><button onclick="document.title=`Hi `+document.querySelector(`input`).value">Greet</button>'
aui snap             # @e1 textbox "Name"  value=""   @e2 button "Greet"
aui type @e1 "Ada"   # ✓ ticket 1 · success
aui click @e2        # ✓ ticket 2 · state-changed
aui snap             # the title is now "Hi Ada"
aui close            # stops the session and its Chrome
```

`open --launch` starts a private headless Chrome for the session; any URL
works in place of the test page. `snap` lists the controls; the other verbs
take the `@eN` refs from your latest `snap`. Change the page between `snap`
and `click` and the click refuses with `run aui snap` instead of pressing.

## Hand over a goal

```bash
aui run --launch --url https://example.com --goal 'click "Learn more"'
```

The runtime observes, picks each step, presses through the same gate and
ticket, and checks the effect. With no flag the picker is Instinct: inside
the binary, no model, no tokens, same input same result. It handles plain
forms and abstains rather than guess on harder pages; when it abstains,
carry on with the verbs. `--decision model|jev|clef` lets a model pick
instead, only from the actions the runtime offers.

## What you can do

| Group | Verbs |
|---|---|
| Look | `snap`, `text`, `read`, `html`, `find`, `get`, `is`, `count`, `shot`, `pdf`, `console`, `requests` |
| Press | `click`, `dblclick`, `rightclick`, `type`, `select`, `key`, `check`, `uncheck`, `hover`, `drag`, `upload`, `download` |
| Move | `open`, `back`, `forward`, `reload`, `scroll`, `scrollto`, `wait`, `tab new\|switch\|close`, `tabs` |
| State | `state save\|load`, `cookies`, `storage`, `set viewport\|device\|media\|offline`, `network block\|har` |
| Many steps | `batch`, `run --goal`, `session list` |

Every verb and MCP tool, one line each: [`docs/tools.md`](docs/tools.md).
`aui --help` is one screen; `aui help <command>` has the flags.

- **Agents:** read [`skills/ultra-instinct/SKILL.md`](skills/ultra-instinct/SKILL.md),
  or run `aui skills get core` for the guide that ships in the binary.
- **MCP:** `aui mcp` is a stdio server with the same actions as 52 tools. A
  Claude Code subagent that uses it is in
  [`agents/screen-driver.md`](agents/screen-driver.md).
- **Embedding in a Rust host:** [`docs/embedding.md`](docs/embedding.md).

## Safety

- **Hard gate and tickets.** Disabled, readonly, hidden, occluded,
  offscreen or behind-a-modal targets never run. A press runs only through
  a one-shot ticket, revalidated on a fresh observation; a changed page
  refuses instead of pressing the wrong element.
- **Domain allow-list.** `--allow-domain shop.example` (or `*.shop.example`)
  allows input only on those hosts, checks navigation URLs first, and
  refuses a press that would submit a form to another host. `data:`,
  `javascript:`, `file:` and other non-web pages refuse too, and
  `ULTRA_INSTINCT_REFUSE_BLOB=1` refuses `blob:` pages as well.
- **Confirm before press.** `--confirm-press "Pay*"` blocks a matching press
  until the user's `--confirm "<label>"` (or a tool call's `confirm`, that
  call only). Enter / Space on a button and Enter in a form that submits
  through a matching button count as presses.
- **Snapshot-bound presses.** An MCP press given the `snapshot` id it was
  chosen from refuses `stale_ref` if that element moved, changed or left.
  Every `aui` verb press is bound to the latest `aui snap` the same way.
- **Secrets fail closed.** Password, hidden, file and secret-labelled fields
  never show a value; text typed into one is masked in every later output,
  error and HAR file; screenshots hide those fields or refuse.
- **Files and tools.** File paths must be absolute; a host can keep
  downloads and uploads to one folder and serve only an allow-list of tools.
- **Your own Chrome, by name only.** `aui open <url> --attach --allow-domain
  <host>` drives your running Chrome once you turn on
  chrome://inspect/#remote-debugging; it refuses without `--allow-domain`,
  works in a tab of its own (a new window; your tabs are never read or
  closed), never runs the cookie, state, storage or
  download tools (they reach your whole profile), and `aui close` leaves
  your Chrome open.
- **Closed model replies.** An optional remote policy may only pick an
  offered choice or abstain.

Report vulnerabilities as described in [`SECURITY.md`](SECURITY.md).

## More

- [`docs/architecture.md`](docs/architecture.md): the loop, the two ways to
  drive, the library API and the crates.
- [`AGENTS.md`](AGENTS.md): the rules for changing the agent path, indexed
  by boundary.
- [`CHANGELOG.md`](CHANGELOG.md): what each release changed.

## Development

```bash
cargo test --workspace
scripts/check_repo_rules.sh && python3 scripts/check_architecture.py
```

Live Chrome tests are `#[ignore]`; CI uses CDP replay fixtures plus one live
smoke job, which also runs the quick start above as written. The bench
harness in `bench/` compares browser tools and agents on local scenario
sites. Read [`AGENTS.md`](AGENTS.md) (the anti-drift rules and test owners)
before changing the agent path.

Rust 1.99.0. Crates are `ultra-instinct-*` 0.1.0; the API is unstable until
1.0. Every crate is `#![forbid(unsafe_code)]`.

## License

MIT, see [`LICENSE`](LICENSE).
