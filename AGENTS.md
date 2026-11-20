# ultra-instinct

The product is `ultra-instinct`; crates are `ultra-instinct-*` (Autonomous Ultra
Instinct) and binaries are `ultra-instinct` plus its short alias `aui`.
The HGRA matcher experiment and the weighted matcher are removed. A crate,
binary or feature
named `hgra` or `resonance` is a bug.

**Product:** Ultra-Instinct is a **Rust-native browser-agent runtime**.
The library `Agent` **owns the loop**: it observes the page, builds a finite
`ActionSpace`, lets a policy choose (Instinct by default; a remote decider —
JEV, Clef, or the `TEXT_MODEL_*` model — only when chosen with `--decision`),
hard-gates the choice, executes only through a one-shot `ActionTicket`, then
re-observes, diffs, and verifies.

MCP (`ultra-instinct mcp`) is an **optional adapter**, not the orchestration
surface. Nothing on the product path requires MCP, Browser Use, CUA, or Jev.

Public API is 0.1 and unstable until 1.0. Toolchain pin: Rust 1.99.0
(`rust-toolchain.toml`). Product crates publish to crates.io as
`ultra-instinct-*`. `#![forbid(unsafe_code)]` on every crate.

## Anti-drift (read before changing anything on the agent path)

The anti-drift block covers the Agent + Instinct + ticket path (impeccable
audit, baseline `87ffc2d`). Each rule is one file; read the ones your change
touches before you start. Rule numbers are stable: code, CI and PRs cite
them as "anti-drift #N".

| # | Rule |
|---|---|
| 1 | [The agent owns the loop](docs/boundaries/01-agent-owns-the-loop.md) |
| 2 | [Instinct owns HOW, Ultra-Instinct owns WHAT](docs/boundaries/02-instinct-how-ultra-instinct-what.md) |
| 3 | [Hard invalidity is guard evidence, not a score](docs/boundaries/03-hard-gate.md) |
| 4 | [One executor boundary, one staleness barrier](docs/boundaries/04-executor-and-staleness.md) |
| 5 | [Stale ≠ spent](docs/boundaries/05-stale-is-not-spent.md) |
| 6 | [Ticket worlds are target-scoped](docs/boundaries/06-ticket-worlds.md) |
| 7 | [Text is not Instinct](docs/boundaries/07-text.md) |
| 7a | [Model text is payload-only and grounded](docs/boundaries/07a-model-text.md) |
| 7b | [Harder page types](docs/boundaries/07b-page-types.md) |
| 8 | [Remote escalation is explicit and closed](docs/boundaries/08-remote.md) |
| 9 | [No matcher](docs/boundaries/09-no-matcher.md) |
| 10 | [One blocking CDP socket per agent](docs/boundaries/10-one-socket.md) |

Any change to observable semantics on this path names the boundary it affects (gate, ticket, executor, world, policy, text) and the test that owns it.

## Crates (product graph)

| Crate | Owns |
|---|---|
| `ultra-instinct-core` | `InteractionManifold`, regions, `ActionSpace`, fixture grammar; modules `geometry` (viewport geometry helpers, `RegionState`), `observe` (observation history, id diff) and `protocol` (guard / ticket / verify wire types) |
| `ultra-instinct-cdp` | CDP transport trait, websocket, replay transport, replay script builder; unix session-multiplexing browser `CdpBrowser` / `CdpPage` (old names `PipeBrowser` / `PipePage`; one thread) over a crate-private `Link`: the `--remote-debugging-pipe` (caller-owned fds) or one browser websocket (`CdpBrowser::connect_websocket`, `ws://` only, `ACCEPT_WAIT` 90 s for Chrome's allow prompt); tab backends: `TargetBackend` (list / create / activate / close pages, connect a session key), `HttpBackend` (the `/json` endpoints + a connector, the default) and unix `SessionBackend` (`Target.*` on a `CdpBrowser`; `new` = `PipeBackend`, keys `pipe:<targetId>`; `attached`, keys `attach:<targetId>`, own-tabs-only: lists, activates, closes and connects only the tabs it opened (each `newWindow: true`, not activated; any other target is `CdpError::NotOwned`), closes those on drop, never `Browser.close`) |
| `ultra-instinct-chrome` | Find / start a local Chrome (`find_chrome`, `launch`, `ChromeProcess` killed on drop; headless at `DEFAULT_WINDOW` 1440×900, headed maximized, `LaunchOptions::window_size` overrides); `attach_endpoint` / `find_attach_endpoint` (the browser websocket a user's remote-debugging toggle wrote to `DevToolsActivePort`, Chrome stable's dir first); std only, re-exported as `ultra_instinct_browser::launch` |
| `ultra-instinct-browser` | CDP observe (DOM/AX fusion, identity, stacking), raw CDP inputs; `launch` (re-export of `ultra-instinct-chrome`); `storage_state` (save / load cookies + storage, the 0600 state file and its parser) |
| `ultra-instinct-policy` | `BrowserPolicy` + `PolicyContext` (`set_situation`, default no-op), `InstinctPolicy` (`instinct-*` 0.1.0 from crates.io) + `TRUST_CAP_MILLIS`/`set_evidence_adjustments` (bounded learned-trust evidence), `TextResolver`, `ModelTextResolver` (feature `model-text`), `RemotePolicy` (feature `remote`), multi-step clause split |
| `ultra-instinct-guard` | Hard `gate`, `ActionTicket` issue / `revalidate`, `TicketLedger`, `consume_ticket_once` / `consume_ticket_once_checked`, front layer + `WorldSnapshot`; the user's `Policy` (`HostPattern` allowed domains, `LabelGlob` confirm-before-press); exact-label `guard()` for MCP / CLI preflight |
| `ultra-instinct-agent` | `Agent` state machine, `execute_ticketed` / `execute_ticketed_with` (policy), `AgentBuilder::guard_policy`, `AgentBuilder::deadline` (checked between steps), `BrowserRuntime`, `BeforeDispatch` (forwards every `BrowserRuntime` method and runs a hook just before `dispatch`; handed only to `execute_ticketed_with`), verification mapping, `MockBrowser` |
| `ultra-instinct-browser-tools` | The `browser_*` host surface as MCP tool families (`call` / `spec`, the tool-name lists, `ToolError` with a stable `code` on every variant) over a `BrowserHost` trait (sessions, tab binding, ledgers, events, snapshot record and lookup, `require_snapshot`, `Policy`, the tab `backend`, `ToolLimits`, `Downloads`, `Uploads`, the tools' own `ToolState`): snapshot-bound index refs (`bound.rs`), secret masking (`secrets.rs`: `sensitive`, typed secrets scrubbed from every output), browser-use names, `browser_wait` conditions, `browser_extra` (dialog, forward / reload, check, focus, keyboard, get / is / count, annotate, `max_chars`), `browser_state` (state save / load, cookies, storage), `browser_env` (set, download, network) |
| `ultra-instinct-mcp` | Optional JSON-RPC adapter (observe / guard / verify), `TOOLS`, and the `Server` that keeps sessions and the snapshot ring and serves the `browser_*` tools through a crate-private `BrowserHost` adapter (`Server` itself does not implement it); host options (`with_pipe` / `with_backend`, `require_snapshot`, `bind_to_latest_look`, `set_policy`, `with_limits`, `with_downloads`, `with_uploads`, `with_tool_allowlist`); `run_goal` / `run_goal_on` / `Server::run_goal` (the agent loop with InstinctPolicy and the deterministic resolver, no model); re-exports `ToolError`, `Policy` and the tool-name lists ([`docs/embedding.md`](docs/embedding.md)) |
| `ultra-instinct-remote` | Remote decision transports and the HTTP text model, the only crate with a model SDK / HTTP client: `typesafe` (feature `jev`), `clef` (feature `clef`), `http_text` (feature `model-text`, OpenAI-compatible / Anthropic / Gemini, with an optional JPEG), `decide` (feature `model-text`: `ModelDecider`, the same model choosing every step over the closed wire); re-exported by `ultra-instinct` |
| `ultra-instinct` | `ultra-instinct run` (agent loop, `--decision instinct|jev|clef|clef-flash|dojo|model` (`--policy` is an alias), `--diary <dir>`); `replay --diary` (offline arena: agree / would-abstain / regress per recorded decision); `observe` / `guard` / `verify` preflight commands; `mcp` (stdio); dispatch of the shell verbs, `batch` and `session` to `ultra-instinct-verbs`; `skills`, `doctor`, `install`; `run --launch/--profile/--load-state/--save-state/--allow-domain/--confirm-press/--confirm`; `TypesafeTransport` (feature `jev`: JEV-primary over the closed remote wire); `ClefTransport` (feature `clef`: Cloudflare Workers AI Clef / Clef-Flash over the same wire through `typesafe-sdk`'s Clef provider, one decision call, single-candidate heads answered locally) |
| `ultra-instinct-verbs` | The `aui` shell verbs (`verbs.rs`: one verb per `browser_*` tool family, parse and render), the per-session daemon and its client (`daemon.rs`: socket, own Chrome, session list), `batch` (shell-style splitter), `CliError`, and the `/json/version` probe; used only by `ultra-instinct` |
| `ultra-instinct-dojo` | Battle diary schema v1 (`DiaryLine` JSONL), `DiaryWriter` / `read_diary`, site + situation keys, decision→`ActionSpace` rebuild for the replay arena; versioned `LessonStore` (words / places / moves / trust, keyed per situation, carrying proving diary ids); lessons only adjust evidence inside the finite `ActionSpace` |

Instinct (`instinct-core` / `-text` / `-lexicon`) is pinned to 0.1.0 from
crates.io in the workspace `Cargo.toml` (`INSTINCT_VERSION_REQ`); bump only with a
deliberate eval.

## Verification owners

Which unit test, proptest, fixture, fuzz target or CI job owns which part,
and how to run each: [`docs/verification.md`](docs/verification.md).

## MCP (optional adapter)

The MCP server, its tools, the host options and the shell verbs:
[`docs/boundaries/mcp.md`](docs/boundaries/mcp.md). Every tool and `aui`
verb, one line each: [`docs/tools.md`](docs/tools.md).

## Signals

Signals (`no-op`, `loop-detected`) are data for the MCP host
journal. They do not retry and do not select an executor.
