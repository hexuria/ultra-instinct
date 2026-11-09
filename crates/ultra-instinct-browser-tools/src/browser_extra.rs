//! `browser_*` tools ported from vercel-labs/agent-browser, and the pieces
//! every `browser_*` call shares: JavaScript dialog tracking, the dialog
//! hint on a failed call, and the `max_chars` output limit.
//!
//! Every `browser_*` call enters through [`call`]. It enables the Page
//! domain once per tab (lazily, like console and network) so
//! `Page.javascriptDialogOpening` / `Page.javascriptDialogClosed` reach the
//! buffered events, runs the tool, folds those events into the tab's
//! dialog state, appends `a <type> dialog is open; call browser_dialog` to
//! any failure while one is open, and cuts text outputs to `max_chars`.
//! A call that opens a dialog does not wait for it: the websocket stops
//! waiting at `Page.javascriptDialogOpening` (`CdpError::DialogOpened`), so
//! a press that opened an `alert()` fails at once with the hint. The press
//! was dispatched: its ticket stays consumed and nothing retries it.
//!
//! Element presses here keep the boundary of `browser_tools`: `browser_check`
//! presses through `execute_ticketed`, `browser_focus` through gate → ticket
//! → `consume_ticket_once`. Payloads (prompt text, typed text, overlay boxes)
//! are CDP arguments, never script source.
//!
//! Index arguments take `5`, `"5"` or `"@e5"`. They resolve against the
//! tab's last observation when no input ran since (the list the host last
//! saw), else against a fresh one; the tool then acts on, or answers from,
//! that region in a fresh observation, so a control that turned disabled or
//! went away since is reported, not silently swapped for its neighbour.

use std::borrow::Cow;
use std::rc::Rc;

use serde_json::{json, Map, Value};

use ultra_instinct_agent::{execute_ticketed_with, Input};
use ultra_instinct_browser::BrowserError;
use ultra_instinct_cdp::{CdpError, CdpEvent, DEFAULT_CDP_HTTP};
use ultra_instinct_core::{Action, InteractionManifold, InteractionRegion, RegionId, Role};
use ultra_instinct_guard::{consume_ticket_once_checked, gate, gate_check};

use crate::args::{browser_err, opt_bool, opt_str, opt_usize};
use crate::bound::{self, at_index};
use crate::browser_tools::interactive_index;
use crate::error::ToolError;
use crate::host::{endpoint_of, keep, session_key, BrowserHost, LiveSession};
use crate::press::{consume_error, exec_error, press_policy};

/// The tools this module serves itself.
pub const EXTRA_TOOLS: [&str; 9] = [
    "browser_dialog",
    "browser_go_forward",
    "browser_reload",
    "browser_check",
    "browser_focus",
    "browser_keyboard",
    "browser_get",
    "browser_is",
    "browser_count",
];

/// Tools whose text output `max_chars` cuts (see [`limit_output`]).
const OUTPUT_TOOLS: [&str; 7] = [
    "browser_get_state",
    "browser_get_text",
    "browser_get_page_text",
    "browser_get_html",
    "browser_extract_content",
    "browser_read_console",
    "browser_read_network",
];

/// `BrowserHost::enabled_domains` bit for the Page domain (console 0b01, network 0b10).
const PAGE_BIT: u8 = 0b100;

/// True when the page has a focused element other than the document itself.
const FOCUSED_JS: &str = "(function(){var a=document.activeElement;return !!a&&a!==document.body&&a!==document.documentElement;})()";

/// The one annotation overlay script, called on `document`. With boxes
/// (`[[x, y, w, h, label], ...]`, viewport px) it replaces the overlay; with
/// `null` it only removes it. Boxes arrive as a CDP argument.
const OVERLAY_FUNCTION: &str = "function(boxes){var old=this.getElementById('__aui_annotate');if(old)old.remove();if(!boxes)return 0;var root=this.createElement('div');root.id='__aui_annotate';root.setAttribute('style','position:fixed;left:0;top:0;width:0;height:0;z-index:2147483647;pointer-events:none');for(var i=0;i<boxes.length;i++){var b=boxes[i];var box=this.createElement('div');box.setAttribute('style','position:fixed;box-sizing:border-box;border:2px solid #e5195e;pointer-events:none;left:'+Number(b[0])+'px;top:'+Number(b[1])+'px;width:'+Number(b[2])+'px;height:'+Number(b[3])+'px');var tag=this.createElement('span');tag.textContent=String(b[4]);tag.setAttribute('style','position:absolute;left:0;top:0;font:bold 11px/14px monospace;background:#e5195e;color:#fff;padding:0 3px');box.appendChild(tag);root.appendChild(box);}this.documentElement.appendChild(root);return boxes.length;}";

/// A JavaScript dialog open on one tab, from `Page.javascriptDialogOpening`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Dialog {
    kind: String,
    message: String,
    url: String,
    default_prompt: String,
}

impl Dialog {
    fn from_params(params: &Value) -> Self {
        let field = |key: &str| {
            params
                .get(key)
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_owned()
        };
        Self {
            kind: field("type"),
            message: field("message"),
            url: field("url"),
            default_prompt: field("defaultPrompt"),
        }
    }
}

/// `text` cut to at most `max_chars` chars, never inside a char, with
/// `… [truncated N chars]` appended when N chars were cut.
pub(crate) fn truncate_output(text: &str, max_chars: usize) -> Cow<'_, str> {
    match text.char_indices().nth(max_chars) {
        None => Cow::Borrowed(text),
        Some((cut, _)) => {
            let dropped = text[cut..].chars().count();
            Cow::Owned(format!("{}{}", &text[..cut], truncated_marker(dropped)))
        }
    }
}

pub(crate) fn truncated_marker(dropped: usize) -> String {
    format!("… [truncated {dropped} chars]")
}

/// Run one `browser_*` tool with dialog tracking and the output limit.
pub(crate) fn call(
    server: &mut dyn BrowserHost,
    name: &str,
    arguments: &Value,
) -> Result<Value, ToolError> {
    // A malformed `cdp` is the tool's own argument error.
    let Ok(endpoint) = endpoint_of(arguments) else {
        return crate::browser_tools::call(server, name, arguments);
    };
    let key = session_key(server, &endpoint);
    if drives_page(name, arguments) {
        enable_page_events(server, &key)?;
    }
    let result = match name {
        "browser_dialog" => dialog(server, &key, arguments),
        "browser_go_forward" => go_forward(server, &key),
        "browser_reload" => reload(server, &key, arguments),
        "browser_check" => check(server, &key, arguments),
        "browser_focus" => focus(server, &key, arguments),
        "browser_keyboard" => keyboard(server, &key, arguments),
        "browser_get" => get(server, &key, arguments),
        "browser_is" => is(server, &key, arguments),
        "browser_count" => count(server, &key, arguments),
        "browser_screenshot" if opt_bool(arguments, "annotate")? == Some(true) => {
            annotated_screenshot(server, &key, arguments)
        }
        _ => crate::browser_tools::call(server, name, arguments),
    };
    fold_dialog_events(server, &key);
    let after = session_key(server, &endpoint);
    if after != key {
        fold_dialog_events(server, &after);
    }
    match result {
        Ok(body) => limit_output(name, arguments, body),
        Err(err) if name == "browser_dialog" => Err(err),
        Err(err) => Err(with_dialog_hint(server, &key, err)),
    }
}

/// Calls that drive the bound tab's session (not tab management or a sleep).
fn drives_page(name: &str, arguments: &Value) -> bool {
    let tab_level = matches!(
        name,
        "browser_wait"
            | "browser_new_tab"
            | "browser_list_tabs"
            | "browser_switch_tab"
            | "browser_close_tab"
    );
    let opens_tab = name == "browser_navigate"
        && arguments.get("new_tab").and_then(Value::as_bool) == Some(true);
    !tab_level && !opens_tab
}

/// `Page.enable` once per tab, so dialog events are buffered. A transport
/// that cannot enable it (a replay script that never scripted the call)
/// leaves dialogs untracked; the next call tries again.
fn enable_page_events(server: &mut dyn BrowserHost, key: &str) -> Result<(), ToolError> {
    if server.enabled_domains(key) & PAGE_BIT != 0 {
        return Ok(());
    }
    let mut session = server.take_session(key)?;
    // A dialog already open is reported as the enable runs: Page is on.
    if matches!(
        session.transport_mut().call("Page.enable", "{}"),
        Ok(_) | Err(CdpError::DialogOpened { .. })
    ) {
        server.enable_domain(key, PAGE_BIT);
    }
    keep(server, key, session);
    Ok(())
}

/// Fold buffered dialog events into `key`'s dialog state. Other Page events
/// are dropped: only this module enables Page, and nothing reads them.
fn fold_dialog_events(server: &mut dyn BrowserHost, key: &str) {
    let events = server.take_events(key);
    let mut rest: Vec<CdpEvent> = Vec::with_capacity(events.len());
    for event in events {
        match event.method.as_str() {
            "Page.javascriptDialogOpening" => {
                server.set_dialog(key, Some(Dialog::from_params(&event.params)));
            }
            "Page.javascriptDialogClosed" => server.set_dialog(key, None),
            method if method.starts_with("Page.") => {}
            _ => rest.push(event),
        }
    }
    if !rest.is_empty() {
        server.push_events(key, rest);
    }
}

/// A failed env / state tool (which skip [`call`]'s wrapper): fold the
/// tab's buffered dialog events and add the hint, so a dialog its press
/// opened reports its type and message like every other tool's.
pub(crate) fn dialog_error(
    server: &mut dyn BrowserHost,
    arguments: &Value,
    err: ToolError,
) -> ToolError {
    let Ok(endpoint) = endpoint_of(arguments) else {
        return err;
    };
    let key = session_key(server, &endpoint);
    fold_dialog_events(server, &key);
    with_dialog_hint(server, &key, err)
}

/// While a dialog is open the page answers nothing else: say so.
fn with_dialog_hint(server: &dyn BrowserHost, key: &str, err: ToolError) -> ToolError {
    let Some(dialog) = server.dialog(key) else {
        return err;
    };
    let hint = format!("a {} dialog is open; call browser_dialog", dialog.kind);
    let open = |detail: String| ToolError::DialogOpen {
        kind: dialog.kind.clone(),
        message: dialog.message.clone(),
        detail,
    };
    match err {
        ToolError::Browser(message)
        | ToolError::DialogOpen {
            detail: message, ..
        } => open(format!("{message}; {hint}")),
        ToolError::TicketInvalid(message) => ToolError::TicketInvalid(format!("{message}; {hint}")),
        ToolError::InvalidArguments(message) => {
            ToolError::InvalidArguments(format!("{message}; {hint}"))
        }
        // Typed refusals already say what refused; a dead pipe or tab is
        // the cause, not the dialog.
        typed @ (ToolError::NeedsConfirm { .. }
        | ToolError::DomainNotAllowed { .. }
        | ToolError::StaleRef { .. }
        | ToolError::UnknownRef { .. }
        | ToolError::SnapshotRequired
        | ToolError::PipeClosed { .. }
        | ToolError::FrameTooLarge { .. }
        | ToolError::TargetGone { .. }) => typed,
        other => open(format!("{other}; {hint}")),
    }
}

/// Cut the text field of an output tool to `max_chars`. A list (elements,
/// entries) keeps whole entries that fit and reports the cut in `truncated`.
fn limit_output(name: &str, arguments: &Value, mut body: Value) -> Result<Value, ToolError> {
    let Some(max) = opt_usize(arguments, "max_chars")? else {
        return Ok(body);
    };
    let field = match name {
        "browser_get_text" | "browser_get_page_text" => "text",
        "browser_get_html" => "html",
        "browser_extract_content" => "content",
        "browser_get_state" if body.get("text").is_some_and(Value::is_string) => "text",
        "browser_get_state" => "elements",
        "browser_read_console" | "browser_read_network" => "entries",
        _ => return Ok(body),
    };
    let mut dropped = 0usize;
    match body.get_mut(field) {
        Some(Value::String(text)) => {
            if let Cow::Owned(cut) = truncate_output(text, max) {
                *text = cut;
            }
        }
        Some(Value::Array(items)) => {
            let mut used = 0usize;
            let mut kept = items.len();
            for (i, item) in items.iter().enumerate() {
                let size = item.to_string().chars().count() + 1;
                if kept == items.len() && used + size > max {
                    kept = i;
                }
                if kept == items.len() {
                    used += size;
                } else {
                    dropped += size;
                }
            }
            items.truncate(kept);
        }
        _ => {}
    }
    if dropped > 0 {
        body["truncated"] = json!(truncated_marker(dropped));
    }
    Ok(body)
}

fn dialog(server: &mut dyn BrowserHost, key: &str, arguments: &Value) -> Result<Value, ToolError> {
    let action = opt_str(arguments, "action")?.unwrap_or("status");
    let accept = match action {
        "status" => None,
        "accept" => Some(true),
        "dismiss" => Some(false),
        other => {
            return Err(ToolError::InvalidArguments(format!(
                "`action` must be status|accept|dismiss, got `{other}`"
            )))
        }
    };
    let text = opt_str(arguments, "text")?;
    // The dialog being answered: its close event arrives with the answer.
    let answering = server.dialog(key).cloned();
    let mut session = server.take_session(key)?;
    let answered = accept.map(|accept| {
        let mut params = json!({ "accept": accept });
        if let Some(text) = text {
            params["promptText"] = json!(text);
        }
        session.page_input("Page.handleJavaScriptDialog", &params.to_string())
    });
    keep(server, key, session);
    fold_dialog_events(server, key);
    let open = server.dialog(key).cloned();
    let describe = |dialog: &Option<Dialog>| {
        let field = |f: fn(&Dialog) -> &str| dialog.as_ref().map(|d| f(d).to_owned());
        json!({
            "type": field(|d| &d.kind),
            "message": field(|d| &d.message),
            "url": field(|d| &d.url),
            "default_prompt": field(|d| &d.default_prompt),
        })
    };
    let Some(answered) = answered else {
        let mut body = describe(&open);
        body["open"] = json!(open.is_some());
        body["tracked"] = json!(server.enabled_domains(key) & PAGE_BIT != 0);
        return Ok(body);
    };
    answered.map_err(browser_err)?;
    server.set_dialog(key, None);
    let mut body = describe(&answering.or(open));
    body["handled"] = json!(action);
    body["open"] = json!(false);
    Ok(body)
}

fn go_forward(server: &mut dyn BrowserHost, key: &str) -> Result<Value, ToolError> {
    let mut session = server.take_session(key)?;
    let outcome = (|| {
        let body = session
            .transport_mut()
            .call("Page.getNavigationHistory", "{}")
            .map_err(browser_err)?;
        let history: Value = serde_json::from_str(&body).map_err(browser_err)?;
        // `currentIndex` indexes `entries`; entry ids are opaque.
        let next = history["currentIndex"]
            .as_u64()
            .and_then(|current| history["entries"].get(usize::try_from(current + 1).ok()?))
            .and_then(|entry| entry["id"].as_i64())
            .ok_or_else(|| ToolError::Browser("no later history entry".into()))?;
        session
            .page_input(
                "Page.navigateToHistoryEntry",
                &json!({ "entryId": next }).to_string(),
            )
            .map_err(browser_err)?;
        let _ = session.ready_state();
        Ok(json!({"went_forward": true}))
    })();
    keep(server, key, session);
    outcome
}

fn reload(server: &mut dyn BrowserHost, key: &str, arguments: &Value) -> Result<Value, ToolError> {
    let ignore_cache = opt_bool(arguments, "ignore_cache")?.unwrap_or(false);
    let mut session = server.take_session(key)?;
    let outcome = session
        .page_input(
            "Page.reload",
            &json!({ "ignoreCache": ignore_cache }).to_string(),
        )
        .map(|_| {
            let _ = session.ready_state();
            json!({"reloaded": true})
        })
        .map_err(browser_err);
    keep(server, key, session);
    outcome
}

/// Set a checkbox / radio / switch to `checked`: no press when the page
/// shows it so now, else the `browser_click` press, then a read-back that
/// must agree. With `snapshot` the element must still be the one shown.
fn check(server: &mut dyn BrowserHost, key: &str, arguments: &Value) -> Result<Value, ToolError> {
    let index = index_arg(arguments)?;
    let want = opt_bool(arguments, "checked")?
        .ok_or_else(|| ToolError::InvalidArguments("missing required `checked`".into()))?;
    let input = if opt_bool(arguments, "mouse")? == Some(true) {
        Input::Click
    } else {
        Input::ScriptClick
    };
    let snapshot = bound::required_snapshot(server, arguments)?;
    let policy = press_policy(server, arguments)?;
    let mut session = server.take_session(key)?;
    let outcome = (|| {
        let (seen, id) = resolve(server, key, &mut session, snapshot, index)?;
        let focused = session.page().and_then(|p| p.focused().cloned());
        let fresh = fresh(&session)?;
        // The element the snapshot showed must still be there, and the
        // state is the page's now, never the snapshot's.
        if let Some(seen) = &seen {
            bound::same_region(seen, fresh, &id, focused.clone())?;
        }
        let now = present(fresh, &id, index)?.state().checked.ok_or_else(|| {
            ToolError::InvalidArguments(format!("element {index} has no checked state"))
        })?;
        if now == want {
            return Ok(json!({"changed": false, "checked": now, "index": index}));
        }
        let manifold = seen.as_deref().unwrap_or(fresh);
        let ticket = gate(
            manifold,
            &id,
            Action::Click,
            focused,
            manifold.captured_at_ms(),
        )
        .map_err(|reason| ToolError::TicketInvalid(reason.to_string()))?;
        let mut ledger = server.take_ledger(key);
        let executed = execute_ticketed_with(&mut session, &mut ledger, &ticket, &input, &policy);
        server.keep_ledger(key, ledger);
        executed.map_err(exec_error)?;
        let after = session.observe().map_err(browser_err)?;
        let read = after.get(&id).and_then(|r| r.state().checked);
        if read != Some(want) {
            return Err(ToolError::Browser(format!(
                "element {index} did not change: checked reads {} after the press",
                read.map_or("unknown".to_owned(), |b| b.to_string())
            )));
        }
        Ok(json!({"changed": true, "checked": want, "index": index, "region": id.as_str()}))
    })();
    keep(server, key, session);
    outcome
}

/// Move keyboard focus to an element: gate (Focus) → ticket →
/// `consume_ticket_once` → `DOM.focus` on its node.
fn focus(server: &mut dyn BrowserHost, key: &str, arguments: &Value) -> Result<Value, ToolError> {
    let index = index_arg(arguments)?;
    let snapshot = bound::required_snapshot(server, arguments)?;
    let policy = press_policy(server, arguments)?;
    let mut session = server.take_session(key)?;
    let outcome = (|| {
        let (seen, id) = resolve(server, key, &mut session, snapshot, index)?;
        // Owned: the press below borrows the session.
        let fresh = fresh(&session)?.clone();
        let manifold = seen.as_deref().unwrap_or(&fresh);
        present(manifold, &id, index)?;
        let focused = session.page().and_then(|p| p.focused().cloned());
        let url = session.page().and_then(|p| p.url().map(str::to_owned));
        let ticket = gate(
            manifold,
            &id,
            Action::Focus,
            focused.clone(),
            manifold.captured_at_ms(),
        )
        .map_err(|reason| ToolError::TicketInvalid(reason.to_string()))?;
        bound::still_seen(&ticket, &fresh, focused.clone())?;
        if let Some(region) = fresh.get(&id) {
            gate_check(&fresh, region, Action::Focus)
                .map_err(|reason| ToolError::TicketInvalid(reason.to_string()))?;
        }
        let mut ledger = server.take_ledger(key);
        let result = consume_ticket_once_checked(
            &mut ledger,
            &ticket,
            &fresh,
            focused,
            &policy,
            url.as_deref(),
            |target: &RegionId, _action: Action| {
                let node = node_params(&session, target)?;
                session
                    .page_input("DOM.focus", &node.to_string())
                    .map(|_| ())
            },
        );
        server.keep_ledger(key, ledger);
        result.map_err(consume_error)?;
        Ok(json!({"focused": index, "region": id.as_str()}))
    })();
    keep(server, key, session);
    outcome
}

/// Type at whatever has focus, with no target: per-char key events
/// (`type`) or one `Input.insertText` (`insert`). Nothing focused refuses.
fn keyboard(
    server: &mut dyn BrowserHost,
    key: &str,
    arguments: &Value,
) -> Result<Value, ToolError> {
    let text = opt_str(arguments, "text")?
        .ok_or_else(|| ToolError::InvalidArguments("missing required `text`".into()))?;
    let mode = opt_str(arguments, "mode")?.unwrap_or("type");
    if !matches!(mode, "type" | "insert") {
        return Err(ToolError::InvalidArguments(format!(
            "`mode` must be type|insert, got `{mode}`"
        )));
    }
    // The policy first (host, and a confirm for an Enter / Space on the
    // focused control), on an observation taken now when one is set. Insert
    // mode sends no keys: it presses nothing. In type mode an Enter after a
    // Tab, or a Space right after one, would press a control not seen yet.
    let policy = press_policy(server, arguments)?;
    let keys = mode == "type";
    let tab_then_press = text
        .find('\t')
        .is_some_and(|tab| text[tab..].contains('\n') || text.contains("\t "));
    if keys && tab_then_press && !policy.confirm_press.is_empty() {
        return Err(ToolError::InvalidArguments(
            "with confirm patterns set, send an Enter after a Tab (or a Space right after one) in its own call".into(),
        ));
    }
    let now = crate::press::focus_now(server, key, !policy.is_empty())?;
    // Every activating key that lands on the observed focus (before the
    // first Tab; after it the Tab rules above apply), each checked.
    let before_tab = text.split('\t').next().unwrap_or_default();
    for (ch, key) in [(' ', " "), ('\n', "Enter")] {
        if keys && before_tab.contains(ch) {
            crate::press::check_focused(&policy, &now, Some(key))?;
        }
    }
    crate::press::check_focused(&policy, &now, None)?;
    // Fail closed: unless a current observation shows a focused field that
    // is not sensitive, this text is masked in every later output. One taken
    // before a press is stale (the press may have moved the focus): observe
    // again. No observation at all, or a failed one, counts as sensitive.
    let focused_plain = now
        .1
        .as_ref()
        .is_some_and(|r| !crate::secrets::sensitive(r));
    if !focused_plain {
        server.tool_state_mut().secrets.remember(text, true)?;
    } else if let Some((_, after)) = text.split_once('\t') {
        // A Tab moves the focus after it was read (into a password field,
        // say): every piece typed after the first Tab may be a secret.
        for piece in after
            .split(['\t', '\n', '\r'])
            .filter(|p| !p.trim().is_empty())
        {
            server.tool_state_mut().secrets.remember(piece, true)?;
        }
    }
    let mut session = server.take_session(key)?;
    let outcome = (|| {
        let params = json!({"expression": FOCUSED_JS, "returnByValue": true}).to_string();
        let body = session
            .transport_mut()
            .call("Runtime.evaluate", &params)
            .map_err(browser_err)?;
        let focused: Value = serde_json::from_str(&body).map_err(browser_err)?;
        if focused["result"]["value"] != json!(true) {
            return Err(ToolError::InvalidArguments(
                "nothing is focused; focus an element first (browser_focus / browser_click)".into(),
            ));
        }
        if mode == "insert" {
            session
                .page_input("Input.insertText", &json!({ "text": text }).to_string())
                .map_err(browser_err)?;
        } else {
            for ch in text.chars() {
                type_char(&mut session, ch).map_err(browser_err)?;
            }
        }
        Ok(json!({"typed": text.chars().count(), "mode": mode}))
    })();
    keep(server, key, session);
    outcome
}

/// One char as key events: the `browser_send_key` path for keys it knows,
/// else a keyDown carrying the char as its text, and keyUp.
fn type_char(session: &mut LiveSession, ch: char) -> Result<(), BrowserError> {
    let named = match ch {
        '\r' => return Ok(()),
        // Enter carries its text ("\r") to insert a line or submit.
        '\n' => "Enter".to_owned(),
        '\t' => "Tab".to_owned(),
        other => other.to_string(),
    };
    match session.key_event(&named, 0) {
        Err(BrowserError::Navigation(message)) if message.starts_with("unsupported key") => {}
        other => return other,
    }
    let text = ch.to_string();
    let down = json!({"type": "keyDown", "key": text, "text": text, "unmodifiedText": text});
    session.page_input("Input.dispatchKeyEvent", &down.to_string())?;
    let up = json!({"type": "keyUp", "key": text});
    session.page_input("Input.dispatchKeyEvent", &up.to_string())?;
    Ok(())
}

/// `browser_get`: value / attr / box / label / role of an element, from the
/// observation (attr reads `DOM.getAttributes`). Secret values are never read.
fn get(server: &mut dyn BrowserHost, key: &str, arguments: &Value) -> Result<Value, ToolError> {
    let index = index_arg(arguments)?;
    let what = opt_str(arguments, "what")?
        .ok_or_else(|| ToolError::InvalidArguments("missing required `what`".into()))?;
    let name = opt_str(arguments, "name")?;
    match (what, name) {
        ("value" | "box" | "label" | "role", _) | ("attr", Some(_)) => {}
        ("attr", None) => {
            return Err(ToolError::InvalidArguments(
                "`what: attr` needs `name`".into(),
            ))
        }
        (other, _) => {
            return Err(ToolError::InvalidArguments(format!(
                "`what` must be value|attr|box|label|role, got `{other}`"
            )))
        }
    }
    let snapshot = bound::snapshot_arg(arguments)?;
    let mut session = server.take_session(key)?;
    let outcome = (|| {
        // Read from the page now; `snapshot` only fixes which element.
        let (_, id) = resolve(server, key, &mut session, snapshot, index)?;
        let region = present(fresh(&session)?, &id, index)?;
        let state = region.state();
        let secret = crate::secrets::sensitive(region);
        let value = match what {
            "value" if secret => Value::Null,
            "value" => json!(state.value.clone().or_else(|| state.selected.clone())),
            "label" => json!(region.label()),
            "role" => json!(region.role().as_str()),
            "box" => {
                let r = region.rect();
                json!({"x": r.x(), "y": r.y(), "w": r.width(), "h": r.height()})
            }
            _ => {
                let name = name.unwrap_or_default();
                if secret && name.eq_ignore_ascii_case("value") {
                    return Err(ToolError::InvalidArguments(format!(
                        "element {index} is a secret field; its value is never read"
                    )));
                }
                attribute(&mut session, &id, name)?
            }
        };
        let mut body = json!({"index": index, "what": what, "value": value});
        if what == "attr" {
            body["name"] = json!(name);
        }
        Ok(body)
    })();
    keep(server, key, session);
    outcome
}

/// One attribute of the region's node; `null` when it is absent.
fn attribute(session: &mut LiveSession, id: &RegionId, name: &str) -> Result<Value, ToolError> {
    let (node_id, backend) = session.node_ids(id).map_err(browser_err)?;
    let (method, params) = match (node_id, backend) {
        (Some(node_id), _) => ("DOM.getAttributes", json!({ "nodeId": node_id })),
        (None, Some(backend)) => ("DOM.describeNode", json!({ "backendNodeId": backend })),
        (None, None) => return Err(browser_err(BrowserError::TargetUnresolved)),
    };
    let body = session
        .transport_mut()
        .call(method, &params.to_string())
        .map_err(browser_err)?;
    let reply: Value = serde_json::from_str(&body).map_err(browser_err)?;
    let flat = reply
        .get("attributes")
        .or_else(|| reply.get("node").and_then(|node| node.get("attributes")))
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    Ok(flat
        .chunks(2)
        .find(|pair| {
            pair[0]
                .as_str()
                .is_some_and(|n| n.eq_ignore_ascii_case(name))
        })
        .and_then(|pair| pair.get(1).cloned())
        .unwrap_or(Value::Null))
}

/// `browser_is`: visible / enabled / checked / editable, from the fresh
/// observation's flags and control state.
fn is(server: &mut dyn BrowserHost, key: &str, arguments: &Value) -> Result<Value, ToolError> {
    let index = index_arg(arguments)?;
    let what = opt_str(arguments, "what")?
        .ok_or_else(|| ToolError::InvalidArguments("missing required `what`".into()))?;
    if !matches!(what, "visible" | "enabled" | "checked" | "editable") {
        return Err(ToolError::InvalidArguments(format!(
            "`what` must be visible|enabled|checked|editable, got `{what}`"
        )));
    }
    let snapshot = bound::snapshot_arg(arguments)?;
    let mut session = server.take_session(key)?;
    let outcome = (|| {
        let (_, id) = resolve(server, key, &mut session, snapshot, index)?;
        let Some(region) = fresh(&session)?.get(&id) else {
            // A control that left the page is not visible; the rest is unknown.
            if what == "visible" {
                return Ok(json!({"index": index, "what": what, "value": false, "gone": true}));
            }
            return Err(gone(index, &id));
        };
        let flags = region.flags();
        let value = match what {
            "visible" => !flags.hidden() && !region.rect().is_zero_area(),
            "enabled" => !flags.disabled(),
            "checked" => region.state().checked.unwrap_or(false),
            _ => region.actions().contains(&Action::Type) && !flags.disabled() && !flags.readonly(),
        };
        Ok(json!({"index": index, "what": what, "value": value}))
    })();
    keep(server, key, session);
    outcome
}

/// Interactive-index entries whose label (exact, case-insensitive) and/or
/// role match.
fn count(server: &mut dyn BrowserHost, key: &str, arguments: &Value) -> Result<Value, ToolError> {
    let label = opt_str(arguments, "label")?.map(|l| l.trim().to_lowercase());
    let role = match opt_str(arguments, "role")? {
        None => None,
        Some(raw) => Some(
            Role::parse(&raw.trim().to_lowercase())
                .ok_or_else(|| ToolError::InvalidArguments(format!("unknown role `{raw}`")))?,
        ),
    };
    let mut session = server.take_session(key)?;
    let outcome = (|| {
        let manifold = session.observe().map_err(browser_err)?;
        let indexes: Vec<usize> = interactive_index(manifold)
            .iter()
            .enumerate()
            .filter_map(|(i, id)| manifold.get(id).map(|region| (i + 1, region)))
            .filter(|(_, region)| {
                label
                    .as_ref()
                    .is_none_or(|l| region.label().trim().to_lowercase() == *l)
                    && role.is_none_or(|r| region.role() == r)
            })
            .map(|(i, _)| i)
            .collect();
        Ok(json!({"count": indexes.len(), "indexes": indexes}))
    })();
    keep(server, key, session);
    outcome
}

/// Screenshot with each interactive region's box and `@eN` label drawn on a
/// temporary overlay. The overlay is removed after the capture, whatever the
/// capture did.
fn annotated_screenshot(
    server: &mut dyn BrowserHost,
    key: &str,
    arguments: &Value,
) -> Result<Value, ToolError> {
    let mut session = server.take_session(key)?;
    let prepared = (|| {
        let manifold = session.observe().map_err(browser_err)?.clone();
        let boxes: Vec<Value> = interactive_index(&manifold)
            .iter()
            .enumerate()
            .filter_map(|(i, id)| {
                let r = manifold.get(id)?.rect();
                Some(json!([
                    r.x(),
                    r.y(),
                    r.width(),
                    r.height(),
                    format!("@e{}", i + 1)
                ]))
            })
            .collect();
        let body = session
            .transport_mut()
            .call(
                "Runtime.evaluate",
                &json!({"expression": "document", "returnByValue": false}).to_string(),
            )
            .map_err(browser_err)?;
        let reply: Value = serde_json::from_str(&body).map_err(browser_err)?;
        let document = reply["result"]["objectId"]
            .as_str()
            .map(str::to_owned)
            .ok_or_else(|| browser_err(BrowserError::MissingObjectId))?;
        Ok::<_, ToolError>((document, Value::Array(boxes)))
    })();
    let (document, boxes) = match prepared {
        Ok(prepared) => prepared,
        Err(err) => {
            keep(server, key, session);
            return Err(err);
        }
    };
    let drawn = overlay(&mut session, &document, boxes);
    keep(server, key, session);
    let shot =
        drawn.and_then(|()| crate::browser_tools::call(server, "browser_screenshot", arguments));
    let mut session = server.take_session(key)?;
    let removed = overlay(&mut session, &document, Value::Null);
    keep(server, key, session);
    let body = shot?;
    removed?;
    Ok(body)
}

/// Run [`OVERLAY_FUNCTION`] on `document` with `boxes` (`null` removes).
fn overlay(session: &mut LiveSession, document: &str, boxes: Value) -> Result<(), ToolError> {
    let params = json!({
        "functionDeclaration": OVERLAY_FUNCTION,
        "objectId": document,
        "arguments": [{ "value": boxes }],
        "returnByValue": true,
    });
    session
        .transport_mut()
        .call("Runtime.callFunctionOn", &params.to_string())
        .map(|_| ())
        .map_err(browser_err)
}

/// Observe, and the region id at `index`. With `snapshot` the index reads
/// that snapshot (`bound::resolve`) and the snapshot's manifold is returned:
/// a press mints its ticket there and revalidates on the session's fresh
/// observation ([`fresh`]). Without (`None`), the index reads the last
/// observation when no input ran since (what the host last saw), else this
/// fresh one, which is also the one the press mints on.
fn resolve(
    server: &dyn BrowserHost,
    key: &str,
    session: &mut LiveSession,
    snapshot: Option<u64>,
    index: usize,
) -> Result<(Option<Rc<InteractionManifold>>, RegionId), ToolError> {
    let prior = session.fresh_manifold().map(interactive_index);
    let manifold = session.observe().map_err(browser_err)?;
    if snapshot.is_some() {
        return bound::resolve(server, key, snapshot, manifold, index)
            .map(|(seen, id)| (seen.snapshot(), id));
    }
    let order = prior.unwrap_or_else(|| interactive_index(manifold));
    Ok((None, at_index(&order, index)?))
}

/// The session's observation (taken by [`resolve`]), borrowed.
fn fresh(session: &LiveSession) -> Result<&InteractionManifold, ToolError> {
    session
        .manifold()
        .ok_or_else(|| browser_err(BrowserError::NotObserved))
}

fn present<'a>(
    manifold: &'a InteractionManifold,
    id: &RegionId,
    index: usize,
) -> Result<&'a InteractionRegion, ToolError> {
    manifold.get(id).ok_or_else(|| gone(index, id))
}

fn gone(index: usize, id: &RegionId) -> ToolError {
    ToolError::Browser(format!(
        "element {index} ({}) is no longer on the page; call browser_get_state",
        id.as_str()
    ))
}

fn node_params(session: &LiveSession, id: &RegionId) -> Result<Value, BrowserError> {
    match session.node_ids(id)? {
        (Some(node_id), _) => Ok(json!({ "nodeId": node_id })),
        (None, Some(backend)) => Ok(json!({ "backendNodeId": backend })),
        (None, None) => Err(BrowserError::TargetUnresolved),
    }
}

/// `index` as `5`, `"5"` or `"@e5"`, 1-based.
pub(crate) fn index_arg(arguments: &Value) -> Result<usize, ToolError> {
    let bad = |got: &Value| {
        ToolError::InvalidArguments(format!(
            "`index` must be a 1-based index (5, \"5\" or \"@e5\"), got {got}"
        ))
    };
    let index = match arguments.get("index") {
        None | Some(Value::Null) => {
            return Err(ToolError::InvalidArguments(
                "missing required `index`".into(),
            ))
        }
        Some(Value::Number(n)) => n.as_u64().and_then(|v| usize::try_from(v).ok()),
        Some(Value::String(s)) => {
            let s = s.trim();
            s.strip_prefix("@e").unwrap_or(s).parse::<usize>().ok()
        }
        Some(other) => return Err(bad(other)),
    };
    match index {
        Some(index) if index > 0 => Ok(index),
        _ => Err(bad(&arguments["index"])),
    }
}

/// Tool specs: this module's tools, and `max_chars` / `annotate` on the
/// `browser_tools` ones they extend.
pub(crate) fn spec(name: &str) -> (String, Map<String, Value>, Vec<&'static str>) {
    let index = || {
        json!({"type": ["integer", "string"],
               "description": "1-based browser_get_state index: 5, \"5\" or \"@e5\""})
    };
    let mut props = Map::new();
    props.insert(
        "cdp".into(),
        json!({"type": "string", "description": format!("CDP debugging endpoint; default {DEFAULT_CDP_HTTP}")}),
    );
    let mut required = Vec::new();
    let description = match name {
        "browser_dialog" => {
            props.insert(
                "action".into(),
                json!({"type": "string", "enum": ["status", "accept", "dismiss"]}),
            );
            props.insert(
                "text".into(),
                json!({"type": "string", "description": "Prompt text sent with accept."}),
            );
            "Status of the open JavaScript alert / confirm / prompt / beforeunload dialog ({open, type, message, url}), or accept / dismiss it (Page.handleJavaScriptDialog). While a dialog is open the page answers nothing else."
        }
        "browser_go_forward" => "Go forward one history entry.",
        "browser_reload" => {
            props.insert("ignore_cache".into(), json!({"type": "boolean"}));
            "Reload the current tab."
        }
        "browser_check" => {
            props.insert("index".into(), index());
            props.insert("snapshot".into(), crate::bound::snapshot_prop());
            props.insert("checked".into(), json!({"type": "boolean"}));
            props.insert("mouse".into(), json!({"type": "boolean"}));
            required.extend(["index", "checked"]);
            "Set a checkbox / radio / switch to `checked`. Already so on the page now: {changed: false}, no press (with snapshot, only after the element still matches it; else stale_ref). Else the browser_click press through the ticketed executor, then a read-back that must show the new state."
        }
        "browser_focus" => {
            props.insert("index".into(), index());
            props.insert("snapshot".into(), crate::bound::snapshot_prop());
            required.push("index");
            "Move keyboard focus to the element at an index (gate + one-shot ticket, then DOM.focus)."
        }
        "browser_keyboard" => {
            props.insert("text".into(), json!({"type": "string"}));
            props.insert(
                "mode".into(),
                json!({"type": "string", "enum": ["type", "insert"]}),
            );
            props.insert("confirm".into(), crate::browser_tools::confirm_prop());
            required.push("text");
            "Type text at the focused element, no target: mode type (default) sends key events per char, insert sends one Input.insertText. Refuses when nothing is focused, or on a page off the domain allow-list (and a form it would submit off the list; a link it follows is checked on the next call); an Enter / Space on a focused control whose label needs a confirm needs `confirm`."
        }
        "browser_get" => {
            props.insert("index".into(), index());
            props.insert("snapshot".into(), crate::bound::snapshot_prop());
            props.insert(
                "what".into(),
                json!({"type": "string", "enum": ["value", "attr", "box", "label", "role"]}),
            );
            props.insert(
                "name".into(),
                json!({"type": "string", "description": "Attribute name for what: attr."}),
            );
            required.extend(["index", "what"]);
            "One fact about the element at an index, from the observation: value (never a password's), attr (DOM.getAttributes), box, label, role."
        }
        "browser_is" => {
            props.insert("index".into(), index());
            props.insert("snapshot".into(), crate::bound::snapshot_prop());
            props.insert(
                "what".into(),
                json!({"type": "string", "enum": ["visible", "enabled", "checked", "editable"]}),
            );
            required.extend(["index", "what"]);
            "Whether the element at an index is visible / enabled / checked / editable, from a fresh observation."
        }
        "browser_count" => {
            props.insert("label".into(), json!({"type": "string"}));
            props.insert("role".into(), json!({"type": "string"}));
            "Number (and indexes) of interactive elements whose label (exact, case-insensitive) and/or role match."
        }
        other => {
            let (description, mut props, required) = crate::browser_tools::spec(other);
            if OUTPUT_TOOLS.contains(&other) {
                props.insert(
                    "max_chars".into(),
                    json!({"type": "integer", "minimum": 0,
                           "description": "Cut the text output to this many chars, appending … [truncated N chars]."}),
                );
            }
            if crate::bound::SNAPSHOT_TOOLS.contains(&other) {
                props.insert("snapshot".into(), crate::bound::snapshot_prop());
            }
            if other == "browser_screenshot" {
                props.insert(
                    "annotate".into(),
                    json!({"type": "boolean",
                           "description": "Draw each interactive element's box and @eN label on a temporary overlay, removed after the capture."}),
                );
            }
            return (description, props, required);
        }
    };
    (description.to_owned(), props, required)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn truncation_cuts_at_a_char_boundary_and_counts_chars() {
        // Two-byte, three-byte and four-byte chars: never cut inside one.
        let text = "aé漢🙂b";
        assert_eq!(truncate_output(text, 10), Cow::Borrowed(text));
        assert_eq!(truncate_output(text, 5), Cow::Borrowed(text));
        assert_eq!(truncate_output(text, 2), "aé… [truncated 3 chars]");
        assert_eq!(truncate_output(text, 3), "aé漢… [truncated 2 chars]");
        assert_eq!(truncate_output(text, 0), "… [truncated 5 chars]");
    }

    #[test]
    fn index_takes_integer_string_and_ref() {
        for raw in [json!(5), json!("5"), json!("@e5"), json!(" @e5 ")] {
            assert_eq!(index_arg(&json!({ "index": raw })).unwrap(), 5, "{raw}");
        }
        for raw in [json!(0), json!("@e0"), json!("e5x"), json!(-1), json!(true)] {
            assert!(index_arg(&json!({ "index": raw })).is_err(), "{raw}");
        }
        assert!(index_arg(&json!({})).is_err());
    }

    #[test]
    fn list_outputs_keep_whole_entries() {
        let body = json!({"entries": [{"t": "aaaa"}, {"t": "bbbb"}, {"t": "cccc"}]});
        let cut = limit_output("browser_read_console", &json!({"max_chars": 30}), body).unwrap();
        assert_eq!(cut["entries"].as_array().unwrap().len(), 2, "{cut}");
        assert_eq!(cut["truncated"], "… [truncated 13 chars]");
    }
}
