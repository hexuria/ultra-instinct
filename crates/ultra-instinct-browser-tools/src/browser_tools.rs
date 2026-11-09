//! `browser_*` tools — the browser-use-compatible host surface.
//!
//! These tools drive a live CDP endpoint the way browser-use's MCP server
//! does, under the same names. The difference is under the hood: target-bound
//! input (`browser_click`, `browser_type`) still goes through gate → ticket →
//! `ultra_instinct_agent::execute_ticketed`, so a stale page refuses instead of clicking
//! the wrong element. Page-level calls (navigate, scroll, screenshot, exec,
//! tabs) are plain CDP like browser-use's.
//!
//! Element indexes follow browser-use convention: interactive regions in
//! reading order (top-to-bottom, left-to-right), 1-based, recomputed from the
//! current observation on every call. Coordinate clicks are deliberately not
//! offered — the surface acts on region identity, never on guessed pixels.

use serde_json::{json, Value};

use ultra_instinct_agent::{
    classify_delta, classify_value, execute_ticketed_with, BrowserRuntime, ExecError, Input,
};
use ultra_instinct_browser::BrowserError;
use ultra_instinct_cdp::DEFAULT_CDP_HTTP;
use ultra_instinct_core::{
    Action, ActionSpace, InteractionManifold, InteractionRegion, RegionId, Role,
};
use ultra_instinct_guard::{
    check_drop_target, consume_ticket_once_checked, gate, gate_check, gate_drag, host_of,
    ConsumeError, Policy,
};
use ultra_instinct_protocol::GuardReason;

use crate::bound::{self, at_index, ticket_error};
use crate::error::ToolError;
use crate::host::{endpoint_of, keep, session_key, BrowserHost};
use ultra_instinct_cdp::PageTab;

/// All `browser_*` tool names this module serves.
pub const BROWSER_TOOLS: [&str; 37] = [
    "browser_navigate",
    "browser_new_tab",
    "browser_go_back",
    "browser_wait",
    "browser_get_state",
    "browser_get_html",
    "browser_get_text",
    "browser_get_page_text",
    "browser_screenshot",
    "browser_scroll",
    "browser_scroll_to",
    "browser_click",
    "browser_right_click",
    "browser_middle_click",
    "browser_double_click",
    "browser_triple_click",
    "browser_hover",
    "browser_drag",
    "browser_type",
    "browser_form_input",
    "browser_send_key",
    "browser_hold_key",
    "browser_file_upload",
    "browser_save_as_pdf",
    "browser_get_dropdown_options",
    "browser_select_dropdown",
    "browser_search",
    "browser_zoom",
    "browser_find",
    "browser_list_tabs",
    "browser_switch_tab",
    "browser_close_tab",
    "browser_exec",
    "browser_javascript_exec",
    "browser_extract_content",
    "browser_read_console",
    "browser_read_network",
];

/// Diagnostic-domain bits in `BrowserHost::enabled_domains`.
const CONSOLE_BIT: u8 = 0b01;
const NETWORK_BIT: u8 = 0b10;

/// Run one `browser_*` tool against this server's state.
pub(crate) fn call(
    server: &mut dyn BrowserHost,
    name: &str,
    arguments: &Value,
) -> Result<Value, ToolError> {
    match name {
        "browser_navigate" => navigate(server, arguments),
        "browser_new_tab" => new_tab(server, arguments),
        "browser_go_back" => go_back(server, arguments),
        "browser_wait" => wait(server, arguments),
        "browser_get_state" => get_state(server, arguments),
        "browser_get_html" => get_html(server, arguments),
        "browser_get_text" | "browser_get_page_text" => get_text(server, arguments),
        "browser_screenshot" => screenshot(server, arguments),
        "browser_scroll" => scroll(server, arguments),
        "browser_scroll_to" => scroll_to(server, arguments),
        "browser_click" => click(server, arguments),
        "browser_right_click" => gated_press(server, arguments, "right", 1),
        "browser_middle_click" => gated_press(server, arguments, "middle", 1),
        "browser_double_click" => gated_press(server, arguments, "left", 2),
        "browser_triple_click" => gated_press(server, arguments, "left", 3),
        "browser_hover" => hover(server, arguments),
        "browser_drag" => drag(server, arguments),
        "browser_type" | "browser_form_input" => type_text(server, arguments),
        "browser_send_key" => send_key(server, arguments, 0),
        "browser_hold_key" => hold_key(server, arguments),
        "browser_file_upload" => file_upload(server, arguments),
        "browser_save_as_pdf" => save_as_pdf(server, arguments),
        "browser_get_dropdown_options" => get_dropdown_options(server, arguments),
        "browser_select_dropdown" => gated_input(server, arguments, "select"),
        "browser_search" => search(server, arguments),
        "browser_zoom" => zoom(server, arguments),
        "browser_find" => find(server, arguments),
        "browser_list_tabs" => list_tabs(server, arguments),
        "browser_switch_tab" => switch_tab(server, arguments),
        "browser_close_tab" => close_tab(server, arguments),
        "browser_exec" | "browser_javascript_exec" => exec(server, arguments),
        "browser_extract_content" => extract_content(server, arguments),
        "browser_read_console" => read_console(server, arguments),
        "browser_read_network" => read_network(server, arguments),
        other => Err(ToolError::UnknownTool(other.to_owned())),
    }
}

fn opt_str<'a>(arguments: &'a Value, key: &str) -> Result<Option<&'a str>, ToolError> {
    match arguments.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(s)) => Ok(Some(s.as_str())),
        Some(other) => Err(ToolError::InvalidArguments(format!(
            "`{key}` must be a string, got {other}"
        ))),
    }
}

fn req_str<'a>(arguments: &'a Value, key: &str) -> Result<&'a str, ToolError> {
    opt_str(arguments, key)?
        .ok_or_else(|| ToolError::InvalidArguments(format!("missing required `{key}`")))
}

fn opt_usize(arguments: &Value, key: &str) -> Result<Option<usize>, ToolError> {
    match arguments.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::Number(n)) => n
            .as_u64()
            .map(|v| usize::try_from(v).unwrap_or(usize::MAX))
            .map(Some)
            .ok_or_else(|| {
                ToolError::InvalidArguments(format!("`{key}` must be a non-negative integer"))
            }),
        Some(other) => Err(ToolError::InvalidArguments(format!(
            "`{key}` must be a non-negative integer, got {other}"
        ))),
    }
}

/// A 1-based element index: `5`, `"5"`, or the `@e5` ref the text state prints.
fn opt_index(arguments: &Value, key: &str) -> Result<Option<usize>, ToolError> {
    match arguments.get(key) {
        Some(Value::String(raw)) => {
            let trimmed = raw.trim();
            let digits = trimmed.strip_prefix("@e").unwrap_or(trimmed);
            digits.parse::<usize>().map(Some).map_err(|_| {
                ToolError::InvalidArguments(format!(
                    "`{key}` must be an index like 5, \"5\" or \"@e5\", got {raw:?}"
                ))
            })
        }
        _ => opt_usize(arguments, key),
    }
}

fn opt_f64(arguments: &Value, key: &str) -> Result<Option<f64>, ToolError> {
    match arguments.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::Number(n)) => n
            .as_f64()
            .map(Some)
            .ok_or_else(|| ToolError::InvalidArguments(format!("`{key}` must be finite"))),
        Some(other) => Err(ToolError::InvalidArguments(format!(
            "`{key}` must be a number, got {other}"
        ))),
    }
}

fn opt_bool(arguments: &Value, key: &str) -> Result<Option<bool>, ToolError> {
    match arguments.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::Bool(b)) => Ok(Some(*b)),
        Some(other) => Err(ToolError::InvalidArguments(format!(
            "`{key}` must be a boolean, got {other}"
        ))),
    }
}

fn browser_err(err: impl ToString) -> ToolError {
    ToolError::Browser(err.to_string())
}

/// Executor failures: ticket/gate refusals are `TicketInvalid` (not stale,
/// never silently retried); observe/dispatch/page-rejection are `Browser`.
fn exec_error(err: ExecError) -> ToolError {
    match err {
        ExecError::Ticket(invalid) => ticket_error(invalid, err.to_string()),
        ExecError::Gate(_) => ToolError::TicketInvalid(err.to_string()),
        ExecError::Observe(_) | ExecError::Rejected(_) | ExecError::Dispatch(_) => browser_err(err),
        other => browser_err(other),
    }
}

/// Host-side consume failures: invalid lease → `TicketInvalid`, page-side
/// press failure → `Browser` (the ticket is already consumed either way).
fn consume_error(err: ConsumeError<BrowserError>) -> ToolError {
    match err {
        ConsumeError::Invalid(invalid) => ticket_error(invalid, err.to_string()),
        ConsumeError::Refused(reason) => refused(reason, ""),
        ConsumeError::Press(e) => browser_err(e),
        other => browser_err(other),
    }
}

/// A policy refusal as the tool error a host matches on (not stale; a
/// retry gets the same answer): `needs-confirm: "Pay $18"` (the label is a
/// JSON string; pass it back as `confirm`) or `domain-not-allowed: <host>`.
pub(crate) fn refused(reason: GuardReason, label_or_url: &str) -> ToolError {
    match reason {
        GuardReason::NeedsConfirm if !label_or_url.is_empty() => ToolError::NeedsConfirm {
            label: label_or_url.to_owned(),
        },
        GuardReason::DomainNotAllowed if !label_or_url.is_empty() => ToolError::DomainNotAllowed {
            host: host_of(label_or_url).unwrap_or_else(|| label_or_url.to_owned()),
        },
        _ if label_or_url.is_empty() => ToolError::TicketInvalid(reason.to_string()),
        _ => ToolError::TicketInvalid(format!("{reason}: {label_or_url}")),
    }
}

/// The server's policy plus this call's `confirm` label, if any.
fn press_policy(server: &dyn BrowserHost, arguments: &Value) -> Result<Policy, ToolError> {
    let policy = server.policy()?.clone();
    Ok(match opt_str(arguments, "confirm")? {
        Some(label) => policy.with_confirmed(label),
        None => policy,
    })
}

/// Refuse a navigation the policy does not allow, before any CDP call.
fn check_navigation(server: &dyn BrowserHost, url: &str) -> Result<(), ToolError> {
    server
        .policy()?
        .check_url(url)
        .map_err(|reason| refused(reason, url))
}

/// Borrow-free take/keep: the session for `key`, reconnected when absent.
fn take(server: &mut dyn BrowserHost, key: &str) -> Result<crate::host::LiveSession, ToolError> {
    server.take_session(key)
}

fn navigate(server: &mut dyn BrowserHost, arguments: &Value) -> Result<Value, ToolError> {
    let endpoint = endpoint_of(arguments)?;
    let url = req_str(arguments, "url")?;
    check_navigation(server, url)?;
    if opt_bool(arguments, "new_tab")? == Some(true) {
        return new_tab_at(server, &endpoint, url);
    }
    let key = session_key(server, &endpoint);
    let mut session = take(server, &key)?;
    let result = session.navigate(url);
    let outcome = result.map(|_| {
        let _ = session.ready_state();
        json!({"navigated": url})
    });
    keep(server, &key, session);
    outcome.map_err(browser_err)
}

fn new_tab(server: &mut dyn BrowserHost, arguments: &Value) -> Result<Value, ToolError> {
    let endpoint = endpoint_of(arguments)?;
    let url = opt_str(arguments, "url")?.unwrap_or("about:blank");
    check_navigation(server, url)?;
    new_tab_at(server, &endpoint, url)
}

/// Create the target, point this endpoint's session at it, keep it.
fn new_tab_at(server: &mut dyn BrowserHost, endpoint: &str, url: &str) -> Result<Value, ToolError> {
    let target = server
        .backend()
        .create_page(endpoint, url)
        .map_err(|err| ToolError::cdp(&err))?;
    server.set_current_tab(endpoint, &target.key);
    let mut session = take(server, &target.key)?;
    let _ = session.ready_state();
    keep(server, &target.key, session);
    Ok(json!({
        "tab_id": target.target_id,
        "url": url,
        "new_tab": true,
    }))
}

fn go_back(server: &mut dyn BrowserHost, arguments: &Value) -> Result<Value, ToolError> {
    let endpoint = endpoint_of(arguments)?;
    let key = session_key(server, &endpoint);
    let mut session = take(server, &key)?;
    let outcome = session.go_back().map(|_| {
        let _ = session.ready_state();
        json!({"went_back": true})
    });
    keep(server, &key, session);
    outcome.map_err(browser_err)
}

/// Pause between looks while `browser_wait` has a condition.
const WAIT_LOOK_MS: u64 = 200;

/// What a conditional `browser_wait` waits for. Every given part must hold.
struct WaitFor {
    /// Case-insensitive substring of the page's visible text.
    text: Option<String>,
    /// Substring of the current URL.
    url: Option<String>,
    /// Case-insensitive substring of a control label in the interactive index.
    control: Option<String>,
    /// No control in the interactive index has exactly this label (trimmed,
    /// case-insensitive).
    gone: Option<String>,
    /// `document.readyState` has reached this (`domcontentloaded`:
    /// interactive or complete; `load`: complete).
    load: Option<LoadState>,
}

/// A `browser_wait` `load` condition.
#[derive(Clone, Copy, PartialEq, Eq)]
enum LoadState {
    DomContentLoaded,
    Load,
}

impl LoadState {
    fn parse(raw: &str) -> Result<Self, ToolError> {
        match raw {
            "domcontentloaded" => Ok(Self::DomContentLoaded),
            "load" => Ok(Self::Load),
            other => Err(ToolError::InvalidArguments(format!(
                "`load` must be domcontentloaded|load, got `{other}`"
            ))),
        }
    }

    /// Whether `document.readyState` (None: unreadable, e.g. mid-navigation)
    /// has reached this state.
    fn reached(self, ready: Option<&str>) -> bool {
        matches!(
            (self, ready),
            (_, Some("complete")) | (Self::DomContentLoaded, Some("interactive"))
        )
    }
}

impl WaitFor {
    fn parse(arguments: &Value) -> Result<Option<Self>, ToolError> {
        let part = |key: &str| -> Result<Option<String>, ToolError> {
            match opt_str(arguments, key)? {
                Some(v) if v.trim().is_empty() => Err(ToolError::InvalidArguments(format!(
                    "`{key}` must not be empty"
                ))),
                other => Ok(other.map(str::to_owned)),
            }
        };
        let until = Self {
            text: part("text")?.map(|t| t.to_lowercase()),
            url: part("url")?,
            control: part("control")?.map(|c| c.trim().to_lowercase()),
            gone: part("gone")?.map(|g| collapse_label(&g)),
            load: opt_str(arguments, "load")?
                .map(LoadState::parse)
                .transpose()?,
        };
        Ok((until.text.is_some()
            || until.url.is_some()
            || until.control.is_some()
            || until.gone.is_some()
            || until.load.is_some())
        .then_some(until))
    }

    /// One look: observe, then check each part. Page text is read only when
    /// a `text` part was given. Returns (holds, current url).
    fn look(
        &self,
        session: &mut crate::host::LiveSession,
    ) -> Result<(bool, Option<String>), ToolError> {
        session.observe().map_err(browser_err)?;
        let url = session.page().and_then(|p| p.url().map(str::to_owned));
        if let Some(want) = &self.url {
            if !url.as_deref().is_some_and(|u| u.contains(want.as_str())) {
                return Ok((false, url));
            }
        }
        if let Some(want) = &self.control {
            let manifold = session
                .manifold()
                .ok_or_else(|| browser_err(BrowserError::NotObserved))?;
            let present = interactive_index(manifold).iter().any(|id| {
                manifold
                    .get(id)
                    .is_some_and(|r| r.label().to_lowercase().contains(want.as_str()))
            });
            if !present {
                return Ok((false, url));
            }
        }
        if let Some(gone) = &self.gone {
            let manifold = session
                .manifold()
                .ok_or_else(|| browser_err(BrowserError::NotObserved))?;
            let present = interactive_index(manifold).iter().any(|id| {
                manifold
                    .get(id)
                    .is_some_and(|r| collapse_label(r.label()) == *gone)
            });
            if present {
                return Ok((false, url));
            }
        }
        if let Some(load) = self.load {
            let ready = session.ready_state().map_err(browser_err)?;
            if !load.reached(ready.as_deref()) {
                return Ok((false, url));
            }
        }
        if let Some(want) = &self.text {
            let text = session.page_text().map_err(browser_err)?;
            if !text.to_lowercase().contains(want.as_str()) {
                return Ok((false, url));
            }
        }
        Ok((true, url))
    }
}

/// Trimmed, whitespace collapsed, lowercased: the `gone` label match.
fn collapse_label(label: &str) -> String {
    label
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase()
}

/// With no condition: sleep `seconds` (default 3). With `text`, `url`,
/// `control`, `gone`, or `load`: observe about every 200 ms until the condition holds or
/// `seconds` (default 10) runs out. Both are capped at 60 s. Never acts.
fn wait(server: &mut dyn BrowserHost, arguments: &Value) -> Result<Value, ToolError> {
    let until = WaitFor::parse(arguments)?;
    let default = if until.is_some() { 10.0 } else { 3.0 };
    let seconds = opt_f64(arguments, "seconds")?.unwrap_or(default);
    // Capped at the host's `max_wait` (default 60 s).
    let seconds = seconds.clamp(0.0, server.limits().max_wait.as_secs_f64());
    let Some(until) = until else {
        std::thread::sleep(std::time::Duration::from_secs_f64(seconds));
        return Ok(json!({"waited": seconds}));
    };
    let endpoint = endpoint_of(arguments)?;
    let key = session_key(server, &endpoint);
    let mut session = take(server, &key)?;
    let started = std::time::Instant::now();
    let limit = std::time::Duration::from_secs_f64(seconds);
    let outcome = (|| {
        let mut looks = 0u32;
        loop {
            let (seen, url) = until.look(&mut session)?;
            looks += 1;
            let elapsed = started.elapsed();
            let after_ms = u64::try_from(elapsed.as_millis()).unwrap_or(u64::MAX);
            if seen {
                return Ok(json!({"seen": true, "after_ms": after_ms, "looks": looks, "url": url}));
            }
            if elapsed >= limit {
                return Ok(
                    json!({"seen": false, "after_ms": after_ms, "looks": looks, "url": url}),
                );
            }
            let pause = std::time::Duration::from_millis(WAIT_LOOK_MS).min(limit - elapsed);
            std::thread::sleep(pause);
        }
    })();
    keep(server, &key, session);
    outcome
}

/// Interactive regions in browser-use reading order: top-to-bottom rows,
/// left-to-right within a row (ties by id). Viability is the action-space
/// rule, so the same index always maps the same way for every tool.
pub(crate) fn interactive_index(manifold: &InteractionManifold) -> Vec<RegionId> {
    let space = ActionSpace::from_manifold(manifold);
    let mut seen = Vec::<RegionId>::new();
    for action in space.actions() {
        let Some(target) = action.target() else {
            continue;
        };
        if !seen.contains(target) {
            seen.push(target.clone());
        }
    }
    let mut regions: Vec<&InteractionRegion> =
        seen.iter().filter_map(|id| manifold.get(id)).collect();
    regions.sort_by(|a, b| {
        let ra = a.rect();
        let rb = b.rect();
        let row_a = (ra.y() / 20.0).floor() as i64;
        let row_b = (rb.y() / 20.0).floor() as i64;
        (row_a, ra.x() as i64, a.id().as_str()).cmp(&(row_b, rb.x() as i64, b.id().as_str()))
    });
    regions.into_iter().map(|r| r.id().clone()).collect()
}

fn element_json(index: usize, region: &InteractionRegion) -> Value {
    let rect = region.rect();
    let mut element = json!({
        "index": index,
        "region": region.id().as_str(),
        "role": region.role().to_string(),
        "label": region.label(),
        "actions": region.actions().iter().map(|a| a.to_string()).collect::<Vec<_>>(),
        "rect": {"x": rect.x(), "y": rect.y(), "w": rect.width(), "h": rect.height()},
        "sensitive": crate::secrets::sensitive(region),
    });
    if let Some(kind) = region.state().input_type.as_deref() {
        element["input_type"] = json!(kind);
    }
    element
}

fn get_state(server: &mut dyn BrowserHost, arguments: &Value) -> Result<Value, ToolError> {
    let endpoint = endpoint_of(arguments)?;
    let text = match opt_str(arguments, "format")?.unwrap_or("text") {
        "text" => true,
        "json" => false,
        other => {
            return Err(ToolError::InvalidArguments(format!(
                "`format` must be text|json, got `{other}`"
            )))
        }
    };
    let key = session_key(server, &endpoint);
    let mut session = take(server, &key)?;
    let observed = (|| {
        session.observe().map_err(browser_err)?;
        // `page()` is written inside `observe` — read it after, not before.
        let page = session.page().cloned();
        let manifold = session
            .manifold()
            .cloned()
            .ok_or_else(|| browser_err(BrowserError::NotObserved))?;
        Ok::<_, ToolError>((page, std::rc::Rc::new(manifold)))
    })();
    keep(server, &key, session);
    let (page, manifold) = observed?;
    let mut order = interactive_index(&manifold);
    let limits = server.limits();
    let dropped = order.len().saturating_sub(limits.max_elements);
    order.truncate(limits.max_elements);
    // Recorded in both formats: index presses may pass this `snapshot` to
    // act on what this call printed, or refuse.
    let snapshot = server
        .record(
            &bound::origin(&key),
            std::rc::Rc::clone(&manifold),
            page.clone()
                .unwrap_or_else(ultra_instinct_browser::PageState::blank),
            order.len(),
        )
        .get();
    if text {
        let mut body = crate::page_text::page_text(page.as_ref(), &manifold, &order, snapshot);
        if dropped > 0 {
            body.push_str(&format!(
                "\n… {dropped} more controls not shown (max_elements)"
            ));
        }
        let cut = crate::browser_extra::truncate_output(&body, limits.max_state_chars);
        let truncated = dropped > 0 || cut.len() != body.len();
        let mut out = json!({"format": "text", "text": cut, "snapshot": snapshot});
        if truncated {
            out["truncated"] = json!(true);
            out["dropped_elements"] = json!(dropped);
        }
        return Ok(out);
    }
    let elements: Vec<Value> = order
        .iter()
        .enumerate()
        .filter_map(|(i, id)| manifold.get(id).map(|r| element_json(i + 1, r)))
        .collect();
    let mut out = json!({
        "url": page.as_ref().and_then(|p| p.url().map(str::to_owned)),
        "title": page.as_ref().and_then(|p| p.title().map(str::to_owned)),
        "interactive_elements": elements.len(),
        "elements": elements,
        "snapshot": snapshot,
    });
    if dropped > 0 {
        out["truncated"] = json!(true);
        out["dropped_elements"] = json!(dropped);
    }
    Ok(out)
}

fn get_html(server: &mut dyn BrowserHost, arguments: &Value) -> Result<Value, ToolError> {
    let endpoint = endpoint_of(arguments)?;
    let key = session_key(server, &endpoint);
    let mut session = take(server, &key)?;
    // A copy without text inputs' `value` attributes (crate::secrets).
    let max = server.limits().max_text_chars;
    let outcome = session
        .evaluate(&capped_js(crate::secrets::SAFE_HTML_JS, max))
        .map(|html| capped_body("html", &html))
        .map_err(browser_err);
    keep(server, &key, session);
    outcome
}

/// `expression`'s text cut to `max` UTF-16 units in the page (never inside
/// a surrogate pair), as `{t, cut}`: the cut text and how much was cut.
/// The limit is the host's number, not caller text.
fn capped_js(expression: &str, max: usize) -> String {
    format!(
        "(function(n){{var t=String({expression});if(t.length<=n)return {{t:t,cut:0}};var c=n;if(c>0){{var k=t.charCodeAt(c-1);if(k>=0xD800&&k<=0xDBFF)c--;}}return {{t:t.slice(0,c),cut:t.length-c}};}})({max})"
    )
}

/// `{field: text}` from a [`capped_js`] value (or a plain string), with the
/// truncation marker and `truncated: true` when the page cut it.
fn capped_body(field: &str, value: &Value) -> Value {
    let text = value
        .get("t")
        .or(Some(value))
        .and_then(Value::as_str)
        .unwrap_or_default();
    match value.get("cut").and_then(Value::as_u64).filter(|&n| n > 0) {
        Some(cut) => json!({
            field: format!("{text}{}", crate::browser_extra::truncated_marker(cut as usize)),
            "truncated": true,
        }),
        None => json!({ field: text }),
    }
}

fn get_text(server: &mut dyn BrowserHost, arguments: &Value) -> Result<Value, ToolError> {
    let endpoint = endpoint_of(arguments)?;
    let key = session_key(server, &endpoint);
    let mut session = take(server, &key)?;
    let max = server.limits().max_text_chars;
    let outcome = session
        .evaluate(&capped_js(
            "document.body ? document.body.innerText : ''",
            max,
        ))
        .map(|text| capped_body("text", &text))
        .map_err(browser_err);
    keep(server, &key, session);
    outcome
}

fn screenshot(server: &mut dyn BrowserHost, arguments: &Value) -> Result<Value, ToolError> {
    let endpoint = endpoint_of(arguments)?;
    let path = opt_str(arguments, "path")?
        .map(crate::host::absolute_path)
        .transpose()?;
    let limit = server.limits().max_screenshot_bytes;
    let key = session_key(server, &endpoint);
    let mut session = take(server, &key)?;
    // Over the byte cap: once more as a JPEG, then `output_too_large`.
    // Sensitive field values are hidden around it (`capture_hidden`).
    let outcome = crate::secrets::capture_hidden(server, &mut session, |session| {
        let png = session.screenshot_png().map_err(browser_err)?;
        if decoded_len(&png) <= limit {
            return Ok(json!({"format": "png", "base64": png}));
        }
        let jpeg = session.screenshot_jpeg().map_err(browser_err)?;
        match jpeg {
            Some(jpeg) if decoded_len(&jpeg) <= limit => {
                Ok(json!({"format": "jpeg", "base64": jpeg}))
            }
            other => Err(ToolError::OutputTooLarge {
                limit,
                bytes: decoded_len(other.as_deref().unwrap_or(&png)),
            }),
        }
    });
    keep(server, &key, session);
    save_artifact(outcome?, "base64", path)
}

/// Bytes a base64 string decodes to (padding aside).
fn decoded_len(base64: &str) -> usize {
    base64.len() / 4 * 3
}

/// With a `path`, writes the decoded base64 in `body[field]` there and
/// records `path` and `bytes`. The body keeps the base64 either way.
fn save_artifact(
    mut body: Value,
    field: &str,
    path: Option<&std::path::Path>,
) -> Result<Value, ToolError> {
    use base64::Engine as _;
    let Some(path) = path else {
        return Ok(body);
    };
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(body[field].as_str().unwrap_or_default())
        .map_err(browser_err)?;
    std::fs::write(path, &bytes)
        .map_err(|err| ToolError::InvalidArguments(format!("`path` {path:?}: {err}")))?;
    body["path"] = json!(path.display().to_string());
    body["bytes"] = json!(bytes.len());
    Ok(body)
}

fn scroll(server: &mut dyn BrowserHost, arguments: &Value) -> Result<Value, ToolError> {
    let endpoint = endpoint_of(arguments)?;
    let down = opt_bool(arguments, "down")?.unwrap_or(true);
    let pages = opt_f64(arguments, "pages")?.unwrap_or(1.0);
    let key = session_key(server, &endpoint);
    let mut session = take(server, &key)?;
    // scroll_pages needs a stored observation for the viewport height.
    let outcome = (|| {
        if session.manifold().is_none() {
            session.observe().map_err(browser_err)?;
        }
        session
            .scroll_pages(down, pages)
            .map(|_| json!({"scrolled": if down { "down" } else { "up" }, "pages": pages}))
            .map_err(browser_err)
    })();
    keep(server, &key, session);
    outcome
}

/// One gated, ticketed input against `index`. The executor re-observes,
/// revalidates the target-scoped world, re-gates, consumes, then dispatches —
/// a page that drifted between get_state and this call refuses, it does not
/// click the wrong element.
fn gated_input(
    server: &mut dyn BrowserHost,
    arguments: &Value,
    input_kind: &str,
) -> Result<Value, ToolError> {
    let endpoint = endpoint_of(arguments)?;
    let index = opt_index(arguments, "index")?
        .ok_or_else(|| ToolError::InvalidArguments("missing required `index`".into()))?;
    let snapshot = bound::required_snapshot(server, arguments)?;
    let input = match input_kind {
        // DOM-side click by default, so existing hosts see no change;
        // `mouse: true` presses like the agent loop does: a trusted pointer
        // at the center, which moves focus and fires pointerdown / mousedown.
        "click" if opt_bool(arguments, "mouse")? == Some(true) => Input::Click,
        "click" => Input::ScriptClick,
        "type" => Input::Type(req_str(arguments, "text")?.to_owned()),
        "select" => Input::Select(req_str(arguments, "text")?.to_owned()),
        "key" => Input::Key(req_str(arguments, "key")?.to_owned()),
        other => return Err(ToolError::UnknownTool(other.to_owned())),
    };
    let action = input.action();
    let policy = press_policy(server, arguments)?;
    let verify = opt_bool(arguments, "verify")? == Some(true);
    let key = session_key(server, &endpoint);
    let mut session = take(server, &key)?;
    let outcome = (|| {
        session.observe().map_err(browser_err)?;
        let focused = session.page().and_then(|p| p.focused().cloned());
        let url = session.page().and_then(|p| p.url().map(str::to_owned));
        let fresh = session
            .manifold()
            .ok_or_else(|| browser_err(BrowserError::NotObserved))?;
        // The region the caller saw (with `snapshot`), else the one at
        // `index` now; the ticket is minted on that observation.
        let (seen, region_id) = bound::resolve(server, &key, snapshot, fresh, index)?;
        let manifold = &seen;
        let ticket = match &input {
            // A KEY ticket binds its one key (KEY_NAMES) on a region that claims Key.
            Input::Key(name) => ultra_instinct_guard::gate_key(
                manifold,
                &region_id,
                name,
                focused,
                manifold.captured_at_ms(),
            ),
            _ => gate(
                manifold,
                &region_id,
                action,
                focused,
                manifold.captured_at_ms(),
            ),
        }
        .map_err(|reason| ToolError::TicketInvalid(reason.to_string()))?;
        // Text for a sensitive field is masked in every output from now on
        // (before dispatch, so a failed press does not leak it either): the
        // field the caller saw, or the one on this observation, is
        // sensitive. The executor's own observation is checked again at
        // dispatch (`TypingInto`).
        if let Input::Type(text) = &input {
            let secret = [manifold.get(&region_id), fresh.get(&region_id)]
                .into_iter()
                .flatten()
                .any(crate::secrets::sensitive);
            if secret {
                server.tool_state_mut().secrets.remember(text, true)?;
            }
        }
        // The user's policy after the gate, before the ticket is used; the
        // executor checks it again on its fresh observation.
        let label = ticket.target_label.clone();
        if let Some(region) = fresh.get(&region_id).or_else(|| manifold.get(&region_id)) {
            policy
                .check(url.as_deref(), region, action)
                .map_err(|reason| refusal_for(reason, &label, url.as_deref()))?;
        }
        let mut ledger = server.take_ledger(&key);
        let mut typing = crate::secrets::TypingInto {
            session: &mut session,
            secrets: &mut server.tool_state_mut().secrets,
            refused: None,
        };
        let executed = execute_ticketed_with(&mut typing, &mut ledger, &ticket, &input, &policy);
        let refused = typing.refused.take();
        server.keep_ledger(&key, ledger);
        if let Some(full) = refused {
            return Err(full);
        }
        let done = executed.map_err(|err| match err {
            ExecError::Gate(
                reason @ (GuardReason::DomainNotAllowed | GuardReason::NeedsConfirm),
            ) => {
                let fresh_url = session.page().and_then(|p| p.url().map(str::to_owned));
                refusal_for(reason, &label, fresh_url.as_deref())
            }
            other => exec_error(other),
        })?;
        let mut body = json!({
            "acted": input_kind,
            "index": index,
            "region": done.target.as_str(),
            "ticket": done.ticket_id,
        });
        if verify {
            body["verification"] = json!(verify_after(&mut session, &done, &input)?);
        }
        Ok(body)
    })();
    keep(server, &key, session);
    outcome
}

/// [`refused`] with the right detail: the label for a confirm, the URL for
/// a domain.
pub(crate) fn refusal_for(reason: GuardReason, label: &str, url: Option<&str>) -> ToolError {
    match reason {
        GuardReason::NeedsConfirm => refused(reason, label),
        _ => refused(reason, url.unwrap_or("")),
    }
}

/// `verify: true`: settle, observe again, and name what the input did
/// (the agent's verification mapping: a typed value is read back).
fn verify_after(
    session: &mut crate::host::LiveSession,
    done: &ultra_instinct_agent::Executed,
    input: &Input,
) -> Result<&'static str, ToolError> {
    BrowserRuntime::settle_after_input(session, &done.target, input);
    BrowserRuntime::observe(session).map_err(browser_err)?;
    let page_delta = match (done.before_page.as_ref(), BrowserRuntime::page(session)) {
        (Some(before), Some(after)) => Some(session.page_delta_between(before, after)),
        _ => None,
    };
    let value = input
        .payload()
        .and_then(|_| BrowserRuntime::read_value(session, &done.target));
    let after = session
        .manifold()
        .ok_or_else(|| browser_err(BrowserError::NotObserved))?;
    let kind = input
        .payload()
        .and_then(|expected| classify_value(expected, value.as_ref()))
        .unwrap_or_else(|| classify_delta(&done.before, after, page_delta.as_ref()));
    Ok(kind.as_str())
}

fn click(server: &mut dyn BrowserHost, arguments: &Value) -> Result<Value, ToolError> {
    gated_input(server, arguments, "click")
}

fn type_text(server: &mut dyn BrowserHost, arguments: &Value) -> Result<Value, ToolError> {
    gated_input(server, arguments, "type")
}

/// Gate → ticket → `consume_ticket_once` → `press` on the fresh manifold.
/// Same consume-before-press order as `execute_ticketed` for inputs the
/// executor does not carry (right/middle/double/triple click, file upload,
/// drag). Weaker staleness guarantee than `execute_ticketed`: the ticket is
/// minted and consumed on the same observe, so `revalidate` cannot fire —
/// the unguarded window is the CPU-only span between index resolution and
/// press (no I/O), strictly smaller than the executor's mint→re-observe
/// window. A second observe would buy executor parity at ~2× observe cost.
fn gated_press_at(
    server: &mut dyn BrowserHost,
    arguments: &Value,
    mut press: impl FnMut(&mut crate::host::LiveSession, &RegionId) -> Result<(), BrowserError>,
) -> Result<Value, ToolError> {
    let endpoint = endpoint_of(arguments)?;
    let index = opt_index(arguments, "index")?
        .ok_or_else(|| ToolError::InvalidArguments("missing required `index`".into()))?;
    if index == 0 {
        return Err(ToolError::InvalidArguments("`index` is 1-based".into()));
    }
    let snapshot = bound::required_snapshot(server, arguments)?;
    let policy = press_policy(server, arguments)?;
    let key = session_key(server, &endpoint);
    let mut session = take(server, &key)?;
    let outcome = (|| {
        let manifold = session.observe().map_err(browser_err)?.clone();
        let focused = session.page().and_then(|p| p.focused().cloned());
        let url = session.page().and_then(|p| p.url().map(str::to_owned));
        let (seen, region_id) = bound::resolve(server, &key, snapshot, &manifold, index)?;
        let ticket = gate(
            &seen,
            &region_id,
            Action::Click,
            focused.clone(),
            seen.captured_at_ms(),
        )
        .map_err(|reason| ToolError::TicketInvalid(reason.to_string()))?;
        // A ticket minted on the snapshot must still hold on the page now,
        // and the hard gate re-runs on the fresh region (executor order).
        bound::still_seen(&ticket, &manifold, focused.clone())?;
        if let Some(region) = manifold.get(&region_id) {
            gate_check(&manifold, region, Action::Click)
                .map_err(|reason| ToolError::TicketInvalid(reason.to_string()))?;
        }
        let mut ledger = server.take_ledger(&key);
        let result = consume_ticket_once_checked(
            &mut ledger,
            &ticket,
            &manifold,
            focused,
            &policy,
            url.as_deref(),
            |target: &RegionId, _action: Action| press(&mut session, target),
        );
        server.keep_ledger(&key, ledger);
        result
            .map(|_| json!({"acted": true, "index": index, "ticket": ticket.ticket_id}))
            .map_err(|err| match err {
                ConsumeError::Refused(reason) => {
                    refusal_for(reason, &ticket.target_label, url.as_deref())
                }
                other => consume_error(other),
            })
    })();
    keep(server, &key, session);
    outcome
}

fn gated_press(
    server: &mut dyn BrowserHost,
    arguments: &Value,
    button: &'static str,
    count: u32,
) -> Result<Value, ToolError> {
    gated_press_at(server, arguments, |session, target| {
        session.button_click(target, button, count).map(|_| ())
    })
}

/// Hover resolves the index on the fresh manifold and moves the pointer —
/// no press, so no ticket (it reveals state but commits no mutation).
fn hover(server: &mut dyn BrowserHost, arguments: &Value) -> Result<Value, ToolError> {
    let endpoint = endpoint_of(arguments)?;
    let index = opt_index(arguments, "index")?
        .ok_or_else(|| ToolError::InvalidArguments("missing required `index`".into()))?;
    if index == 0 {
        return Err(ToolError::InvalidArguments("`index` is 1-based".into()));
    }
    let snapshot = bound::required_snapshot(server, arguments)?;
    let key = session_key(server, &endpoint);
    let mut session = take(server, &key)?;
    let outcome = (|| {
        let manifold = session.observe().map_err(browser_err)?.clone();
        let focused = session.page().and_then(|p| p.focused().cloned());
        let (seen, region_id) = bound::resolve(server, &key, snapshot, &manifold, index)?;
        if snapshot.is_some() {
            bound::same_region(&seen, &manifold, &region_id, focused)?;
        }
        session
            .hover(&region_id)
            .map(|_| json!({"hovered": index}))
            .map_err(browser_err)
    })();
    keep(server, &key, session);
    outcome
}

/// Element-to-element drag by index (`from_index` → `to_index`).
fn drag(server: &mut dyn BrowserHost, arguments: &Value) -> Result<Value, ToolError> {
    let endpoint = endpoint_of(arguments)?;
    let from = opt_index(arguments, "from_index")?
        .ok_or_else(|| ToolError::InvalidArguments("missing required `from_index`".into()))?;
    let to = opt_index(arguments, "to_index")?
        .ok_or_else(|| ToolError::InvalidArguments("missing required `to_index`".into()))?;
    if from == 0 || to == 0 {
        return Err(ToolError::InvalidArguments("indexes are 1-based".into()));
    }
    let snapshot = bound::required_snapshot(server, arguments)?;
    let policy = press_policy(server, arguments)?;
    let key = session_key(server, &endpoint);
    let mut session = take(server, &key)?;
    let outcome = (|| {
        let fresh = session.observe().map_err(browser_err)?.clone();
        let (manifold, from_id) = bound::resolve(server, &key, snapshot, &fresh, from)?;
        // The drop target reads the same shown prefix as the source.
        let (_, to_id) = bound::resolve(server, &key, snapshot, &fresh, to)?;
        let focused = session.page().and_then(|p| p.focused().cloned());
        let url = session.page().and_then(|p| p.url().map(str::to_owned));
        // Drag is a mutation: the source goes through the same
        // gate → ticket → consume-before-press path as clicks. A drag-marked
        // source binds both ends like the agent's DRAG; any other source
        // keeps the click gate.
        let drag_source = manifold
            .get(&from_id)
            .is_some_and(|r| r.actions().contains(&Action::Drag));
        let ticket = if drag_source {
            gate_drag(
                &manifold,
                &from_id,
                &to_id,
                focused.clone(),
                manifold.captured_at_ms(),
            )
        } else {
            // The release still lands on `to`: it must be a visible,
            // uncovered, reachable drop target like a DRAG destination.
            gate(
                &manifold,
                &from_id,
                Action::Click,
                focused.clone(),
                manifold.captured_at_ms(),
            )
            .and_then(|ticket| {
                let dest = manifold.get(&to_id).ok_or(GuardReason::MissingTarget)?;
                if from_id == to_id {
                    return Err(GuardReason::UnsupportedAction);
                }
                check_drop_target(&manifold, dest).map(|()| ticket)
            })
        }
        .map_err(|reason| ToolError::TicketInvalid(reason.to_string()))?;
        // Both ends as the caller saw them, on the page now.
        bound::still_seen(&ticket, &fresh, focused.clone())?;
        if ticket.dest.is_none() {
            bound::same_region(&manifold, &fresh, &to_id, None)?;
        }
        let mut ledger = server.take_ledger(&key);
        let result = consume_ticket_once_checked(
            &mut ledger,
            &ticket,
            &fresh,
            focused,
            &policy,
            url.as_deref(),
            |target: &RegionId, _action: Action| session.drag(target, &to_id),
        );
        server.keep_ledger(&key, ledger);
        result
            .map(|_| json!({"dragged": {"from": from, "to": to}}))
            .map_err(|err| match err {
                ConsumeError::Refused(reason) => {
                    refusal_for(reason, &ticket.target_label, url.as_deref())
                }
                other => consume_error(other),
            })
    })();
    keep(server, &key, session);
    outcome
}

fn send_key(
    server: &mut dyn BrowserHost,
    arguments: &Value,
    hold_ms: u64,
) -> Result<Value, ToolError> {
    // With an index the key goes to that element as a ticketed KEY.
    if hold_ms == 0 && arguments.get("index").is_some_and(|v| !v.is_null()) {
        return gated_input(server, arguments, "key");
    }
    let endpoint = endpoint_of(arguments)?;
    let key_name = req_str(arguments, "key")?.to_owned();
    let key = session_key(server, &endpoint);
    let mut session = take(server, &key)?;
    let outcome = session
        .key_event(&key_name, hold_ms)
        .map(|_| json!({"key": key_name}))
        .map_err(browser_err);
    keep(server, &key, session);
    outcome
}

fn hold_key(server: &mut dyn BrowserHost, arguments: &Value) -> Result<Value, ToolError> {
    let hold_ms = opt_usize(arguments, "hold_ms")?
        .map(|ms| ms as u64)
        .or_else(|| {
            opt_f64(arguments, "seconds")
                .ok()
                .flatten()
                .map(|s| (s * 1000.0) as u64)
        })
        .unwrap_or(500);
    send_key(server, arguments, hold_ms)
}

fn file_upload(server: &mut dyn BrowserHost, arguments: &Value) -> Result<Value, ToolError> {
    let mut files: Vec<String> = Vec::new();
    if let Some(path) = opt_str(arguments, "path")? {
        files.push(path.to_owned());
    }
    if let Some(Value::Array(paths)) = arguments.get("paths") {
        for path in paths {
            match path {
                Value::String(s) => files.push(s.clone()),
                other => {
                    return Err(ToolError::InvalidArguments(format!(
                        "`paths` entries must be strings, got {other}"
                    )))
                }
            }
        }
    }
    if files.is_empty() {
        return Err(ToolError::InvalidArguments(
            "missing required `path` or `paths`".into(),
        ));
    }
    gated_press_at(server, arguments, move |session, target| {
        session.set_files(target, &files)
    })
}

fn save_as_pdf(server: &mut dyn BrowserHost, arguments: &Value) -> Result<Value, ToolError> {
    let endpoint = endpoint_of(arguments)?;
    let landscape = opt_bool(arguments, "landscape")?.unwrap_or(false);
    let print_background = opt_bool(arguments, "print_background")?.unwrap_or(false);
    let scale = opt_f64(arguments, "scale")?.unwrap_or(1.0);
    let path = opt_str(arguments, "path")?
        .map(crate::host::absolute_path)
        .transpose()?;
    let key = session_key(server, &endpoint);
    let mut session = take(server, &key)?;
    let outcome = crate::secrets::capture_hidden(server, &mut session, |session| {
        session
            .save_pdf(landscape, print_background, scale)
            .map(|data| json!({"data": data}))
            .map_err(browser_err)
    });
    keep(server, &key, session);
    save_artifact(outcome?, "data", path)
}

fn get_dropdown_options(
    server: &mut dyn BrowserHost,
    arguments: &Value,
) -> Result<Value, ToolError> {
    let endpoint = endpoint_of(arguments)?;
    let index = opt_index(arguments, "index")?
        .ok_or_else(|| ToolError::InvalidArguments("missing required `index`".into()))?;
    if index == 0 {
        return Err(ToolError::InvalidArguments("`index` is 1-based".into()));
    }
    let key = session_key(server, &endpoint);
    let mut session = take(server, &key)?;
    let outcome = (|| {
        let manifold = session.observe().map_err(browser_err)?.clone();
        let region_id = at_index(&interactive_index(&manifold), index)?;
        let region = manifold.get(&region_id).ok_or_else(|| {
            ToolError::InvalidArguments(format!("element {index} resolved to a stale region"))
        })?;
        if region.role() != Role::ComboBox {
            return Err(ToolError::InvalidArguments(format!(
                "element {index} is a {:?}, not a dropdown",
                region.role()
            )));
        }
        let state = region.state();
        Ok::<Value, ToolError>(json!({
            "index": index,
            "options": state.options,
            "selected": state.selected,
        }))
    })();
    keep(server, &key, session);
    outcome
}

fn search(server: &mut dyn BrowserHost, arguments: &Value) -> Result<Value, ToolError> {
    let query = req_str(arguments, "query")?;
    let encoded: String = query
        .bytes()
        .flat_map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                vec![b as char]
            }
            b' ' => vec!['+'],
            other => format!("%{other:02X}").chars().collect(),
        })
        .collect();
    let url = format!("https://www.google.com/search?q={encoded}");
    let args = json!({"url": url, "cdp": opt_str(arguments, "cdp")?});
    navigate(server, &args)
}

fn zoom(server: &mut dyn BrowserHost, arguments: &Value) -> Result<Value, ToolError> {
    let endpoint = endpoint_of(arguments)?;
    let factor = match opt_f64(arguments, "factor")? {
        Some(f) => f,
        None => match opt_str(arguments, "direction")?.unwrap_or("reset") {
            "in" => 1.25,
            "out" => 0.8,
            "reset" => 1.0,
            other => {
                return Err(ToolError::InvalidArguments(format!(
                    "`direction` must be in|out|reset, got `{other}`"
                )))
            }
        },
    };
    let key = session_key(server, &endpoint);
    let mut session = take(server, &key)?;
    let outcome = session
        .zoom(factor)
        .map(|_| json!({"zoomed": factor}))
        .map_err(browser_err);
    keep(server, &key, session);
    outcome
}

fn list_tabs(server: &mut dyn BrowserHost, arguments: &Value) -> Result<Value, ToolError> {
    let endpoint = endpoint_of(arguments)?;
    let tabs: Vec<Value> = pages(server, &endpoint)?
        .iter()
        .map(|t| {
            json!({
                "tab_id": t.target_id,
                "url": t.url,
                "title": t.title,
            })
        })
        .collect();
    Ok(json!({"tabs": tabs}))
}

/// The backend's page tabs for `endpoint`.
fn pages(server: &mut dyn BrowserHost, endpoint: &str) -> Result<Vec<PageTab>, ToolError> {
    server
        .backend()
        .list_pages(endpoint)
        .map_err(|err| ToolError::cdp(&err))
}

/// Resolve `tab_id` to exactly one target: an exact id match, else a
/// non-empty prefix that matches exactly one tab. `""` and ambiguous
/// prefixes refuse rather than silently picking the first tab.
fn find_tab<'a>(targets: &'a [PageTab], tab_id: &str) -> Result<&'a PageTab, ToolError> {
    if let Some(t) = targets.iter().find(|t| t.target_id == tab_id) {
        return Ok(t);
    }
    if tab_id.is_empty() {
        return Err(ToolError::InvalidArguments("empty `tab_id`".into()));
    }
    let mut hits = targets.iter().filter(|t| t.target_id.starts_with(tab_id));
    match (hits.next(), hits.next()) {
        (Some(t), None) => Ok(t),
        (None, _) => Err(ToolError::InvalidArguments(format!("no tab `{tab_id}`"))),
        (Some(_), Some(_)) => Err(ToolError::InvalidArguments(format!(
            "`tab_id` `{tab_id}` is ambiguous"
        ))),
    }
}

fn switch_tab(server: &mut dyn BrowserHost, arguments: &Value) -> Result<Value, ToolError> {
    let endpoint = endpoint_of(arguments)?;
    let tab_id = req_str(arguments, "tab_id")?;
    let targets = pages(server, &endpoint)?;
    let target = find_tab(&targets, tab_id)?;
    server
        .backend()
        .activate_page(&endpoint, &target.target_id)
        .map_err(|err| ToolError::cdp(&err))?;
    server.set_current_tab(&endpoint, &target.key);
    Ok(json!({"active_tab": target.target_id, "url": target.url}))
}

fn close_tab(server: &mut dyn BrowserHost, arguments: &Value) -> Result<Value, ToolError> {
    let endpoint = endpoint_of(arguments)?;
    let tab_id = req_str(arguments, "tab_id")?;
    let targets = pages(server, &endpoint)?;
    let target = find_tab(&targets, tab_id)?;
    let closed = server
        .backend()
        .close_page(&endpoint, &target.target_id)
        .map_err(|err| ToolError::cdp(&err))?;
    // The closed tab's session state is dead regardless of binding: drop
    // its session, ledger, events, and domain bits.
    server.drop_session(&target.key);
    // If the closed tab was bound, fall back to any remaining page target.
    if server.current_tab(&endpoint) == Some(target.key.as_str()) {
        match pages(server, &endpoint)?
            .into_iter()
            .find(|t| t.target_id != target.target_id)
            .map(|t| t.key)
        {
            Some(next) => server.set_current_tab(&endpoint, &next),
            None => server.clear_current_tab(&endpoint),
        }
    }
    Ok(json!({"closed": closed, "tab_id": target.target_id}))
}

/// `browser_exec` / `browser_javascript_exec`: Runtime.evaluate, awaitPromise,
/// returnByValue — the raw CDP escape hatch browser-use exposes.
fn exec(server: &mut dyn BrowserHost, arguments: &Value) -> Result<Value, ToolError> {
    let endpoint = endpoint_of(arguments)?;
    let expression = opt_str(arguments, "script")?
        .or(opt_str(arguments, "expression")?)
        .ok_or_else(|| ToolError::InvalidArguments("missing required `script`".to_owned()))?
        .to_owned();
    let key = session_key(server, &endpoint);
    let mut session = take(server, &key)?;
    let outcome = session
        .evaluate(&expression)
        .map(|result| json!({"result": result}))
        .map_err(browser_err);
    keep(server, &key, session);
    outcome
}

/// Deterministic DOM→markdown extraction (no model call).
const EXTRACT_JS: &str = r#"(() => {
  const out = [];
  const title = document.title || '';
  if (title) out.push('# ' + title);
  document.querySelectorAll('h1,h2,h3,h4,h5,h6,p,li,a[href],main,article').forEach(el => {
    const t = (el.innerText || el.textContent || '').replace(/\s+/g, ' ').trim();
    if (!t) return;
    const tag = el.tagName;
    if (/^H[1-6]$/.test(tag)) out.push('\n' + '#'.repeat(Number(tag[1])) + ' ' + t);
    else if (tag === 'A' && el.href) out.push('[' + t + '](' + el.href + ')');
    else if (tag === 'LI') out.push('- ' + t);
    else if (tag !== 'MAIN' && tag !== 'ARTICLE') out.push(t);
  });
  return out.join('\n').slice(0, 30000);
})()"#;

fn extract_content(server: &mut dyn BrowserHost, arguments: &Value) -> Result<Value, ToolError> {
    let endpoint = endpoint_of(arguments)?;
    let key = session_key(server, &endpoint);
    let mut session = take(server, &key)?;
    let outcome = session
        .evaluate(EXTRACT_JS)
        .map(|result| json!({"content": result}))
        .map_err(browser_err);
    keep(server, &key, session);
    outcome
}

/// Scroll the element at `index` into view via its `data-hu-k` identity.
fn scroll_to(server: &mut dyn BrowserHost, arguments: &Value) -> Result<Value, ToolError> {
    let endpoint = endpoint_of(arguments)?;
    let index = opt_index(arguments, "index")?
        .ok_or_else(|| ToolError::InvalidArguments("missing required `index`".to_owned()))?;
    if index == 0 {
        return Err(ToolError::InvalidArguments("`index` is 1-based".to_owned()));
    }
    let key = session_key(server, &endpoint);
    let mut session = take(server, &key)?;
    let outcome = (|| {
        let manifold = session.observe().map_err(browser_err)?;
        let region_id = at_index(&interactive_index(manifold), index)?;
        session
            .scroll_into_view(&region_id)
            .map(|_| json!({"scrolled": true, "index": index}))
            .map_err(browser_err)
    })();
    keep(server, &key, session);
    outcome
}

/// Filter the interactive index by label text and/or role.
fn find(server: &mut dyn BrowserHost, arguments: &Value) -> Result<Value, ToolError> {
    let endpoint = endpoint_of(arguments)?;
    let text = opt_str(arguments, "text")?
        .or(opt_str(arguments, "query")?)
        .map(str::to_lowercase);
    let role = opt_str(arguments, "role")?.map(str::to_lowercase);
    let key = session_key(server, &endpoint);
    let mut session = take(server, &key)?;
    let outcome = (|| {
        let manifold = session.observe().map_err(browser_err)?;
        let order = interactive_index(manifold);
        let mut matches = Vec::new();
        for (i, id) in order.iter().enumerate() {
            let Some(region) = manifold.get(id) else {
                continue;
            };
            let label = region.label().to_lowercase();
            let region_role = region.role().to_string().to_lowercase();
            if let Some(query) = &text {
                if !label.contains(query.as_str()) {
                    continue;
                }
            }
            if let Some(query) = &role {
                if !region_role.contains(query.as_str()) {
                    continue;
                }
            }
            matches.push(element_json(i + 1, region));
            if matches.len() >= 50 {
                break;
            }
        }
        Ok::<Value, ToolError>(json!({"matches": matches, "matched": matches.len()}))
    })();
    keep(server, &key, session);
    outcome
}

/// Lazily enable a diagnostic domain on the session's tab.
fn enable_diagnostics(
    server: &mut dyn BrowserHost,
    key: &str,
    session: &mut crate::host::LiveSession,
    bit: u8,
    domains: &[&str],
) -> Result<(), ToolError> {
    if server.enabled_domains(key) & bit != 0 {
        return Ok(());
    }
    for domain in domains {
        session
            .transport_mut()
            .call(domain, "{}")
            .map_err(|err| ToolError::Browser(err.to_string()))?;
    }
    server.enable_domain(key, bit);
    Ok(())
}

/// Console + log entries buffered since the last read (drains the buffer).
fn read_console(server: &mut dyn BrowserHost, arguments: &Value) -> Result<Value, ToolError> {
    let endpoint = endpoint_of(arguments)?;
    let key = session_key(server, &endpoint);
    let mut session = take(server, &key)?;
    let enabled = enable_diagnostics(
        server,
        &key,
        &mut session,
        CONSOLE_BIT,
        &["Runtime.enable", "Log.enable"],
    );
    let drained = session.transport_mut().drain_events();
    server.push_events(&key, drained);
    keep(server, &key, session);
    enabled?;
    let mut entries = Vec::new();
    let mut rest = Vec::new();
    for event in server.take_events(&key) {
        if let Some(entry) = console_entry(&event) {
            entries.push(entry);
        } else {
            rest.push(event);
        }
    }
    if !rest.is_empty() {
        server.push_events(&key, rest);
    }
    Ok(json!({"entries": entries, "dropped": server.dropped_events(&key)}))
}

/// One console/log entry, when the event is one.
fn console_entry(event: &ultra_instinct_cdp::CdpEvent) -> Option<Value> {
    match event.method.as_str() {
        "Runtime.consoleAPICalled" => {
            let params = &event.params;
            let level = params.get("type").and_then(Value::as_str).unwrap_or("log");
            let text = params
                .get("args")
                .and_then(Value::as_array)
                .map(|args| {
                    args.iter()
                        .map(|arg| {
                            arg.get("value")
                                .cloned()
                                .or_else(|| arg.get("description").cloned())
                                .map(|v| v.to_string())
                                .unwrap_or_default()
                        })
                        .collect::<Vec<_>>()
                        .join(" ")
                })
                .unwrap_or_default();
            Some(json!({"level": level, "text": text}))
        }
        "Runtime.exceptionThrown" => {
            let details = event
                .params
                .get("exceptionDetails")
                .cloned()
                .unwrap_or(Value::Null);
            let text = details
                .get("exception")
                .and_then(|e| e.get("description"))
                .or_else(|| details.get("text"))
                .and_then(Value::as_str)
                .unwrap_or("exception")
                .to_owned();
            Some(json!({"level": "error", "text": text}))
        }
        "Log.entryAdded" => {
            let entry = event.params.get("entry")?;
            Some(json!({
                "level": entry.get("level").and_then(Value::as_str).unwrap_or("info"),
                "text": entry.get("text").and_then(Value::as_str).unwrap_or(""),
                "source": entry.get("source").and_then(Value::as_str).unwrap_or(""),
            }))
        }
        _ => None,
    }
}

/// Network entries buffered since the last read, aggregated by requestId.
fn read_network(server: &mut dyn BrowserHost, arguments: &Value) -> Result<Value, ToolError> {
    let endpoint = endpoint_of(arguments)?;
    let key = session_key(server, &endpoint);
    let mut session = take(server, &key)?;
    let enabled = enable_diagnostics(server, &key, &mut session, NETWORK_BIT, &["Network.enable"]);
    let drained = session.transport_mut().drain_events();
    server.push_events(&key, drained);
    keep(server, &key, session);
    enabled?;
    let mut requests: serde_json::Map<String, Value> = serde_json::Map::new();
    let mut order = Vec::<String>::new();
    let mut rest = Vec::new();
    for event in server.take_events(&key) {
        let Some(id) = event
            .params
            .get("requestId")
            .and_then(Value::as_str)
            .map(str::to_owned)
        else {
            rest.push(event);
            continue;
        };
        match event.method.as_str() {
            "Network.requestWillBeSent" => {
                let request = event.params.get("request").cloned().unwrap_or(Value::Null);
                if !requests.contains_key(&id) {
                    order.push(id.clone());
                }
                let entry = requests.entry(id.clone()).or_insert_with(|| json!({}));
                entry["url"] = json!(request.get("url").and_then(Value::as_str).unwrap_or(""));
                entry["method"] = json!(request
                    .get("method")
                    .and_then(Value::as_str)
                    .unwrap_or("GET"));
                if let Some(redirect) = event.params.get("redirectResponse") {
                    entry["redirect_from"] = json!({
                        "url": redirect.get("url").and_then(Value::as_str).unwrap_or(""),
                        "status": redirect.get("status").cloned().unwrap_or(Value::Null),
                    });
                }
            }
            "Network.responseReceived" => {
                let entry = requests.entry(id.clone()).or_insert_with(|| {
                    order.push(id.clone());
                    json!({})
                });
                let response = event.params.get("response").cloned().unwrap_or(Value::Null);
                entry["status"] = response.get("status").cloned().unwrap_or(Value::Null);
                if entry.get("url").is_none() {
                    entry["url"] = response.get("url").cloned().unwrap_or(Value::Null);
                }
            }
            "Network.loadingFailed" => {
                // Orphan failures still surface — a requestId with no prior
                // requestWillBeSent becomes an error-only row, not silence.
                let entry = requests.entry(id.clone()).or_insert_with(|| {
                    order.push(id.clone());
                    json!({})
                });
                entry["error"] = event
                    .params
                    .get("errorText")
                    .cloned()
                    .unwrap_or(Value::Null);
            }
            _ => rest.push(event),
        }
    }
    let dropped = server.dropped_events(&key);
    if !rest.is_empty() {
        server.push_events(&key, rest);
    }
    let entries: Vec<Value> = order
        .iter()
        .filter_map(|id| {
            requests
                .get(id)
                .map(|e| json!({"requestId": id, "entry": e}))
        })
        .collect();
    Ok(json!({"entries": entries, "dropped": dropped}))
}

/// Schema of an element index: an integer, `"5"`, or `"@e5"`.
fn index_prop() -> Value {
    json!({"type": ["integer", "string"], "description": "1-based index from browser_get_state: 5, \"5\" or \"@e5\""})
}

/// Schema of the `confirm` argument of a press.
fn confirm_prop() -> Value {
    json!({"type": "string", "description": "The label of a press the user confirmed; lifts a needs-confirm refusal (ULTRA_INSTINCT_CONFIRM_PRESS) for that label only."})
}

/// Schema of the `verify` argument of a ticketed input.
fn verify_prop() -> Value {
    json!({"type": "boolean", "description": "Observe again after the input and return `verification` (success, state-changed, navigation, dispatched-no-visible-change, wrong-effect, …)."})
}

/// Tool spec fragments shared by every `browser_*` tool.
pub(crate) fn spec(name: &str) -> (String, serde_json::Map<String, Value>, Vec<&'static str>) {
    let cdp = || {
        (
            "cdp",
            json!({"type": "string", "description": format!("CDP debugging endpoint; default {DEFAULT_CDP_HTTP}")}),
        )
    };
    let mut props = serde_json::Map::new();
    props.insert(cdp().0.to_owned(), cdp().1);
    let mut required = Vec::new();
    let description = match name {
        "browser_navigate" => {
            props.insert("url".into(), json!({"type": "string"}));
            props.insert("new_tab".into(), json!({"type": "boolean"}));
            required.push("url");
            "Navigate the current tab to url; new_tab=true opens it in a new tab."
        }
        "browser_new_tab" => {
            props.insert("url".into(), json!({"type": "string"}));
            "Open a new tab (default about:blank) and make it the current one."
        }
        "browser_go_back" => "Go back one history entry.",
        "browser_wait" => {
            props.insert("seconds".into(), json!({"type": "number"}));
            props.insert("text".into(), json!({"type": "string", "description": "Stop once the page's visible text contains this (case-insensitive)."}));
            props.insert("url".into(), json!({"type": "string", "description": "Stop once the current URL contains this."}));
            props.insert("control".into(), json!({"type": "string", "description": "Stop once a control label in the browser_get_state index contains this (case-insensitive)."}));
            props.insert("gone".into(), json!({"type": "string", "description": "Stop once no control in the browser_get_state index has exactly this label (case-insensitive)."}));
            props.insert("load".into(), json!({"type": "string", "enum": ["domcontentloaded", "load"], "description": "Stop once document.readyState is interactive or complete (domcontentloaded) or complete (load). Not network idle."}));
            "Wait. With text, url, control, gone, or load: observe about every 200 ms until every given condition holds or seconds (default 10) runs out; returns seen, after_ms, looks, and the last url. With no condition: sleep seconds (default 3). Capped at 60 s."
        }
        "browser_get_state" => {
            props.insert(
                "format".into(),
                json!({"type": "string", "enum": ["text", "json"], "description": "text (default): one line per control, `@eN role \"label\"  state`; json: the elements[] body"}),
            );
            "Page title, url, and interactive elements as 1-based indexes in reading order (@e1, @e2, …) — the refs browser_click/browser_type take. Default format text: one short line per control with its value / checked / expanded / selected state; format json returns elements[]."
        }
        "browser_get_html" => "document.documentElement.outerHTML of the current tab.",
        "browser_get_page_text" => "Visible text of the current tab (alias of browser_get_text).",
        "browser_get_text" => "document.body.innerText — visible page text.",
        "browser_screenshot" => {
            props.insert("path".into(), json!({"type": "string"}));
            "PNG screenshot of the viewport, returned as an MCP image. `path` also writes the PNG there."
        }
        "browser_scroll" => {
            props.insert("down".into(), json!({"type": "boolean"}));
            props.insert("pages".into(), json!({"type": "number"}));
            "Scroll pages viewport heights (default 1.0) down or up."
        }
        "browser_click" => {
            props.insert("index".into(), index_prop());
            props.insert("mouse".into(), json!({"type": "boolean"}));
            props.insert("confirm".into(), confirm_prop());
            props.insert("verify".into(), verify_prop());
            required.push("index");
            "Left-click the interactive element at a browser_get_state index, through the ticketed executor (re-observes and refuses on drift). Default is a script click; mouse: true presses a real mouse at its center instead (moves focus, fires mousedown) — use it when a script click does nothing."
        }
        "browser_type" => {
            props.insert("index".into(), index_prop());
            props.insert("text".into(), json!({"type": "string"}));
            props.insert("verify".into(), verify_prop());
            required.extend(["index", "text"]);
            "Set the text of the interactive element at a browser_get_state index, through the ticketed executor."
        }
        "browser_list_tabs" => "List open page tabs (id, url, title).",
        "browser_switch_tab" => {
            props.insert("tab_id".into(), json!({"type": "string"}));
            required.push("tab_id");
            "Activate a tab by id and drive it in later calls."
        }
        "browser_close_tab" => {
            props.insert("tab_id".into(), json!({"type": "string"}));
            required.push("tab_id");
            "Close a tab by id; falls back to another tab if it was current."
        }
        "browser_scroll_to" => {
            props.insert("index".into(), index_prop());
            required.push("index");
            "Scroll the element at a browser_get_state index into view."
        }
        "browser_find" => {
            props.insert("text".into(), json!({"type": "string"}));
            props.insert("role".into(), json!({"type": "string"}));
            "Filter the interactive index by label text and/or role; returns matching elements with their indexes."
        }
        "browser_exec" | "browser_javascript_exec" => {
            // `script` and `expression` are aliases; either satisfies the call.
            props.insert("script".into(), json!({"type": "string"}));
            props.insert("expression".into(), json!({"type": "string"}));
            "Evaluate JavaScript in the tab (Runtime.evaluate, awaitPromise, returnByValue) and return the result."
        }
        "browser_extract_content" => {
            "Extract readable page content as markdown (deterministic DOM walk, no model call)."
        }
        "browser_read_console" => {
            "Drain console/JS-exception entries buffered since the last read; enables Runtime+Log domains lazily."
        }
        "browser_read_network" => {
            "Drain network entries buffered since the last read, aggregated by requestId; enables Network lazily."
        }
        "browser_right_click" | "browser_middle_click" | "browser_double_click"
        | "browser_triple_click" => {
            props.insert("index".into(), index_prop());
            props.insert("confirm".into(), confirm_prop());
            required.push("index");
            "Click the element at a browser_get_state index with the named button/count, through gate + one-shot ticket + consume-before-press."
        }
        "browser_hover" => {
            props.insert("index".into(), index_prop());
            required.push("index");
            "Move the pointer to the element at a browser_get_state index (no press)."
        }
        "browser_drag" => {
            props.insert("from_index".into(), index_prop());
            props.insert("to_index".into(), index_prop());
            props.insert("confirm".into(), confirm_prop());
            required.extend(["from_index", "to_index"]);
            "Press at one element's center, drag to another's, release — both by index."
        }
        "browser_form_input" => {
            props.insert("index".into(), index_prop());
            props.insert("text".into(), json!({"type": "string"}));
            required.extend(["index", "text"]);
            "Alias of browser_type (browser-use form_input name)."
        }
        "browser_send_key" => {
            props.insert("key".into(), json!({"type": "string"}));
            props.insert("index".into(), index_prop());
            props.insert("verify".into(), verify_prop());
            required.push("key");
            "One key event (Enter, Tab, Escape, arrows, letters...) via Input.dispatchKeyEvent. With index: a ticketed KEY on that element (it must take keys; key is one of ArrowLeft, ArrowRight, ArrowUp, ArrowDown, Home, End, PageUp, PageDown, Enter, Escape)."
        }
        "browser_hold_key" => {
            props.insert("key".into(), json!({"type": "string"}));
            props.insert("seconds".into(), json!({"type": "number"}));
            props.insert("hold_ms".into(), json!({"type": "integer"}));
            required.push("key");
            "Hold a key for a duration (seconds or hold_ms, capped 10s)."
        }
        "browser_file_upload" => {
            props.insert("index".into(), index_prop());
            props.insert("path".into(), json!({"type": "string"}));
            props.insert("paths".into(), json!({"type": "array", "items": {"type": "string"}}));
            props.insert("confirm".into(), confirm_prop());
            required.push("index");
            "Set files on the file input at a browser_get_state index (DOM.setFileInputFiles; paths resolved by the Chrome process)."
        }
        "browser_save_as_pdf" => {
            props.insert("landscape".into(), json!({"type": "boolean"}));
            props.insert("print_background".into(), json!({"type": "boolean"}));
            props.insert("scale".into(), json!({"type": "number"}));
            props.insert("path".into(), json!({"type": "string"}));
            "Page.printToPDF → base64 data of the current tab. `path` also writes the PDF there."
        }
        "browser_get_dropdown_options" => {
            props.insert("index".into(), index_prop());
            required.push("index");
            "Observed enabled options (+ selected labels) of the <select> at a browser_get_state index."
        }
        "browser_select_dropdown" => {
            props.insert("index".into(), index_prop());
            props.insert("text".into(), json!({"type": "string"}));
            props.insert("verify".into(), verify_prop());
            required.extend(["index", "text"]);
            "Select exactly one option of the <select> at an index by label/value, through the ticketed executor."
        }
        "browser_search" => {
            props.insert("query".into(), json!({"type": "string"}));
            required.push("query");
            "Navigate the current tab to a Google results URL for query."
        }
        "browser_zoom" => {
            props.insert("factor".into(), json!({"type": "number"}));
            props.insert("direction".into(), json!({"type": "string", "enum": ["in", "out", "reset"]}));
            "Set page zoom: explicit factor (0,10] or direction in(1.25)/out(0.8)/reset(1.0)."
        }
        other => return (other.to_owned(), props, required),
    };
    (description.to_owned(), props, required)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn region(id: &str, x: f64, y: f64, actions: &[Action]) -> InteractionRegion {
        use ultra_instinct_core::{Rect, RegionFlags, RegionParts, Role, SourceMask};
        InteractionRegion::try_new(RegionParts {
            id: RegionId::try_new(id).unwrap(),
            role: Role::Button,
            label: id.to_owned(),
            rect: Rect::try_new(x, y, 50.0, 10.0).unwrap(),
            actions: actions.to_vec(),
            parent: None,
            sources: SourceMask::DOM.union(SourceMask::ACCESSIBILITY),
            flags: RegionFlags::default(),
            temporal_stability: ultra_instinct_core::UnitInterval::ONE,
        })
        .unwrap()
    }

    #[test]
    fn console_entry_parses_console_and_log_events() {
        let console = ultra_instinct_cdp::CdpEvent {
            method: "Runtime.consoleAPICalled".to_owned(),
            params: json!({"type": "warn", "args": [{"value": "careful"}, {"description": "obj"}]}),
        };
        let entry = console_entry(&console).unwrap();
        assert_eq!(entry["level"], "warn");
        assert!(entry["text"].as_str().unwrap().contains("careful"));

        let log = ultra_instinct_cdp::CdpEvent {
            method: "Log.entryAdded".to_owned(),
            params: json!({"entry": {"level": "error", "text": "boom", "source": "network"}}),
        };
        let entry = console_entry(&log).unwrap();
        assert_eq!(entry["level"], "error");
        assert_eq!(entry["text"], "boom");

        let other = ultra_instinct_cdp::CdpEvent {
            method: "Network.requestWillBeSent".to_owned(),
            params: json!({}),
        };
        assert!(console_entry(&other).is_none());
    }

    #[test]
    fn index_is_reading_order_and_stable() {
        use ultra_instinct_core::{InteractionManifold, Rect};
        let manifold = InteractionManifold::try_new(
            Rect::try_new(0.0, 0.0, 800.0, 600.0).unwrap(),
            vec![
                region("b2", 10.0, 40.0, &[Action::Click]),
                region("a1", 10.0, 10.0, &[Action::Click]),
                region("a2", 200.0, 12.0, &[Action::Click]),
            ],
            0,
        )
        .unwrap();
        let order = interactive_index(&manifold);
        let names: Vec<&str> = order.iter().map(RegionId::as_str).collect();
        assert_eq!(names, ["a1", "a2", "b2"]);
        // Deterministic: same manifold → same order.
        assert_eq!(interactive_index(&manifold), order);
    }
}
