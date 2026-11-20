# Plan: close the gap with agent-browser

Source: a side-by-side read of this repo (main `a3e90d0`) and
`vercel-labs/agent-browser` v0.43.0. Tracking issue: [#137](https://github.com/hexuria/ultra-instinct/issues/137).

This plan changes as we build. Edit it in the PR that changes scope, and
tick the matching box in the tracking issue when an item merges.

## Goal

> A person or agent with no Rust toolchain installs `aui` with one command on
> macOS or Linux, follows a README quick start of six commands or fewer, and
> can do from the shell everything the MCP tools can do, with every press
> still going through the hard gate and a one-shot `ActionTicket`.

The goal is met when every item below is merged or struck out with a
reason, and the acceptance run at the end passes on a clean machine.

To drive it, give an agent this prompt:

```text
Work through docs/plans/agent-browser-parity.md in hexuria/ultra-instinct.
Take the first unticked item whose "needs" are merged. One item is one PR.
Follow AGENTS.md and the PR template, run the item's done-check, then tick
its box in the tracking issue and in the plan. Stop and ask before any item
marked "ask first". Repeat until the acceptance run passes.
```

## Rules for every item

- One item, one PR, named with the item id (`P1.2: ...`).
- No item weakens the gate, ticket, executor, world, policy or text
  boundaries (AGENTS.md anti-drift block). A new verb is a wrapper over an
  existing ticketed tool.
- A PR that needs a crate ceiling raised stops and asks first.
- "Done" is the item's done-check passing, not CI alone.

## Phase 1: install without Rust

| Id | Item | Done-check | Needs |
|---|---|---|---|
| P1.1 | Release workflow: on a `v*` tag, build `aui` + `ultra-instinct` for macOS arm64 / x64 and Linux x64 / arm64 (glibc and musl), attach archives and checksums to a GitHub release | A test tag produces a release with every archive; each binary prints `aui --version` on its platform | |
| P1.2 | `scripts/install.sh`: download the right release binary, verify its checksum, fall back to the source build | On a machine with no `cargo`, `curl ... \| sh` then `aui doctor` passes | P1.1 |
| P1.3 | `aui upgrade`: replace the binary with the latest release, refuse on a checksum mismatch | Old binary upgrades in place; a tampered archive refuses | P1.1 |
| P1.4 | CI on `macos-15` for build + tests (Linux only today) | The macOS job is green and required | |
| P1.5 | `CHANGELOG.md` with a release-notes section the release workflow reads | The release body equals the changelog entry | P1.1 |
| P1.6 | Default decider is `instinct` when no model key is set, so a first run works offline (ask first: changes the `jev` default) | `aui run --launch --url ... --goal ...` runs with no key and no flag | |

Out of scope here, ask first: npm / Homebrew packages, Windows (the session
daemon is unix-socket only).

## Phase 2: shell verbs match the MCP tools

| Id | Item | Done-check | Needs |
|---|---|---|---|
| P2.1 | Parity test: every `browser_*` tool has an `aui` verb or an entry in a "no verb, because" list | The test fails when a tool is added with neither | |
| P2.2 | Verbs `back`, `hover <ref>`, `drag <ref> <ref>`, `upload <ref> <path>...` | Each runs against a live page; `drag` and `upload` refuse on a stale snap | P2.1 |
| P2.3 | Verbs `text`, `read` (extract content), `html`, `find`, `pdf <path>` | Output matches the MCP tool for the same page | P2.1 |
| P2.4 | Verbs `tab new\|switch\|close`, `eval <script>`, `console`, `requests` | Same | P2.1 |
| P2.5 | Verbs for the remaining clicks and keys: `dblclick`, `rightclick`, `hold`, `scrollto`, `zoom`, `options <ref>` | P2.1's "no verb" list is empty or each entry has a reason | P2.1 |
| P2.6 | `docs/tools.md` and the `core` skill list the new verbs | `aui skills get core` names every verb | P2.2 to P2.5 |

## Phase 3: remove

| Id | Item | Done-check | Needs |
|---|---|---|---|
| P3.1 | Drop the alias tools `browser_form_input`, `browser_get_page_text`, `browser_javascript_exec`, and `browser_search` (ask first: breaks MCP callers that use those names) | Tool count and docs agree; calling a dropped name returns the unknown-tool code | |
| P3.2 | Move `aui observe / guard / verify` and `replay / lessons / exams` out of the top-level help into one group each | `aui --help` fits one screen; old spellings still work or print where they went | |
| P3.3 (done) | Delete `fixtures/*.browser-use.json` and `*.cua.json` if no test or bench reads them | `cargo nextest run --workspace` and the bench smoke pass without them | |
| P3.4 | Fold `ultra-instinct-geometry`, `-protocol` and `-observe` into their one natural owner (ask first: they are published on crates.io and `scripts/architecture.txt` names them) | Architecture check passes; public paths re-exported or the break is in the changelog | |
| P3.5 | Replace the README hero image with one this repo owns, or drop it | README renders with no third-party link | |

## Phase 4: improve

| Id | Item | Done-check | Needs |
|---|---|---|---|
| P4.1 | README usage-first: install, the six-line quick start, safety, links. Diagrams and the crate table move to `docs/architecture.md` | README is under 150 lines and its quick start runs as written | P1.2 |
| P4.2 | AGENTS.md becomes an index; each anti-drift rule moves to `docs/boundaries/<name>.md` unchanged in meaning | Every rule number still resolves; `check_repo_rules.sh` passes | |
| P4.3 | Skill: `skills/ultra-instinct/SKILL.md` is a thin stub that says to run `aui skills get core`; add `aui skills get core --ref <name>` for commands, safety, sessions | `aui skills list` shows the references; the stub has no verb details | P2.6 |
| P4.4 | Live Chrome smoke job in CI: a small set of the `#[ignore]` tests against headless Chrome | The job is green on main and fails when a verb is broken on purpose | |
| P4.5 | Split the verb layer out of the CLI crate (the ceilings file already names it) | Both crates are under their ceilings with no ceiling raised | P2.5 |

## Phase 5: new features

Each is a design note in this file first, then a PR. All are "ask first".

| Id | Item | Done-check |
|---|---|---|
| P5.1 | Content boundary markers around page text in tool output | Page text that imitates a tool result stays inside the markers; the marker cannot be forged from the page |
| P5.2 | `aui diff`: what changed between two snaps | Reports appeared / disappeared / changed controls on a fixture pair |
| P5.3 | Session state auto save and restore per session name | Log in once, `aui close`, reopen: still logged in; state file is 0600 |
| P5.4 | Proxy flag on `--launch` | Requests go through a local test proxy |
| P5.5 | Session recording (video or frame sequence) | A run produces a playable file; secret fields are hidden in it |
| P5.6 | Action policy file: allow / deny by tool name per project | A denied tool refuses with a stable code |

## Not planned

These exist in agent-browser and are left out on purpose. Move one up only
with a written reason.

- CSS / XPath selectors and coordinate clicks: they bypass the snapshot-bound
  ref and the ticket.
- Cross-origin iframe and closed-shadow coverage (anti-drift 7b).
- Cloud browser providers, other engines, iOS Safari.
- Dashboard, live stream, chat, plugins, WebMCP.
- Request interception through Fetch (`browser_network` blocks URLs only).

## Keep

No item may remove or weaken: one-shot tickets with fresh-observe
revalidation, `stale_ref` refusal, the `aui run --goal` loop with Instinct,
closed model replies, confirm-before-press, the domain allow-list with the
form-target check, fail-closed secret masking, own-tabs-only `--attach`, the
embeddable `Server`, and the proptest / fuzz / mutants / architecture checks.

## Acceptance run

On a clean macOS machine and a clean Linux machine, with no Rust installed:

1. Run the one-line install from the README.
2. `aui doctor` passes.
3. Run the README quick start exactly as written.
4. Run one command per verb in `docs/tools.md` against `examples/`.
5. Change the page between `aui snap` and `aui click`: the click refuses.
6. `aui run --launch --url <example> --goal 'Type "rust" into Search'` ends DONE with no model key set.
