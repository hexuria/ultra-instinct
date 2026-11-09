//! `browser_*` MCP tool families for ultra-instinct: the browser-use-compatible
//! host surface (navigate, get_state, click, type, scroll, screenshot, tabs),
//! the agent-browser-style tools (dialog, check, focus, get / is, login state,
//! settings, downloads, network) and the [`ToolError`] every tool returns.
//!
//! The tools run on a [`BrowserHost`] (the MCP `Server` in
//! `ultra-instinct-mcp`). Target-bound input still goes through gate → ticket
//! → `execute_ticketed`, so a stale page refuses instead of clicking the wrong
//! element; page-level calls are plain CDP.

#![forbid(unsafe_code)]

mod args;
mod bound;
mod browser_env;
mod browser_extra;
mod browser_state;
mod browser_tools;
mod error;
mod host;
mod page_text;
mod press;
mod secrets;

pub use browser_env::ENV_TOOLS;
pub use browser_extra::EXTRA_TOOLS;
pub use browser_state::STATE_TOOLS;
pub use browser_tools::BROWSER_TOOLS;
pub use error::ToolError;
pub use host::{BrowserHost, Downloads, LiveSession, ToolLimits, ToolState, Uploads, RUNS_INSIDE};
pub use secrets::{sensitive, MASK, MAX_SENSITIVE_SECRETS};
pub use ultra_instinct_cdp::{CdpConnector, HttpBackend, PageTab, TargetBackend};

use serde_json::{Map, Value};

/// Run a `browser_*` tool on `host`. `None` when `name` is not one; the
/// caller has already checked that `arguments` is an object.
///
/// The result is not masked yet: the host runs every tool's result through
/// [`ToolState::scrub_result`] at its one exit (the MCP `Server` does).
///
/// A tool in [`RUNS_INSIDE`] whose inner tool the host does not allow
/// ([`BrowserHost::allows`]) refuses `tool_not_allowed` before any CDP call.
pub fn call(
    host: &mut dyn BrowserHost,
    name: &str,
    arguments: &Value,
) -> Option<Result<Value, ToolError>> {
    if !name.starts_with("browser_") {
        return None;
    }
    if let Err(err) = host::inner_allowed(host, name) {
        return Some(Err(err));
    }
    let state_after =
        STATE_AFTER.contains(&name) && arguments.get("state") == Some(&Value::Bool(true));
    // `state: true` reads the page too, so it needs `browser_get_state` allowed.
    if state_after && !host.allows("browser_get_state") {
        return Some(Err(ToolError::ToolNotAllowed {
            tool: "browser_get_state".to_owned(),
        }));
    }
    let result = if ENV_TOOLS.contains(&name) {
        browser_env::call(host, name, arguments)
            .map_err(|err| browser_extra::dialog_error(host, arguments, err))
    } else if let Some(result) = browser_state::call(host, name, arguments) {
        result.map_err(|err| browser_extra::dialog_error(host, arguments, err))
    } else {
        browser_extra::call(host, name, arguments)
    };
    Some(match result {
        Ok(body) if state_after => Ok(with_state_after(host, arguments, body)),
        other => other,
    })
}

/// Input tools that take `state: true`: the reply adds the page after the input and its `snapshot`.
pub const STATE_AFTER: [&str; 21] = [
    "browser_navigate",
    "browser_go_back",
    "browser_go_forward",
    "browser_reload",
    "browser_scroll",
    "browser_click",
    "browser_right_click",
    "browser_middle_click",
    "browser_double_click",
    "browser_triple_click",
    "browser_hover",
    "browser_drag",
    "browser_type",
    "browser_form_input",
    "browser_select_dropdown",
    "browser_send_key",
    "browser_file_upload",
    "browser_check",
    "browser_focus",
    "browser_keyboard",
    "browser_download",
];

/// Adds the page after the input; a failed read is `state_error`, not the input's error.
fn with_state_after(host: &mut dyn BrowserHost, arguments: &Value, mut body: Value) -> Value {
    let mut read = Map::new();
    if let Some(cdp) = arguments.get("cdp") {
        read.insert("cdp".into(), cdp.clone());
    }
    read.insert("format".into(), "text".into());
    let read = browser_extra::call(host, "browser_get_state", &Value::Object(read));
    if let Some(map) = body.as_object_mut() {
        match read {
            Ok(state) => {
                map.insert("state".into(), state["text"].clone());
                map.insert("snapshot".into(), state["snapshot"].clone());
            }
            Err(err) => {
                map.insert("state_error".into(), err.to_value());
            }
        }
    }
    body
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

/// The snapshot origin `browser_get_state` records for the tab the `cdp`
/// endpoint drives now (`browser:<session key>`), so a host's guard can
/// take those snapshots as what the caller saw on that tab.
pub fn state_origin(host: &dyn BrowserHost, cdp: &str) -> String {
    bound::origin(&host::session_key(host, cdp))
}

/// `(description, properties, required)` for a `browser_*` tool.
pub fn spec(name: &str) -> (String, Map<String, Value>, Vec<&'static str>) {
    let (description, mut props, required) = if ENV_TOOLS.contains(&name) {
        browser_env::spec(name)
    } else {
        browser_state::spec(name).unwrap_or_else(|| browser_extra::spec(name))
    };
    if STATE_AFTER.contains(&name) {
        props.insert(
            "state".into(),
            serde_json::json!({"type": "boolean",
                "description": "After the input, also return the page as browser_get_state prints it (`state`, text) and its `snapshot` id, for the next press. A failed read is `state_error`; the input still ran."}),
        );
    }
    (description, props, required)
}
