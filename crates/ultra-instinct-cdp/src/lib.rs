//! CDP transports and replay support for ultra-instinct.
//!
//! Transports: a blocking websocket ([`WebSocketTransport`]), a replay
//! script ([`ReplayTransport`]), and, on unix only, a caller-owned
//! `--remote-debugging-pipe` pair ([`PipeBrowser`] / [`PipePage`]).

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

pub use backend::{CdpConnector, HttpBackend, PageTab, TargetBackend};
pub use error::CdpError;
#[cfg(unix)]
pub use pipe::{
    leading_id, Frame, FrameSplitter, PipeBrowser, PipeLimits, PipePage, PipeTarget,
    DEFAULT_MAX_FRAME_BYTES,
};
#[cfg(unix)]
pub use pipe_backend::{PipeBackend, PIPE_ENDPOINT};
pub use replay::ReplayTransport;
pub use transport::{CdpEvent, CdpTransport};
pub use ws::{
    activate_target, close_target, create_target, open_tab, page_targets, PageTarget,
    WebSocketTransport, DEFAULT_CDP_HTTP,
};

/// Node attribute stamped by compact observe and shared with the script builder.
pub const HU_K_ATTR: &str = "data-hu-k";
