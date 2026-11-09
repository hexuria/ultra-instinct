//! `browser_*` MCP tool families for ultra-instinct: the browser-use-compatible
//! host surface (navigate, get_state, click, type, scroll, screenshot, tabs),
//! the agent-browser ports (dialog, check, focus, get / is, login state,
//! settings, downloads, network) and the [`ToolError`] every tool returns.
//!
//! The tools run on a [`BrowserHost`] (the MCP `Server` in
//! `ultra-instinct-mcp`). Target-bound input still goes through gate → ticket
//! → `execute_ticketed`, so a stale page refuses instead of clicking the wrong
//! element; page-level calls are plain CDP.

#![forbid(unsafe_code)]

mod bound;
mod browser_env;
mod browser_extra;
mod browser_state;
mod browser_tools;
mod error;
mod host;
mod page_text;

pub use browser_env::ENV_TOOLS;
pub use browser_extra::EXTRA_TOOLS;
pub use browser_state::STATE_TOOLS;
pub use browser_tools::BROWSER_TOOLS;
pub use error::{ToolError, ERROR_CODES};
pub use host::{
    BrowserHost, CdpConnector, HttpBackend, LiveSession, PageTab, TargetBackend, ToolState,
};

use serde_json::{Map, Value};

/// Run a `browser_*` tool on `host`. `None` when `name` is not one; the
/// caller has already checked that `arguments` is an object.
pub fn call(
    host: &mut dyn BrowserHost,
    name: &str,
    arguments: &Value,
) -> Option<Result<Value, ToolError>> {
    if ENV_TOOLS.contains(&name) {
        return Some(browser_env::call(host, name, arguments));
    }
    if let Some(result) = browser_state::call(host, name, arguments) {
        return Some(result);
    }
    if name.starts_with("browser_") {
        return Some(browser_extra::call(host, name, arguments));
    }
    None
}

/// `(description, properties, required)` for a `browser_*` tool.
pub fn spec(name: &str) -> (String, Map<String, Value>, Vec<&'static str>) {
    if ENV_TOOLS.contains(&name) {
        browser_env::spec(name)
    } else {
        browser_state::spec(name).unwrap_or_else(|| browser_extra::spec(name))
    }
}
