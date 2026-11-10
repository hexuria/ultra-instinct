//! CDP transports and replay support for ultra-instinct.
//!
//! Transports: a blocking websocket ([`WebSocketTransport`]), a replay
//! script ([`ReplayTransport`]), and, on unix only, a session-multiplexing
//! browser ([`CdpBrowser`] / [`CdpPage`]) over a caller-owned
//! `--remote-debugging-pipe` pair or over Chrome's one browser websocket.

#![forbid(unsafe_code)]

mod backend;
mod error;
pub mod har;
#[cfg(unix)]
mod pipe;
#[cfg(unix)]
mod pipe_backend;
mod replay;
pub mod script;
mod transport;
mod ws;
#[cfg(unix)]
mod ws_browser;

pub use backend::{CdpConnector, HttpBackend, PageTab, TargetBackend};
pub use error::CdpError;
#[cfg(unix)]
pub use pipe::{
    leading_id, CdpBrowser, CdpPage, Frame, FrameSplitter, PipeBrowser, PipeLimits, PipePage,
    PipeTarget, DEFAULT_MAX_FRAME_BYTES,
};
#[cfg(unix)]
pub use pipe_backend::{PipeBackend, SessionBackend, ATTACH_ENDPOINT, PIPE_ENDPOINT};
pub use replay::ReplayTransport;
pub use transport::{CdpEvent, CdpTransport};
pub use ws::{
    activate_target, close_target, create_target, open_tab, page_targets, PageTarget,
    WebSocketTransport, DEFAULT_CDP_HTTP,
};
#[cfg(unix)]
pub use ws_browser::ACCEPT_WAIT;

/// Node attribute stamped by compact observe and shared with the script builder.
pub const HU_K_ATTR: &str = "data-hu-k";
