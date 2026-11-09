//! Blocking CDP over a caller-owned pipe pair (`--remote-debugging-pipe`).
//!
//! Chrome reads CDP from its fd 3 and writes to its fd 4: one UTF-8 JSON
//! message per frame, each frame ended by a NUL byte. The pipe is
//! browser-level: pages are reached with `Target.attachToTarget
//! {targetId, flatten: true}` and every page message carries `sessionId`.
//! This module never spawns Chrome; whoever launched it hands over the ends.
//!
//! **Threading.** One thread owns a [`PipeBrowser`] and every [`PipePage`]
//! made from it. They share the connection through `Rc<RefCell<..>>`, so all
//! of them are deliberately `!Send` and `!Sync`. There is no reader thread:
//! whoever calls pumps the pipe until its own id answers (pump-on-call),
//! routing events into per-session queues on the way. Message ids are unique
//! across the whole pipe and only grow, so a response for any other id
//! belongs to an abandoned call (timed out or cut short by a dialog) and is
//! dropped. Waiting uses `poll(2)` on the read end (through `rustix`), which
//! is why this module is `cfg(unix)` only.
//!
//! **Failing closed.** A frame over [`PipeLimits::max_frame_bytes`], EOF, or a
//! read / write error marks the pipe dead; every later call on the browser
//! and its pages returns the same error. A call that only timed out leaves
//! the pipe usable.
//!
//! Writes are plain blocking writes; the call deadline covers the wait for
//! the answer, not a peer that stops draining its end.

use std::cell::RefCell;
use std::collections::{HashMap, VecDeque};
use std::fs::File;
use std::io::{ErrorKind, Read, Write};
use std::os::fd::OwnedFd;
use std::rc::Rc;
use std::time::{Duration, Instant};

use rustix::event::{poll, PollFd, PollFlags, Timespec};
use serde_json::{json, Value};

use crate::ws::{
    created_target_id, deadline_error, event_params, frame_fields, raw_string, response_result,
    CALL_DEADLINE, DIALOG_OPENING, MAX_BUFFERED_EVENTS,
};
use crate::{CdpError, CdpEvent, CdpTransport};

/// Default cap on one pipe frame: 64 MiB, the websocket transport's message
/// cap (tungstenite's default), so a full-page screenshot fits.
pub const DEFAULT_MAX_FRAME_BYTES: usize = 64 << 20;

/// Bytes read from the pipe per `read(2)`.
const READ_CHUNK: usize = 64 << 10;

const DETACHED_FROM_TARGET: &str = "Target.detachedFromTarget";
const TARGET_DESTROYED: &str = "Target.targetDestroyed";

/// Limits for one pipe connection.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PipeLimits {
    /// Largest frame accepted, NUL excluded. Default [`DEFAULT_MAX_FRAME_BYTES`].
    pub max_frame_bytes: usize,
    /// How long one call waits for its answer. Default 30 s, the websocket
    /// transport's call deadline.
    pub call_deadline: Duration,
}

impl Default for PipeLimits {
    fn default() -> Self {
        Self {
            max_frame_bytes: DEFAULT_MAX_FRAME_BYTES,
            call_deadline: CALL_DEADLINE,
        }
    }
}

/// Splits a NUL-delimited byte stream into frames, refusing any frame over
/// a byte cap. Push each chunk read, then take frames until `Ok(None)`;
/// buffering stays within the cap plus one chunk.
///
/// After [`CdpError::FrameTooLarge`] the splitter is spent: it drops what it
/// held and every later [`FrameSplitter::next_frame`] returns that error.
#[derive(Debug)]
pub struct FrameSplitter {
    buf: Vec<u8>,
    /// Bytes of `buf` already known to hold no NUL.
    scanned: usize,
    max_frame_bytes: usize,
    failed: bool,
}

impl FrameSplitter {
    pub fn new(max_frame_bytes: usize) -> Self {
        Self {
            buf: Vec::new(),
            scanned: 0,
            max_frame_bytes,
            failed: false,
        }
    }

    /// Append bytes read from the stream (ignored once the splitter failed).
    pub fn push(&mut self, bytes: &[u8]) {
        if !self.failed {
            self.buf.extend_from_slice(bytes);
        }
    }

    /// The next complete frame without its NUL, `Ok(None)` when more bytes
    /// are needed.
    pub fn next_frame(&mut self) -> Result<Option<Vec<u8>>, CdpError> {
        if self.failed {
            return Err(self.too_large());
        }
        match self.buf[self.scanned..].iter().position(|&b| b == 0) {
            Some(offset) => {
                let end = self.scanned + offset;
                if end > self.max_frame_bytes {
                    return Err(self.fail());
                }
                let frame = self.buf[..end].to_vec();
                self.buf.drain(..=end);
                self.scanned = 0;
                Ok(Some(frame))
            }
            None => {
                self.scanned = self.buf.len();
                if self.buf.len() > self.max_frame_bytes {
                    return Err(self.fail());
                }
                Ok(None)
            }
        }
    }

    /// Bytes held for frames not yet complete.
    pub fn buffered(&self) -> usize {
        self.buf.len()
    }

    fn fail(&mut self) -> CdpError {
        self.failed = true;
        self.buf = Vec::new();
        self.scanned = 0;
        self.too_large()
    }

    fn too_large(&self) -> CdpError {
        CdpError::FrameTooLarge {
            limit: self.max_frame_bytes,
        }
    }
}

/// One `type == "page"` entry from `Target.getTargets`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PipeTarget {
    pub target_id: String,
    /// Always `"page"` here: other target types are filtered out.
    pub target_type: String,
    pub url: String,
    pub title: String,
}

struct Session {
    target_id: String,
    events: VecDeque<CdpEvent>,
    gone: bool,
}

/// The connection every handle shares: one reader, one writer, one id space.
struct Wire {
    reader: File,
    writer: File,
    splitter: FrameSplitter,
    chunk: Vec<u8>,
    limits: PipeLimits,
    next_id: i64,
    dead: Option<CdpError>,
    browser_events: VecDeque<CdpEvent>,
    sessions: HashMap<String, Session>,
}

/// What one frame meant to the call waiting on it.
enum Routed {
    Answer(Result<String, CdpError>),
    Continue,
}

impl Wire {
    fn call(
        &mut self,
        session: Option<&str>,
        method: &str,
        params_json: &str,
    ) -> Result<String, CdpError> {
        if let Some(err) = &self.dead {
            return Err(err.clone());
        }
        if let Some(gone) = session.and_then(|id| self.gone_error(id)) {
            return Err(gone);
        }
        let params: Value = serde_json::from_str(params_json).map_err(|err| CdpError::BadJson {
            message: err.to_string(),
        })?;
        let id = self.send(session, method, params)?;
        let deadline = Instant::now() + self.limits.call_deadline;
        loop {
            while let Some(frame) = self.next_frame()? {
                if let Routed::Answer(answer) = self.route(&frame, id, session, method) {
                    return answer;
                }
            }
            self.fill(deadline, method)?;
        }
    }

    /// Write one request frame; returns its id.
    fn send(
        &mut self,
        session: Option<&str>,
        method: &str,
        params: Value,
    ) -> Result<i64, CdpError> {
        if let Some(err) = &self.dead {
            return Err(err.clone());
        }
        let id = self.next_id;
        self.next_id += 1;
        let mut envelope = json!({"id": id, "method": method, "params": params});
        if let Some(session) = session {
            envelope["sessionId"] = Value::from(session);
        }
        let mut bytes = envelope.to_string().into_bytes();
        bytes.push(0);
        if let Err(err) = self
            .writer
            .write_all(&bytes)
            .and_then(|()| self.writer.flush())
        {
            return Err(self.die(CdpError::PipeClosed {
                message: format!("write: {err}"),
            }));
        }
        Ok(id)
    }

    fn next_frame(&mut self) -> Result<Option<Vec<u8>>, CdpError> {
        self.splitter.next_frame().map_err(|err| self.die(err))
    }

    /// Wait (bounded by `deadline`) for readable bytes and push them.
    fn fill(&mut self, deadline: Instant, method: &str) -> Result<(), CdpError> {
        loop {
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() {
                return Err(deadline_error(method, self.limits.call_deadline));
            }
            // Long waits poll in hour slices (macOS caps poll at `c_int::MAX` ms).
            let slice = left.min(Duration::from_secs(3600));
            let timeout = Timespec {
                tv_sec: slice.as_secs().try_into().unwrap_or(3600),
                tv_nsec: slice.subsec_nanos().into(),
            };
            let mut fds = [PollFd::new(&self.reader, PollFlags::IN)];
            match poll(&mut fds, Some(&timeout)) {
                Ok(0) => continue,
                Ok(_) => {}
                Err(rustix::io::Errno::INTR) => continue,
                Err(err) => {
                    return Err(self.die(CdpError::PipeClosed {
                        message: format!("poll: {err}"),
                    }));
                }
            }
            match self.reader.read(&mut self.chunk) {
                Ok(0) => {
                    return Err(self.die(CdpError::PipeClosed {
                        message: "end of stream".into(),
                    }));
                }
                Ok(n) => {
                    self.splitter.push(&self.chunk[..n]);
                    return Ok(());
                }
                Err(err)
                    if matches!(err.kind(), ErrorKind::Interrupted | ErrorKind::WouldBlock) =>
                {
                    continue;
                }
                Err(err) => {
                    return Err(self.die(CdpError::PipeClosed {
                        message: format!("read: {err}"),
                    }));
                }
            }
        }
    }

    fn route(&mut self, frame: &[u8], id: i64, session: Option<&str>, method: &str) -> Routed {
        let fields = match std::str::from_utf8(frame) {
            Ok(text) => match frame_fields(text) {
                Ok(Some(fields)) => fields,
                Ok(None) => return Routed::Continue,
                Err(err) => return Routed::Answer(Err(err)),
            },
            Err(err) => {
                return Routed::Answer(Err(CdpError::BadJson {
                    message: format!("pipe frame is not UTF-8: {err}"),
                }));
            }
        };
        if let Some(frame_id) = fields.get("id") {
            // Ids are unique across the pipe and only grow: anything but
            // ours answers an abandoned call.
            if frame_id.get().parse::<i64>().ok() == Some(id) {
                return Routed::Answer(response_result(&fields));
            }
            return Routed::Continue;
        }
        let Some(event) = fields.get("method").and_then(|raw| raw_string(raw)) else {
            return Routed::Continue;
        };
        let Some(params) = event_params(&fields) else {
            return Routed::Continue;
        };
        let event_session = fields.get("sessionId").and_then(|raw| raw_string(raw));
        if event == DETACHED_FROM_TARGET {
            if let Some(detached) = params.get("sessionId").and_then(Value::as_str) {
                if let Some(state) = self.sessions.get_mut(detached) {
                    state.gone = true;
                }
            }
        } else if event == TARGET_DESTROYED {
            if let Some(target) = params.get("targetId").and_then(Value::as_str) {
                for state in self.sessions.values_mut() {
                    if state.target_id == target {
                        state.gone = true;
                    }
                }
            }
        }
        let dialog = event == DIALOG_OPENING;
        let queue = match &event_session {
            None => &mut self.browser_events,
            Some(key) => match self.sessions.get_mut(key) {
                Some(state) => &mut state.events,
                // A session this browser did not attach: nobody drains it.
                None => return Routed::Continue,
            },
        };
        if queue.len() >= MAX_BUFFERED_EVENTS {
            queue.pop_front();
        }
        queue.push_back(CdpEvent {
            method: event,
            params,
        });
        if let Some(ours) = session {
            // The page answers nothing while a dialog is open: stop waiting.
            // The event stays queued.
            if dialog && event_session.as_deref() == Some(ours) {
                return Routed::Answer(Err(CdpError::DialogOpened {
                    method: method.to_owned(),
                }));
            }
            if let Some(gone) = self.gone_error(ours) {
                return Routed::Answer(Err(gone));
            }
        }
        Routed::Continue
    }

    fn gone_error(&self, session: &str) -> Option<CdpError> {
        let state = self.sessions.get(session)?;
        state.gone.then(|| CdpError::TargetGone {
            target_id: state.target_id.clone(),
        })
    }

    fn die(&mut self, err: CdpError) -> CdpError {
        self.dead = Some(err.clone());
        err
    }
}

/// A browser-level CDP connection over a pipe pair the caller owns.
///
/// `!Send` / `!Sync`: one thread owns it and every page it makes.
pub struct PipeBrowser {
    wire: Rc<RefCell<Wire>>,
}

impl PipeBrowser {
    /// Take over the ends of a `--remote-debugging-pipe` pair with default
    /// [`PipeLimits`]. `reader` is the end Chrome writes (its fd 4), `writer`
    /// the end Chrome reads (its fd 3). Anything that converts into an
    /// [`OwnedFd`] works: `File`, `OwnedFd`, `std::io::PipeReader` /
    /// `PipeWriter`, `ChildStdout` / `ChildStdin`. No I/O happens here.
    pub fn new(reader: impl Into<OwnedFd>, writer: impl Into<OwnedFd>) -> Self {
        Self::with_limits(reader, writer, PipeLimits::default())
    }

    pub fn with_limits(
        reader: impl Into<OwnedFd>,
        writer: impl Into<OwnedFd>,
        limits: PipeLimits,
    ) -> Self {
        let wire = Wire {
            reader: File::from(reader.into()),
            writer: File::from(writer.into()),
            splitter: FrameSplitter::new(limits.max_frame_bytes),
            chunk: vec![0; READ_CHUNK],
            limits,
            next_id: 1,
            dead: None,
            browser_events: VecDeque::new(),
            sessions: HashMap::new(),
        };
        Self {
            wire: Rc::new(RefCell::new(wire)),
        }
    }

    pub fn limits(&self) -> PipeLimits {
        self.wire.borrow().limits
    }

    /// One browser-level call (no `sessionId`). Returns the `result` text.
    pub fn call(&self, method: &str, params_json: &str) -> Result<String, CdpError> {
        self.wire.borrow_mut().call(None, method, params_json)
    }

    /// Browser-level events (no `sessionId`) queued since the last drain.
    pub fn drain_events(&self) -> Vec<CdpEvent> {
        self.wire.borrow_mut().browser_events.drain(..).collect()
    }

    /// Attach to `target_id` (`Target.attachToTarget`, `flatten: true`).
    pub fn page(&self, target_id: &str) -> Result<PipePage, CdpError> {
        let body = self.call(
            "Target.attachToTarget",
            &json!({"targetId": target_id, "flatten": true}).to_string(),
        )?;
        let session_id = json_field(&body, "sessionId", "Target.attachToTarget")?;
        self.wire.borrow_mut().sessions.insert(
            session_id.clone(),
            Session {
                target_id: target_id.to_owned(),
                events: VecDeque::new(),
                gone: false,
            },
        );
        Ok(PipePage {
            wire: Rc::clone(&self.wire),
            session_id,
            target_id: target_id.to_owned(),
        })
    }

    /// Page targets (`Target.getTargets`, `type == "page"` only), in reply order.
    pub fn targets(&self) -> Result<Vec<PipeTarget>, CdpError> {
        let body = self.call("Target.getTargets", "{}")?;
        let value: Value = serde_json::from_str(&body).map_err(|err| CdpError::BadJson {
            message: err.to_string(),
        })?;
        let text = |info: &Value, key: &str| {
            info.get(key)
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_owned()
        };
        Ok(value
            .get("targetInfos")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter(|info| info.get("type").and_then(Value::as_str) == Some("page"))
            .filter_map(|info| {
                Some(PipeTarget {
                    target_id: info.get("targetId").and_then(Value::as_str)?.to_owned(),
                    target_type: text(info, "type"),
                    url: text(info, "url"),
                    title: text(info, "title"),
                })
            })
            .collect())
    }

    /// Open a page target on `url` (`Target.createTarget`); returns its id.
    pub fn create_target(&self, url: &str) -> Result<String, CdpError> {
        let body = self.call("Target.createTarget", &json!({"url": url}).to_string())?;
        created_target_id(&body)
    }

    /// Close a target (`Target.closeTarget`); the reported `success` flag.
    pub fn close_target(&self, target_id: &str) -> Result<bool, CdpError> {
        let body = self.call(
            "Target.closeTarget",
            &json!({"targetId": target_id}).to_string(),
        )?;
        let value: Value = serde_json::from_str(&body).map_err(|err| CdpError::BadJson {
            message: err.to_string(),
        })?;
        value
            .get("success")
            .and_then(Value::as_bool)
            .ok_or_else(|| CdpError::BadJson {
                message: "closeTarget result missing `success`".to_owned(),
            })
    }

    /// Bring a target to the front (`Target.activateTarget`).
    pub fn activate_target(&self, target_id: &str) -> Result<(), CdpError> {
        self.call(
            "Target.activateTarget",
            &json!({"targetId": target_id}).to_string(),
        )?;
        Ok(())
    }
}

impl CdpTransport for PipeBrowser {
    fn call(&mut self, method: &str, params_json: &str) -> Result<String, CdpError> {
        PipeBrowser::call(self, method, params_json)
    }

    fn drain_events(&mut self) -> Vec<CdpEvent> {
        PipeBrowser::drain_events(self)
    }
}

/// One attached page session on a [`PipeBrowser`]'s pipe.
///
/// `!Send` / `!Sync`, like its browser. Dropping it sends
/// `Target.detachFromTarget` without waiting for the reply (best effort;
/// skipped when the pipe is dead or the target is already gone) and drops
/// the session's queued events.
pub struct PipePage {
    wire: Rc<RefCell<Wire>>,
    session_id: String,
    target_id: String,
}

impl PipePage {
    pub fn session_id(&self) -> &str {
        &self.session_id
    }

    pub fn target_id(&self) -> &str {
        &self.target_id
    }
}

impl CdpTransport for PipePage {
    fn call(&mut self, method: &str, params_json: &str) -> Result<String, CdpError> {
        self.wire
            .borrow_mut()
            .call(Some(&self.session_id), method, params_json)
    }

    /// This session's events only.
    fn drain_events(&mut self) -> Vec<CdpEvent> {
        self.wire
            .borrow_mut()
            .sessions
            .get_mut(&self.session_id)
            .map(|state| state.events.drain(..).collect())
            .unwrap_or_default()
    }
}

impl Drop for PipePage {
    fn drop(&mut self) {
        let Ok(mut wire) = self.wire.try_borrow_mut() else {
            return;
        };
        let state = wire.sessions.remove(&self.session_id);
        if state.is_some_and(|state| !state.gone) && wire.dead.is_none() {
            // The reply is dropped later like any abandoned id.
            let _ = wire.send(
                None,
                "Target.detachFromTarget",
                json!({"sessionId": self.session_id}),
            );
        }
    }
}

fn json_field(body: &str, key: &str, method: &str) -> Result<String, CdpError> {
    let value: Value = serde_json::from_str(body).map_err(|err| CdpError::BadJson {
        message: err.to_string(),
    })?;
    value
        .get(key)
        .and_then(Value::as_str)
        .map(str::to_owned)
        .ok_or_else(|| CdpError::Transport {
            message: format!("{method} result has no {key}"),
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splitter_refuses_an_unterminated_frame_past_the_cap_and_stays_failed() {
        let mut splitter = FrameSplitter::new(4);
        splitter.push(b"abcd");
        assert_eq!(splitter.next_frame(), Ok(None));
        splitter.push(b"e");
        assert_eq!(
            splitter.next_frame(),
            Err(CdpError::FrameTooLarge { limit: 4 })
        );
        assert_eq!(splitter.buffered(), 0);
        splitter.push(b"\0ok\0");
        assert_eq!(
            splitter.next_frame(),
            Err(CdpError::FrameTooLarge { limit: 4 })
        );
    }

    #[test]
    fn splitter_takes_a_frame_exactly_at_the_cap() {
        let mut splitter = FrameSplitter::new(4);
        splitter.push(b"abcd\0abcde\0");
        assert_eq!(splitter.next_frame(), Ok(Some(b"abcd".to_vec())));
        assert_eq!(
            splitter.next_frame(),
            Err(CdpError::FrameTooLarge { limit: 4 })
        );
    }

    #[test]
    fn deadline_error_keeps_the_websocket_wording_for_whole_seconds() {
        assert_eq!(
            deadline_error("X.y", CALL_DEADLINE).to_string(),
            "CDP transport: no CDP result for `X.y` within 30 s"
        );
        assert_eq!(
            deadline_error("X.y", Duration::from_millis(250)).to_string(),
            "CDP transport: no CDP result for `X.y` within 250 ms"
        );
    }
}
