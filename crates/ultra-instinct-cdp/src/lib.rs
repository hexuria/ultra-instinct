//! CDP transports and replay support for ultra-instinct.

#![forbid(unsafe_code)]

mod error;
mod replay;
pub mod script;
mod transport;
mod ws;

pub use error::CdpError;
pub use replay::ReplayTransport;
pub use transport::{CdpEvent, CdpTransport};
pub use ws::{
    activate_target, close_target, create_target, open_tab, page_targets, PageTarget,
    WebSocketTransport, DEFAULT_CDP_HTTP,
};

/// Node attribute stamped by compact observe and shared with the script builder.
pub const HU_K_ATTR: &str = "data-hu-k";
