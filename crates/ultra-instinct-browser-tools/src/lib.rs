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
mod secrets;

pub use browser_env::ENV_TOOLS;
pub use browser_extra::EXTRA_TOOLS;
pub use browser_state::STATE_TOOLS;
pub use browser_tools::BROWSER_TOOLS;
pub use error::ToolError;
pub use host::{BrowserHost, Downloads, LiveSession, ToolLimits, ToolState};
pub use secrets::{sensitive, MASK};
pub use ultra_instinct_cdp::{CdpConnector, HttpBackend, PageTab, TargetBackend};

use serde_json::{Map, Value};

/// Run a `browser_*` tool on `host`. `None` when `name` is not one; the
/// caller has already checked that `arguments` is an object.
pub fn call(
    host: &mut dyn BrowserHost,
    name: &str,
    arguments: &Value,
) -> Option<Result<Value, ToolError>> {
    let result = if ENV_TOOLS.contains(&name) {
        browser_env::call(host, name, arguments)
            .map_err(|err| browser_extra::dialog_error(host, arguments, err))
    } else if let Some(result) = browser_state::call(host, name, arguments) {
        result.map_err(|err| browser_extra::dialog_error(host, arguments, err))
    } else if name.starts_with("browser_") {
        browser_extra::call(host, name, arguments)
    } else {
        return None;
    };
    // Text typed into a sensitive field never comes back out.
    let secrets = &host.tool_state().secrets;
    Some(match result {
        Ok(mut body) => {
            secrets.scrub_value(&mut body);
            Ok(body)
        }
        Err(err) => Err(secrets.scrub_error(err)),
    })
}

/// `err` with the type and message of the dialog open on the tab
/// `arguments` names, when it is a `dialog_open` that lacks them (a typed
/// CDP failure the host recovered from text, or a tool outside the
/// `browser_*` family). Folds the tab's buffered dialog events first.
pub fn fill_dialog(host: &mut dyn BrowserHost, arguments: &Value, err: ToolError) -> ToolError {
    match &err {
        ToolError::DialogOpen { kind, .. } if kind.is_empty() => {
            browser_extra::dialog_error(host, arguments, err)
        }
        _ => err,
    }
}

/// `(description, properties, required)` for a `browser_*` tool.
pub fn spec(name: &str) -> (String, Map<String, Value>, Vec<&'static str>) {
    if ENV_TOOLS.contains(&name) {
        browser_env::spec(name)
    } else {
        browser_state::spec(name).unwrap_or_else(|| browser_extra::spec(name))
    }
}
