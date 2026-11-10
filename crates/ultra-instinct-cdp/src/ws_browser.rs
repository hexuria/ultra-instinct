//! The browser websocket link: one blocking `ws://` socket to Chrome's
//! browser target (`/devtools/browser/<id>`), carrying the same flattened
//! page sessions as the pipe.
//!
//! This is how a Chrome whose remote debugging the user turned on at
//! `chrome://inspect/#remote-debugging` is reached: it writes
//! `DevToolsActivePort` with the browser socket's path, and its HTTP `/json`
//! endpoints may not answer, so tabs are `Target.*` calls on this socket.
//! Chrome may ask the user to allow the connection first; connecting waits
//! for that up to [`ACCEPT_WAIT`].
//!
//! After the handshake the socket is non-blocking and `poll(2)` watches it
//! for reads and, while a request is unwritten, writes, like the pipe: a
//! big request never blocks the reads that would drain Chrome's side. One
//! thread, no reader thread. A message over [`PipeLimits::max_frame_bytes`]
//! or a request cut off by its deadline ends the link (tungstenite cannot
//! skip a message), unlike the pipe, which re-syncs.

use std::collections::VecDeque;
use std::io::ErrorKind;
use std::net::TcpStream;
use std::time::{Duration, Instant};

use rustix::event::{poll, PollFd, PollFlags, Timespec};
use tungstenite::protocol::WebSocketConfig;
use tungstenite::{Message, WebSocket};

use crate::pipe::{Frame, Link, PipeLimits, Stop};
use crate::ws::connect_plain;
use crate::{CdpBrowser, CdpError};

/// How long connecting waits, in all, for Chrome to let the client in (its
/// "Allow remote debugging?" prompt): the handshake and the first call.
pub const ACCEPT_WAIT: Duration = Duration::from_secs(90);

/// What to do when the browser socket does not open.
const ACCEPT_HINT: &str = "turn on remote debugging at chrome://inspect/#remote-debugging \
and accept Chrome's \"Allow remote debugging?\" prompt, then try again";

struct WsLink {
    socket: WebSocket<TcpStream>,
    frames: VecDeque<Frame>,
    /// A request handed to tungstenite and not yet flushed.
    pending: bool,
    /// A write error `begin` met, reported by the next pump.
    failed: Option<String>,
}

impl WsLink {
    fn read_one(&mut self) -> Result<bool, Stop> {
        match self.socket.read() {
            Ok(Message::Text(text)) => {
                self.frames
                    .push_back(Frame::Message(text.as_bytes().to_vec()));
                Ok(true)
            }
            Ok(Message::Binary(bytes)) => {
                self.frames.push_back(Frame::Message(bytes.to_vec()));
                Ok(true)
            }
            Ok(Message::Close(_)) => Err(Stop::Closed("the browser closed the websocket".into())),
            // Pings are answered by tungstenite on the next read or flush.
            Ok(_) => Ok(true),
            Err(tungstenite::Error::Io(err))
                if matches!(err.kind(), ErrorKind::WouldBlock | ErrorKind::Interrupted) =>
            {
                Ok(false)
            }
            Err(err) => Err(Stop::Closed(format!("browser websocket: {err}"))),
        }
    }
}

impl Link for WsLink {
    fn begin(&mut self, message: Vec<u8>) {
        let text = String::from_utf8(message).unwrap_or_default();
        match self.socket.write(Message::Text(text.into())) {
            Ok(()) => {}
            Err(tungstenite::Error::Io(err)) if err.kind() == ErrorKind::WouldBlock => {}
            Err(err) => self.failed = Some(format!("browser websocket: {err}")),
        }
        self.pending = true;
    }

    fn flushed(&self) -> bool {
        !self.pending
    }

    fn pump(&mut self, deadline: Instant) -> Result<(), Stop> {
        loop {
            if let Some(message) = self.failed.take() {
                return Err(Stop::Closed(message));
            }
            if self.pending {
                match self.socket.flush() {
                    Ok(()) => {
                        self.pending = false;
                        return Ok(());
                    }
                    Err(tungstenite::Error::Io(err))
                        if matches!(err.kind(), ErrorKind::WouldBlock | ErrorKind::Interrupted) => {
                    }
                    Err(err) => return Err(Stop::Closed(format!("browser websocket: {err}"))),
                }
            }
            if self.read_one()? {
                return Ok(());
            }
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() {
                return Err(Stop::Deadline);
            }
            let slice = left.min(Duration::from_secs(3600));
            let timeout = Timespec {
                tv_sec: slice.as_secs().try_into().unwrap_or(3600),
                tv_nsec: slice.subsec_nanos().into(),
            };
            let mut flags = PollFlags::IN;
            if self.pending {
                flags |= PollFlags::OUT;
            }
            let mut fds = [PollFd::new(self.socket.get_ref(), flags)];
            match poll(&mut fds, Some(&timeout)) {
                Ok(_) | Err(rustix::io::Errno::INTR) => {}
                Err(err) => return Err(Stop::Closed(format!("poll: {err}"))),
            }
        }
    }

    fn abandon(&mut self) -> bool {
        // tungstenite holds the rest; sent later it would run late: end here.
        true
    }

    fn next_frame(&mut self) -> Option<Frame> {
        self.frames.pop_front()
    }

    fn idle(&self) -> bool {
        !self.pending
    }

    fn try_send(&mut self, message: Vec<u8>) {
        let text = String::from_utf8(message).unwrap_or_default();
        if self.socket.write(Message::Text(text.into())).is_ok() {
            let _ = self.socket.flush();
        }
    }
}

impl CdpBrowser {
    /// Connect to Chrome's browser websocket (`ws://127.0.0.1:<port>/devtools/browser/<id>`)
    /// with default [`PipeLimits`], waiting up to [`ACCEPT_WAIT`] for the
    /// user to allow it. Plain `ws://` only.
    pub fn connect_websocket(ws_url: &str) -> Result<Self, CdpError> {
        Self::connect_websocket_with(ws_url, PipeLimits::default(), ACCEPT_WAIT)
    }

    /// [`CdpBrowser::connect_websocket`] with `limits` and an `accept_wait`
    /// for the handshake and the first call (`Browser.getVersion`) together. A
    /// socket that does not open, or a first call that gets no answer in
    /// time, is a [`CdpError::Transport`] that says to turn on remote
    /// debugging and accept Chrome's prompt.
    pub fn connect_websocket_with(
        ws_url: &str,
        limits: PipeLimits,
        accept_wait: Duration,
    ) -> Result<Self, CdpError> {
        let hint = |detail: String| CdpError::Transport {
            message: format!("could not attach to Chrome at {ws_url}: {detail}; {ACCEPT_HINT}"),
        };
        let started = Instant::now();
        let config = WebSocketConfig::default()
            .max_message_size(Some(limits.max_frame_bytes))
            .max_frame_size(Some(limits.max_frame_bytes));
        let socket = match connect_plain(ws_url, accept_wait, Some(config)) {
            Ok(socket) => socket,
            Err(err @ CdpError::Transport { .. }) if ws_url.starts_with("wss://") => {
                return Err(err)
            }
            Err(CdpError::Transport { message }) => return Err(hint(message)),
            Err(err) => return Err(err),
        };
        socket
            .get_ref()
            .set_nonblocking(true)
            .map_err(|err| CdpError::Transport {
                message: format!("browser websocket: {err}"),
            })?;
        let link = WsLink {
            socket,
            frames: VecDeque::new(),
            pending: false,
            failed: None,
        };
        let browser = Self::over(Box::new(link), limits, None);
        let left = accept_wait
            .saturating_sub(started.elapsed())
            .max(Duration::from_millis(100));
        match browser.call_within("Browser.getVersion", "{}", left) {
            Ok(_) => Ok(browser),
            Err(CdpError::Timeout { .. }) => Err(hint(format!(
                "no answer within {} s",
                accept_wait.as_secs()
            ))),
            Err(err) => Err(err),
        }
    }
}
