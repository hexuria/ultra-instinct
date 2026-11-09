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
- **Same early exits as the websocket.** `Page.javascriptDialogOpening` for
  the calling session ends the wait with `DialogOpened` (the event stays
  queued). `Target.detachedFromTarget` / `Target.targetDestroyed` make the
  page's calls return `TargetGone`, including one already waiting. Dropping
  a live page sends `Target.detachFromTarget` without waiting.
- **Fail closed.** A frame over the cap (`FrameTooLarge`), EOF or a read /
  write error (`PipeClosed`) kills the pipe; every later call on the browser
  and its pages returns that error.

Limits (`PipeLimits`): `max_frame_bytes` 64 MiB (the websocket's message
cap; screenshots fit), `call_deadline` 30 s (the websocket's). Buffering is
bounded by the frame cap plus one 64 KiB read. Writes are plain blocking
writes; the deadline covers the wait for the answer.

Owners: `crates/ultra-instinct-cdp/tests/pipe.rs` (fake Chrome on
`std::io::pipe()`, chunking proptests), the `pipe_frame` fuzz target, and the
ignored live test `tests/pipe_live.rs`, which launches Chrome through
`/bin/sh` to place the pipes at fds 3 / 4 (std cannot remap fds without a
`pre_exec` hook).

## Phase 2 (not in this change)

A tab backend in the tool layer that drives `PipePage`s in place of the
websocket tabs, and a new `ultra-instinct-embed` crate the daemon links to
hand its pipe ends to the agent on its own thread.

## Consequences

- No new threads or shared state on the agent path; the pipe is one more
  `CdpTransport`.
- Not available on non-unix targets.
- Chrome only answers what the caller asks: target discovery events
  (`Target.targetDestroyed`) need `Target.setDiscoverTargets` from the caller.
