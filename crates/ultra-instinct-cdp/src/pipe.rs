//! Blocking CDP over a caller-owned pipe pair (`--remote-debugging-pipe`).
//!
//! Chrome reads CDP from its fd 3 and writes to its fd 4: one UTF-8 JSON
//! message per frame, each frame ended by a NUL byte. The pipe is
//! browser-level: pages are reached with `Target.attachToTarget
//! {targetId, flatten: true}` and every page message carries `sessionId`.
//! This module never spawns Chrome; whoever launched it hands over the ends.
//!
//! **Two links, one wire.** The routing below (ids, sessions, events,
//! dialogs, gone targets, limits) runs over a crate-private `Link` that only
//! moves frames: the pipe here, or Chrome's browser websocket
//! (`ws_browser.rs`, [`CdpBrowser::connect_websocket`]). [`PipeBrowser`] /
//! [`PipePage`] are [`CdpBrowser`] / [`CdpPage`] under their old names.
//!
//! **Threading.** One thread owns a [`CdpBrowser`] and every [`CdpPage`]
//! made from it. They share the connection through `Rc<RefCell<..>>`, so all
//! of them are deliberately `!Send` and `!Sync`. There is no reader thread:
//! whoever calls pumps the pipe until its own id answers (pump-on-call),
//! routing events into per-session queues on the way. Message ids are unique
//! across the whole pipe and only grow, so a response for any other id
//! belongs to an abandoned call (timed out or cut short by a dialog) and is
//! dropped. Waiting uses `poll(2)` on the read end (through `rustix`), which
//! is why this module is `cfg(unix)` only.
//!
//! **One bad frame is one bad frame.** JSON never holds a raw NUL, so the
//! stream re-syncs at the next one. A frame over
//! [`PipeLimits::max_frame_bytes`] is skipped without being buffered past
//! the cap: when its first bytes name the waiting call's id (Chrome writes
//! `{"id":N,` first) that call gets [`CdpError::FrameTooLarge`], else the
//! frame is dropped and counted ([`PipeBrowser::dropped_frames`]). A frame
//! that is not JSON answers the waiting call only when its id is that
//! call's; otherwise it too is dropped and counted. EOF or a read / write
//! error marks the pipe dead: every later call on the browser and its pages
//! returns that error. A call that only timed out leaves the pipe usable.
//!
//! **Writes never block the reader.** The write end is made non-blocking
//! (`O_NONBLOCK` through `fcntl`; for a socket dup shared with the read end
//! the flag is shared too). A request is written in pieces while `poll(2)`
//! watches both ends: frames Chrome writes meanwhile are read and routed, so
//! a peer that will not read until its own write completes cannot deadlock
//! us. The write is bounded by the call deadline (typed
//! [`CdpError::Timeout`]); a request cut off part way leaves the stream
//! unframed, so the pipe is then dead for later calls.

use std::cell::RefCell;
use std::collections::{HashMap, VecDeque};
use std::fs::File;
use std::io::{ErrorKind, Read, Write};
use std::os::fd::OwnedFd;
use std::rc::Rc;
use std::time::{Duration, Instant};

use rustix::event::{poll, PollFd, PollFlags, Timespec};
use rustix::fs::{fcntl_getfl, fcntl_setfl, OFlags};
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

/// Bytes kept from the start of an oversize frame to read its `"id"`.
const ID_PREFIX_BYTES: usize = 64;

/// What [`FrameSplitter::next_frame`] cut from the stream.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Frame {
    /// One complete frame, its NUL removed.
    Message(Vec<u8>),
    /// A frame over the cap, skipped up to and including its NUL. `id` is
    /// the `"id"` its first bytes name (`{"id":N,`), when they do.
    Oversize { id: Option<i64> },
}

/// Splits a NUL-delimited byte stream into frames, skipping any frame over
/// a byte cap. Push each chunk read, then take frames until `None`;
/// buffering stays within the cap plus one chunk. An oversize frame is
/// reported once ([`Frame::Oversize`]) and its bytes are dropped as they
/// arrive, never held past the cap; the next frame starts after its NUL.
#[derive(Debug)]
pub struct FrameSplitter {
    buf: Vec<u8>,
    /// Bytes of `buf` already known to hold no NUL.
    scanned: usize,
    max_frame_bytes: usize,
    /// Inside an oversize frame: dropping bytes up to its NUL. Holds the id
    /// its first bytes named.
    skipping: Option<Option<i64>>,
}

impl FrameSplitter {
    pub fn new(max_frame_bytes: usize) -> Self {
        Self {
            buf: Vec::new(),
            scanned: 0,
            max_frame_bytes,
            skipping: None,
        }
    }

    /// Append bytes read from the stream.
    pub fn push(&mut self, bytes: &[u8]) {
        self.buf.extend_from_slice(bytes);
    }

    /// The next complete frame, an oversize frame skipped whole, or `None`
    /// when more bytes are needed.
    pub fn next_frame(&mut self) -> Option<Frame> {
        let nul = self.buf[self.scanned..].iter().position(|&b| b == 0);
        if let Some(id) = self.skipping {
            let Some(offset) = nul else {
                self.buf.clear();
                self.scanned = 0;
                return None;
            };
            self.buf.drain(..=self.scanned + offset);
            self.scanned = 0;
            self.skipping = None;
            return Some(Frame::Oversize { id });
        }
        match nul {
            Some(offset) => {
                let end = self.scanned + offset;
                let frame = if end > self.max_frame_bytes {
                    Frame::Oversize {
                        id: leading_id(&self.buf[..end]),
                    }
                } else {
                    Frame::Message(self.buf[..end].to_vec())
                };
                self.buf.drain(..=end);
                self.scanned = 0;
                Some(frame)
            }
            None if self.buf.len() > self.max_frame_bytes => {
                // Over the cap with no end in sight: keep only its id.
                self.skipping = Some(leading_id(&self.buf));
                self.buf.clear();
                self.scanned = 0;
                None
            }
            None => {
                self.scanned = self.buf.len();
                None
            }
        }
    }

    /// Bytes held for frames not yet complete.
    pub fn buffered(&self) -> usize {
        self.buf.len()
    }
}

/// The id a frame's first bytes name: `{"id":N` with optional whitespace,
/// as Chrome writes every response. `None` for anything else.
pub fn leading_id(frame: &[u8]) -> Option<i64> {
    let head = &frame[..frame.len().min(ID_PREFIX_BYTES)];
    let text = std::str::from_utf8(head)
        .unwrap_or_else(|err| std::str::from_utf8(&head[..err.valid_up_to()]).unwrap_or_default());
    let rest = text.trim_start().strip_prefix('{')?.trim_start();
    let rest = rest.strip_prefix("\"id\"")?.trim_start();
    let rest = rest.strip_prefix(':')?.trim_start();
    let digits = rest
        .char_indices()
        .find(|&(i, c)| !(c.is_ascii_digit() || (i == 0 && c == '-')))
        .map_or(rest.len(), |(i, _)| i);
    // The number must end inside the prefix, else it may be cut short.
    if digits == rest.len() && head.len() < frame.len() {
        return None;
    }
    rest[..digits].parse().ok()
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

/// Why [`Link::pump`] stopped without progress.
pub(crate) enum Stop {
    /// The call's deadline passed.
    Deadline,
    /// The link broke (EOF, a read / write / poll error); this is the message.
    Closed(String),
}

/// How frames move over one browser connection: the pipe ([`PipeLink`]) or
/// the browser websocket (`crate::ws_browser`). The [`Wire`] above it owns
/// ids, routing, sessions, events and dialogs; a link only moves bytes.
pub(crate) trait Link {
    /// Begin writing one request (its JSON text, unframed). [`Link::pump`]
    /// writes it while it also reads.
    fn begin(&mut self, message: Vec<u8>);
    /// Whether everything begun is written.
    fn flushed(&self) -> bool;
    /// Wait until `deadline` for progress: bytes read (whole frames go to
    /// [`Link::next_frame`]) or, while not [`Link::flushed`], bytes written.
    fn pump(&mut self, deadline: Instant) -> Result<(), Stop>;
    /// The begun request missed its deadline. `true`: part of it is on the
    /// wire, so the stream is unframed and the link is dead.
    fn abandon(&mut self) -> bool;
    /// The next whole frame read, if any.
    fn next_frame(&mut self) -> Option<Frame>;
    /// Nothing of an earlier request is waiting to be written.
    fn idle(&self) -> bool;
    /// One non-blocking try at a request nobody waits for (never waits;
    /// a rest the link could not take now may be finished by the next
    /// [`Link::begin`], or the request is skipped).
    fn try_send(&mut self, message: Vec<u8>);
}

/// The pipe link: NUL-delimited frames over two fds, `poll(2)` on both.
struct PipeLink {
    reader: File,
    writer: File,
    splitter: FrameSplitter,
    chunk: Vec<u8>,
    /// The rest of a frame [`Link::try_send`] got only partly written: the
    /// next [`Link::begin`] writes it first, so frames stay whole.
    outbox: Vec<u8>,
    /// The bytes being written: the carried outbox, then this request.
    out: Vec<u8>,
    written: usize,
    carried: usize,
}

impl Link for PipeLink {
    fn begin(&mut self, message: Vec<u8>) {
        // A frame `try_send` left half written goes first.
        let mut bytes = std::mem::take(&mut self.outbox);
        self.carried = bytes.len();
        bytes.extend_from_slice(&message);
        bytes.push(0);
        self.out = bytes;
        self.written = 0;
    }

    fn flushed(&self) -> bool {
        self.written >= self.out.len()
    }

    /// Wait until the read end has bytes, then push one chunk; while a
    /// request is unwritten, also until the write end takes some of it.
    fn pump(&mut self, deadline: Instant) -> Result<(), Stop> {
        loop {
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() {
                return Err(Stop::Deadline);
            }
            // Long waits poll in hour slices (macOS caps poll at `c_int::MAX` ms).
            let slice = left.min(Duration::from_secs(3600));
            let timeout = Timespec {
                tv_sec: slice.as_secs().try_into().unwrap_or(3600),
                tv_nsec: slice.subsec_nanos().into(),
            };
            let pending = !self.flushed();
            let (readable, writable) = {
                let mut fds = [
                    PollFd::new(&self.reader, PollFlags::IN),
                    PollFd::new(&self.writer, PollFlags::OUT),
                ];
                let watched = if pending { 2 } else { 1 };
                match poll(&mut fds[..watched], Some(&timeout)) {
                    Ok(0) | Err(rustix::io::Errno::INTR) => continue,
                    Ok(_) => {}
                    Err(err) => return Err(Stop::Closed(format!("poll: {err}"))),
                }
                (
                    !fds[0].revents().is_empty(),
                    watched == 2 && !fds[1].revents().is_empty(),
                )
            };
            if readable {
                match self.reader.read(&mut self.chunk) {
                    Ok(0) => return Err(Stop::Closed("end of stream".into())),
                    Ok(n) => {
                        self.splitter.push(&self.chunk[..n]);
                        return Ok(());
                    }
                    Err(err)
                        if matches!(err.kind(), ErrorKind::Interrupted | ErrorKind::WouldBlock) => {
                    }
                    Err(err) => return Err(Stop::Closed(format!("read: {err}"))),
                }
            }
            if writable {
                match self.writer.write(&self.out[self.written..]) {
                    Ok(n) => {
                        self.written += n;
                        return Ok(());
                    }
                    Err(err)
                        if matches!(err.kind(), ErrorKind::Interrupted | ErrorKind::WouldBlock) => {
                    }
                    Err(err) => return Err(Stop::Closed(format!("write: {err}"))),
                }
            }
        }
    }

    fn abandon(&mut self) -> bool {
        let out = std::mem::take(&mut self.out);
        let (written, carried) = (self.written, self.carried);
        self.written = 0;
        if written > carried {
            // Half a request on the wire: nothing after it parses.
            return true;
        }
        // None of this request left; the carried rest still waits.
        self.outbox = out[written..carried].to_vec();
        false
    }

    fn next_frame(&mut self) -> Option<Frame> {
        self.splitter.next_frame()
    }

    fn idle(&self) -> bool {
        self.outbox.is_empty()
    }

    fn try_send(&mut self, mut message: Vec<u8>) {
        message.push(0);
        if let Ok(n) = self.writer.write(&message) {
            if n > 0 && n < message.len() {
                self.outbox = message[n..].to_vec();
            }
        }
    }
}

/// The connection every handle shares: one link, one id space.
struct Wire {
    link: Box<dyn Link>,
    limits: PipeLimits,
    next_id: i64,
    dead: Option<CdpError>,
    /// Frames dropped unread: oversize or malformed, for no waiting call.
    dropped_frames: u64,
    browser_events: VecDeque<CdpEvent>,
    sessions: HashMap<String, Session>,
}

/// What one frame meant to the call waiting on it.
enum Routed {
    Answer(Result<String, CdpError>),
    Continue,
}

impl Wire {
    /// One call, answered within `wait` (the call deadline unless a caller
    /// asks for another).
    fn call(
        &mut self,
        session: Option<&str>,
        method: &str,
        params_json: &str,
        wait: Duration,
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
        let deadline = Instant::now() + wait;
        let (id, early) = self.send(session, method, params, deadline, wait)?;
        if let Some(answer) = early {
            return answer;
        }
        loop {
            if let Some(answer) = self.route_buffered(id, session, method) {
                return answer;
            }
            self.pump(deadline, method, wait)?;
        }
    }

    /// Write one request frame, reading and routing what arrives meanwhile.
    /// Returns its id, and the answer a frame read during the write already
    /// gave the call (a dialog, a gone target).
    fn send(
        &mut self,
        session: Option<&str>,
        method: &str,
        params: Value,
        deadline: Instant,
        wait: Duration,
    ) -> Result<(i64, Option<Result<String, CdpError>>), CdpError> {
        if let Some(err) = &self.dead {
            return Err(err.clone());
        }
        let (id, message) = self.frame(session, method, params);
        self.link.begin(message);
        let mut early = None;
        while !self.link.flushed() {
            // Route what came in (events queue; a second answer is moot).
            while let Some(answer) = self.route_buffered(id, session, method) {
                early.get_or_insert(answer);
            }
            match self.pump(deadline, method, wait) {
                Ok(()) => {}
                Err(err @ CdpError::Timeout { .. }) => {
                    if self.link.abandon() {
                        self.die(CdpError::PipeClosed {
                            message: format!("`{method}` was cut off mid-write by its deadline"),
                        });
                    }
                    return Err(err);
                }
                Err(err) => return Err(err),
            }
        }
        Ok((id, early))
    }

    /// A request's JSON text with the next id.
    fn frame(&mut self, session: Option<&str>, method: &str, params: Value) -> (i64, Vec<u8>) {
        let id = self.next_id;
        self.next_id += 1;
        let mut envelope = json!({"id": id, "method": method, "params": params});
        if let Some(session) = session {
            envelope["sessionId"] = Value::from(session);
        }
        (id, envelope.to_string().into_bytes())
    }

    /// One non-blocking try at a request whose answer nobody waits for. It
    /// never waits and never marks the link dead: a request the link takes
    /// none of now is skipped (also while an earlier one's rest is pending,
    /// or on a write error the next call meets).
    fn send_now(&mut self, method: &str, params: Value) {
        if self.dead.is_some() || !self.link.idle() {
            return;
        }
        let (_, message) = self.frame(None, method, params);
        self.link.try_send(message);
    }

    /// Route every complete frame buffered; the waiting call's answer, if
    /// one came.
    fn route_buffered(
        &mut self,
        id: i64,
        session: Option<&str>,
        method: &str,
    ) -> Option<Result<String, CdpError>> {
        while let Some(frame) = self.link.next_frame() {
            let routed = match frame {
                Frame::Message(frame) => self.route(&frame, id, session, method),
                Frame::Oversize { id: Some(frame_id) } if frame_id == id => {
                    Routed::Answer(Err(CdpError::FrameTooLarge {
                        limit: self.limits.max_frame_bytes,
                    }))
                }
                Frame::Oversize { .. } => {
                    self.dropped_frames += 1;
                    Routed::Continue
                }
            };
            if let Routed::Answer(answer) = routed {
                return Some(answer);
            }
        }
        None
    }

    /// One wait on the link; a broken link marks the wire dead.
    fn pump(&mut self, deadline: Instant, method: &str, wait: Duration) -> Result<(), CdpError> {
        match self.link.pump(deadline) {
            Ok(()) => Ok(()),
            Err(Stop::Deadline) => Err(deadline_error(method, wait)),
            Err(Stop::Closed(message)) => Err(self.die(CdpError::PipeClosed { message })),
        }
    }

    fn route(&mut self, frame: &[u8], id: i64, session: Option<&str>, method: &str) -> Routed {
        let parsed = match std::str::from_utf8(frame) {
            Ok(text) => frame_fields(text),
            Err(err) => Err(CdpError::BadJson {
                message: format!("pipe frame is not UTF-8: {err}"),
            }),
        };
        let fields = match parsed {
            Ok(Some(fields)) => fields,
            Ok(None) => return Routed::Continue,
            // A broken frame answers only the call it names; one for an
            // abandoned call, an event, or no id at all is dropped.
            Err(err) if leading_id(frame) == Some(id) => return Routed::Answer(Err(err)),
            Err(_) => {
                self.dropped_frames += 1;
                return Routed::Continue;
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

/// A browser-level CDP connection: over a pipe pair the caller owns
/// ([`CdpBrowser::new`]), or over Chrome's one browser websocket
/// (`CdpBrowser::connect_websocket`, for a Chrome whose remote debugging the
/// user turned on). Both carry flattened page sessions the same way.
///
/// `!Send` / `!Sync`: one thread owns it and every page it makes. A clone is
/// another handle on the same connection (like a [`CdpPage`]), not a second
/// link.
#[derive(Clone)]
pub struct CdpBrowser {
    wire: Rc<RefCell<Wire>>,
}

/// The pipe browser's name before the websocket link: the same type.
pub type PipeBrowser = CdpBrowser;
/// The pipe page's name before the websocket link: the same type.
pub type PipePage = CdpPage;

impl CdpBrowser {
    /// Take over the ends of a `--remote-debugging-pipe` pair with default
    /// [`PipeLimits`]. `reader` is the end Chrome writes (its fd 4), `writer`
    /// the end Chrome reads (its fd 3). Anything that converts into an
    /// [`OwnedFd`] works: `File`, `OwnedFd`, `std::io::PipeReader` /
    /// `PipeWriter`, `ChildStdout` / `ChildStdin`. No I/O happens here; the
    /// write end is set `O_NONBLOCK` (shared with any dup of it, such as the
    /// read end when both are one socket).
    pub fn new(reader: impl Into<OwnedFd>, writer: impl Into<OwnedFd>) -> Self {
        Self::with_limits(reader, writer, PipeLimits::default())
    }

    pub fn with_limits(
        reader: impl Into<OwnedFd>,
        writer: impl Into<OwnedFd>,
        limits: PipeLimits,
    ) -> Self {
        let writer = File::from(writer.into());
        // Writes interleave with reads (`Wire::send`), so the write end must
        // never block; a failure here fails every call instead.
        let dead = fcntl_getfl(&writer)
            .and_then(|flags| fcntl_setfl(&writer, flags | OFlags::NONBLOCK))
            .err()
            .map(|err| CdpError::PipeClosed {
                message: format!("cannot make the write end non-blocking: {err}"),
            });
        let link = PipeLink {
            reader: File::from(reader.into()),
            writer,
            splitter: FrameSplitter::new(limits.max_frame_bytes),
            chunk: vec![0; READ_CHUNK],
            outbox: Vec::new(),
            out: Vec::new(),
            written: 0,
            carried: 0,
        };
        Self::over(Box::new(link), limits, dead)
    }

    /// A browser over `link`; `dead` fails every call from the start.
    pub(crate) fn over(link: Box<dyn Link>, limits: PipeLimits, dead: Option<CdpError>) -> Self {
        let wire = Wire {
            link,
            limits,
            next_id: 1,
            dead,
            dropped_frames: 0,
            browser_events: VecDeque::new(),
            sessions: HashMap::new(),
        };
        Self {
            wire: Rc::new(RefCell::new(wire)),
        }
    }

    /// One browser-level call answered within `wait` instead of the call
    /// deadline (the first call on a link that may wait on a prompt, a
    /// cleanup that must not hang).
    pub(crate) fn call_within(
        &self,
        method: &str,
        params_json: &str,
        wait: Duration,
    ) -> Result<String, CdpError> {
        self.wire.borrow_mut().call(None, method, params_json, wait)
    }

    pub fn limits(&self) -> PipeLimits {
        self.wire.borrow().limits
    }

    /// Frames dropped unread so far: over the cap or not JSON, and not the
    /// answer to the call that was waiting (an event, an abandoned call's
    /// reply, or no readable id).
    pub fn dropped_frames(&self) -> u64 {
        self.wire.borrow().dropped_frames
    }

    /// One browser-level call (no `sessionId`). Returns the `result` text.
    pub fn call(&self, method: &str, params_json: &str) -> Result<String, CdpError> {
        let wait = self.limits().call_deadline;
        self.wire.borrow_mut().call(None, method, params_json, wait)
    }

    /// Browser-level events (no `sessionId`) queued since the last drain.
    pub fn drain_events(&self) -> Vec<CdpEvent> {
        self.wire.borrow_mut().browser_events.drain(..).collect()
    }

    /// Take the browser-level events `take` picks, in order; every other
    /// one stays queued (still bounded) for another consumer.
    pub fn take_events_where(&self, mut take: impl FnMut(&CdpEvent) -> bool) -> Vec<CdpEvent> {
        let mut wire = self.wire.borrow_mut();
        let (taken, kept) = wire.browser_events.drain(..).partition(|event| take(event));
        wire.browser_events = kept;
        taken.into()
    }

    /// Attach to `target_id` (`Target.attachToTarget`, `flatten: true`).
    pub fn page(&self, target_id: &str) -> Result<CdpPage, CdpError> {
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
        Ok(CdpPage {
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

impl CdpTransport for CdpBrowser {
    fn call(&mut self, method: &str, params_json: &str) -> Result<String, CdpError> {
        CdpBrowser::call(self, method, params_json)
    }

    fn drain_events(&mut self) -> Vec<CdpEvent> {
        CdpBrowser::drain_events(self)
    }
}

/// One attached page session on a [`CdpBrowser`]'s link.
///
/// `!Send` / `!Sync`, like its browser. Dropping it makes one non-blocking
/// try at `Target.detachFromTarget` and never waits for the reply or for
/// room in the pipe (skipped when the pipe is dead or full, or the target
/// is already gone; a frame written in part is finished by the next call),
/// and drops the session's queued events.
pub struct CdpPage {
    wire: Rc<RefCell<Wire>>,
    session_id: String,
    target_id: String,
}

impl CdpPage {
    pub fn session_id(&self) -> &str {
        &self.session_id
    }

    pub fn target_id(&self) -> &str {
        &self.target_id
    }
}

impl CdpTransport for CdpPage {
    fn call(&mut self, method: &str, params_json: &str) -> Result<String, CdpError> {
        let mut wire = self.wire.borrow_mut();
        let wait = wire.limits.call_deadline;
        wire.call(Some(&self.session_id), method, params_json, wait)
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

impl Drop for CdpPage {
    fn drop(&mut self) {
        let Ok(mut wire) = self.wire.try_borrow_mut() else {
            return;
        };
        let state = wire.sessions.remove(&self.session_id);
        if state.is_some_and(|state| !state.gone) {
            // One non-blocking try (Chrome detaches on target or browser
            // close anyway); the reply is dropped like any abandoned id.
            wire.send_now(
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

    fn message(bytes: &[u8]) -> Option<Frame> {
        Some(Frame::Message(bytes.to_vec()))
    }

    #[test]
    fn splitter_skips_an_unterminated_frame_past_the_cap_and_resyncs() {
        let mut splitter = FrameSplitter::new(4);
        splitter.push(b"abcd");
        assert_eq!(splitter.next_frame(), None);
        splitter.push(b"e");
        assert_eq!(splitter.next_frame(), None);
        assert_eq!(splitter.buffered(), 0, "nothing held past the cap");
        splitter.push(b"fgh");
        assert_eq!(splitter.next_frame(), None);
        assert_eq!(splitter.buffered(), 0);
        splitter.push(b"ij\0ok\0");
        assert_eq!(splitter.next_frame(), Some(Frame::Oversize { id: None }));
        assert_eq!(splitter.next_frame(), message(b"ok"));
        assert_eq!(splitter.next_frame(), None);
    }

    #[test]
    fn splitter_takes_a_frame_exactly_at_the_cap() {
        let mut splitter = FrameSplitter::new(4);
        splitter.push(b"abcd\0abcde\0");
        assert_eq!(splitter.next_frame(), message(b"abcd"));
        assert_eq!(splitter.next_frame(), Some(Frame::Oversize { id: None }));
        assert_eq!(splitter.next_frame(), None);
    }

    #[test]
    fn an_oversize_frame_keeps_the_id_it_starts_with() {
        let mut splitter = FrameSplitter::new(16);
        splitter.push(br#"{"id":42,"result":{"data":"xxxxxxxx"#);
        assert_eq!(splitter.next_frame(), None);
        splitter.push(b"xxxx\"}}\0");
        assert_eq!(
            splitter.next_frame(),
            Some(Frame::Oversize { id: Some(42) })
        );
    }

    #[test]
    fn leading_id_reads_only_a_leading_id() {
        assert_eq!(leading_id(br#"{"id":7,"result":{}}"#), Some(7));
        assert_eq!(leading_id(br#" { "id" : 12 , "x"#), Some(12));
        assert_eq!(leading_id(br#"{"id":-3}"#), Some(-3));
        assert_eq!(leading_id(br#"{"method":"X","id":7}"#), None);
        assert_eq!(leading_id(br#"{"id":"7"}"#), None);
        assert_eq!(leading_id(b"\xff\xfe"), None);
        // A number running to the end of a cut prefix may be cut short.
        let long = format!("{{\"id\":{}", "1".repeat(80));
        assert_eq!(leading_id(long.as_bytes()), None);
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
