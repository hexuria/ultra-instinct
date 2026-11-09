//! CDP transports and replay support for ultra-instinct.
//!
//! Transports: a blocking websocket ([`WebSocketTransport`]), a replay
//! script ([`ReplayTransport`]), and, on unix only, a caller-owned
//! `--remote-debugging-pipe` pair ([`PipeBrowser`] / [`PipePage`], ADR 0013).

#![forbid(unsafe_code)]

mod error;
#[cfg(unix)]
mod pipe;
mod replay;
pub mod script;
mod transport;
mod ws;

pub use error::CdpError;
#[cfg(unix)]
pub use pipe::{
    FrameSplitter, PipeBrowser, PipeLimits, PipePage, PipeTarget, DEFAULT_MAX_FRAME_BYTES,
};
pub use replay::ReplayTransport;
pub use transport::{CdpEvent, CdpTransport};
pub use ws::{
    activate_target, close_target, create_target, open_tab, page_targets, PageTarget,
    WebSocketTransport, DEFAULT_CDP_HTTP,
};

/// Node attribute stamped by compact observe and shared with the script builder.
pub const HU_K_ATTR: &str = "data-hu-k";
