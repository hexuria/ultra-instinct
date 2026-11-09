# ADR 0013: CDP over a caller-owned pipe

- Status: accepted
- Date: 2026-11-09 (Asia/Manila)
- Relates: AGENTS.md anti-drift rule 10; ADR 0012 (session daemon)

## Context

A Linux box runs Chromium with `--remote-debugging-pipe` and, on purpose, no
DevTools port: nothing on the box can reach the browser except the trusted
daemon that launched it and holds the other end of the pipe. The daemon is
async (tokio), but it will run our code on a dedicated thread, so blocking is
fine. Every transport we had (`WebSocketTransport`, `open_tab`) needs a port.

## Decision

`ultra-instinct-cdp` gains `PipeBrowser` / `PipePage` (`src/pipe.rs`,
`cfg(unix)` only).

- **Caller-owned ends.** `PipeBrowser::new(reader, writer)` takes anything
  that converts into an `OwnedFd` (`File`, `OwnedFd`, `std::io::PipeReader`
  / `PipeWriter`, `ChildStdout` / `ChildStdin`). It never spawns Chrome.
- **Framing.** Chrome reads its fd 3 and writes its fd 4: one UTF-8 JSON
  message per frame, ended by a NUL byte. `FrameSplitter` cuts the stream.
  Envelope decode (raw-value fields, `error` → `Protocol`) is the websocket
  transport's, shared inside the crate.
- **Browser-level pipe, flat sessions.** `page(target_id)` sends
  `Target.attachToTarget {flatten: true}` and keeps the `sessionId`; a page's
  calls carry it, and its `drain_events` sees only its session. Events with
  no `sessionId` queue on the browser; events for a session this browser did
  not attach are dropped. Each queue keeps the newest 1024 events.
- **One thread, pump-on-call.** No reader thread, no atomics, no locks
  (rule 10 holds). The browser and its pages share the connection through
  `Rc<RefCell<..>>`, so they are `!Send` / `!Sync`: one thread owns them
  all. The caller reads frames until its own id answers. Ids are unique
  across the pipe and only grow, so any other response belongs to an
  abandoned call and is dropped.
- **Deadlines without threads.** The wait is `rustix::event::poll` (safe) on
  the read end, bounded by the call deadline. `forbid(unsafe_code)` stays.
  A timeout leaves the pipe usable.
- **Writes interleave reads.** The write end is set `O_NONBLOCK`
  (`rustix::fs::fcntl_setfl`, safe; on a socket dup shared with the read
  end the flag is shared, so reads take `WouldBlock` too). A request is
  written in pieces while one `poll` watches both ends; frames Chrome
  writes meanwhile are read and routed. So a Chrome blocked writing events
  to us while we write a large request to it cannot deadlock the pair. The
  write is bounded by the call deadline (`Timeout`); a request cut off part
  way leaves the stream unframed, so the pipe is then closed for later
  calls (`PipeClosed`). A timeout before any byte left keeps it usable.
- **Same early exits as the websocket.** `Page.javascriptDialogOpening` for
  the calling session ends the wait with `DialogOpened` (the event stays
  queued). `Target.detachedFromTarget` / `Target.targetDestroyed` make the
  page's calls return `TargetGone`, including one already waiting. Dropping
  a live page sends `Target.detachFromTarget` without waiting.
- **One bad frame costs one frame** (revised; the first draft failed
  closed). JSON never holds a raw NUL, so the stream re-syncs at the next
  one. A frame over the cap is skipped without being buffered past the cap;
  if its first bytes name the waiting call's id (`{"id":N,`, which Chrome
  writes first) that call gets `FrameTooLarge { limit }`, else (an event,
  an abandoned call's reply, no readable id) it is dropped and counted
  (`PipeBrowser::dropped_frames`). The same holds for a frame that is not
  JSON: it answers the waiting call (`BadJson`) only when its leading id is
  that call's. The pipe and every other tab stay usable; one huge reply no
  longer kills them all. EOF or a read / write error (`PipeClosed`) still
  kills the pipe; every later call on the browser and its pages returns
  that error.

Limits (`PipeLimits`): `max_frame_bytes` 64 MiB (the websocket's message
cap; screenshots fit), `call_deadline` 30 s (the websocket's; it covers
the write and the wait for the answer). Buffering is bounded by the frame
cap plus one 64 KiB read, oversize frames included.

Owners: `crates/ultra-instinct-cdp/tests/pipe.rs` (fake Chrome on
`std::io::pipe()`, chunking proptests), the `pipe_frame` fuzz target, and the
ignored live test `tests/pipe_live.rs`, which launches Chrome through
`/bin/sh` to place the pipes at fds 3 / 4 (std cannot remap fds without a
`pre_exec` hook).

## Phase 2: embedding over the pipe

A host that runs one OS thread per screen owns a `PipeBrowser` and an
`ultra_instinct_mcp::Server` on that thread and calls `Server::call_tool`
directly; there is no stdio and no DevTools port. How to embed:
[`docs/embedding.md`](../embedding.md). The `ultra-instinct-embed` crate the
first draft named is not needed: `ultra-instinct-mcp` is the embedding
surface.

- **Tab backend.** `ultra-instinct-cdp` gains `TargetBackend` (list /
  create / activate / close pages, connect a session key). `HttpBackend`
  is the old path (Chrome's `/json` endpoints plus a connector) and stays
  the default; `PipeBackend` drives `Target.*` on a `PipeBrowser` and
  attaches flat `PipePage`s keyed `pipe:<targetId>` (focus emulation on, as
  the websocket connect does). It turns on `Target.setDiscoverTargets` once,
  so `Target.targetDestroyed` reaches pages, and hands browser-level
  `Browser.download*` events to the tab that drains. `Server::with_pipe`
  (`with_backend` for others): `cdp` is ignored in pipe mode (an internal
  re-call passes it, so refusing it would break those), the first call binds
  the first page, and observe / guard / verify read the bound tab.
  `drain_events` stays per session, so dialog, console, network, HAR and
  download readers work unchanged.
- **Snapshot-bound presses.** `browser_get_state` records what it printed
  (both formats) and returns `snapshot`. An index press given `snapshot`
  resolves the index among the elements that snapshot showed (an index
  past `max_elements` is `unknown_ref`) and mints its ticket there; the
  existing boundary then refuses on the fresh observation
  (`execute_ticketed`'s observe + `revalidate`, or `revalidate` + the hard
  gate before `consume_ticket_once`), as `stale_ref`, with nothing pressed.
  Focus is the one world fact taken from the page now: the caller's own
  typing moves it. A bare index keeps the old behaviour (fresh observation)
  unless `Server::require_snapshot(true)`, which refuses it
  (`snapshot_required`). No new check: the refusal is `revalidate`'s.
- **Error codes.** Every `ToolError` value carries a stable snake_case
  `code`. Policy refusals, stale refs, unknown indexes and CDP pipe /
  timeout / target-gone / dialog failures are typed variants (the code comes
  from the type, never from parsing text); their `variant` and `message`
  are what they were. `CdpError::Timeout` replaces the timeout's free-text
  `Transport` error (same wording). The agent path reports executor
  failures as text, so the server's session transports note the typed CDP
  failure they hit and the call's generic `browser` error is swapped for it.
  A best-effort call whose error the tool discards (`browser_download`'s
  restore of the download behaviour, `browser_keyboard`'s focus re-read)
  runs under `best_effort`, which leaves the note as it was: a download
  that failed on its own reports its own error, not the restore's timeout.
- **Secrets.** A field is `sensitive` when it is a password / hidden / file
  input, its label names a secret (AGENTS.md 7a, `is_secret_field`), or its
  type could not be read. Its value is never printed; text typed into it
  (any length; under 4 chars only as a whole token) is masked (`••••`) in
  every later output, error and HAR. Masking is one exit,
  `Server::call_tool`, for every tool; only the screenshot `base64` and
  PDF `data` fields are exempt. Sensitive values are never evicted: past
  64 a new one refuses `secrets_full` before it is typed. `run_goal`
  remembers each payload before dispatch (non-sensitive ones from 4
  chars) and returns them in `RunReport::typed` (must not be logged).
  `browser_get_html` drops text inputs' `value` attributes. Observe no
  longer names a text area by its own text (its value).
- **Limits.** `Server::with_limits(ToolLimits)`: state text 100 000 chars,
  1 000 elements (observe's regions too), 16 MiB screenshots (one JPEG
  retry, then `output_too_large`), 60 s waits, 1 000 000 chars of
  `browser_get_text` / `browser_get_html` (cut inside the page script, so
  a huge page rarely makes a frame near the cap).
- **Paths.** Screenshot, PDF, state, download and HAR paths must be
  absolute (`path_not_absolute`); nothing invents a default.
  `Server::with_downloads(Anywhere | Off | Dir)`.
- **Allow-list.** `Server::with_tool_allowlist`: other tools refuse
  `tool_not_allowed` and `tools/list` shows only the allowed ones.
  `Server::run_goal` is gated by the name `run_goal` (not a tool, never
  listed): absent from a set list, the run ends `Failed` with
  `tool_not_allowed: …` before any CDP call. One name, not the set of press
  tools the loop could use: the loop presses through its own executor and
  may click, type, select, hover, drag, scroll or press keys on any page.
- **Runtime policy and the loop.** `Server::set_policy` between calls; a
  `confirm` lifts its own call only. `ultra_instinct_mcp::run_goal` runs the
  agent loop (InstinctPolicy, deterministic text, no model) under a guard
  `Policy`, with a step cap and a deadline checked between steps
  (`AgentBuilder::deadline`).

Owners: `crates/ultra-instinct-mcp/tests/pipe_mode.rs`, `snapshot_refs.rs`,
`secrets.rs`, `host_options.rs`, `run_goal.rs`, the ignored live
`pipe_live.rs`; `crates/ultra-instinct-browser-tools/tests/error_codes.rs`.

## Consequences

- No new threads or shared state on the agent path; the pipe is one more
  `CdpTransport`.
- Not available on non-unix targets.
- Chrome only answers what the caller asks: target discovery events
  (`Target.targetDestroyed`) need `Target.setDiscoverTargets` from the caller.
