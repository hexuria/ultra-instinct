//! agent-browser ports over the replay harness: dialogs, forward / reload,
//! check, focus, keyboard, get / is / count, annotated screenshots, and the
//! `max_chars` output limit. Each test pins the CDP calls a tool emits.

use std::cell::RefCell;
use std::collections::VecDeque;
use std::rc::Rc;

use serde_json::{json, Value};
use ultra_instinct_browser::script::{Control, PageSpec, ScriptBuilder};
use ultra_instinct_browser::{CdpError, CdpTransport, ReplayTransport};
use ultra_instinct_cdp::CdpEvent;
use ultra_instinct_mcp::{Server, ToolError};

const CDP: &str = "http://127.0.0.1:9222";

type Log = Rc<RefCell<Vec<(String, Value)>>>;

/// A replay peer that also emits CDP events: each trigger fires once,
/// after the first call of its method, the way Chrome sends
/// `Page.javascriptDialogOpening` while the call that opened the dialog runs.
struct Evented {
    inner: ReplayTransport,
    log: Log,
    triggers: VecDeque<(&'static str, CdpEvent)>,
    pending: Vec<CdpEvent>,
    /// A trigger on this method also ends the call with
    /// `CdpError::DialogOpened`, as the websocket does when the event
    /// arrives before the reply.
    interrupt: Option<&'static str>,
}

impl CdpTransport for Evented {
    fn call(&mut self, method: &str, params_json: &str) -> Result<String, CdpError> {
        self.log.borrow_mut().push((
            method.to_owned(),
            serde_json::from_str(params_json).unwrap_or(Value::Null),
        ));
        let result = self.inner.call(method, params_json);
        if self.triggers.front().is_some_and(|(m, _)| *m == method) {
            let (_, event) = self.triggers.pop_front().expect("front checked");
            self.pending.push(event);
            if self.interrupt == Some(method) {
                self.interrupt = None;
                return Err(CdpError::DialogOpened {
                    method: method.to_owned(),
                });
            }
        }
        result
    }

    fn drain_events(&mut self) -> Vec<CdpEvent> {
        std::mem::take(&mut self.pending)
    }
}

fn event(method: &str, params: Value) -> CdpEvent {
    CdpEvent {
        method: method.to_owned(),
        params,
    }
}

fn server_with(script: ScriptBuilder, triggers: Vec<(&'static str, CdpEvent)>) -> (Server, Log) {
    server_interrupted(script, triggers, None)
}

fn server_interrupted(
    script: ScriptBuilder,
    triggers: Vec<(&'static str, CdpEvent)>,
    interrupt: Option<&'static str>,
) -> (Server, Log) {
    let log: Log = Rc::new(RefCell::new(Vec::new()));
    let shared = Rc::clone(&log);
    let mut script = Some((script, triggers));
    let server = Server::with_connector(move |_url| {
        let (script, triggers) = script.take().ok_or_else(|| CdpError::Transport {
            message: "one connect per test".into(),
        })?;
        Ok(Box::new(Evented {
            inner: ReplayTransport::parse(&script.to_json())?,
            log: Rc::clone(&shared),
            triggers: triggers.into(),
            pending: Vec::new(),
            interrupt,
        }) as Box<dyn CdpTransport>)
    });
    (server, log)
}

fn server(script: ScriptBuilder) -> (Server, Log) {
    server_with(script, Vec::new())
}

fn call(server: &mut Server, tool: &str, arguments: Value) -> Value {
    server
        .call_tool(tool, &arguments)
        .unwrap_or_else(|err| panic!("{tool} {arguments}: {err:?}"))
}

fn calls(log: &Log, method: &str) -> Vec<Value> {
    log.borrow()
        .iter()
        .filter(|(m, _)| m == method)
        .map(|(_, params)| params.clone())
        .collect()
}

fn presses(log: &Log) -> usize {
    calls(log, "Runtime.callFunctionOn").len() + calls(log, "Input.dispatchMouseEvent").len()
}

/// Email y=20 → 1, Sign in y=80 → 2, Remember me y=140 → 3.
fn page(remember: bool, email_disabled: bool) -> PageSpec {
    let checkbox = Control {
        tag: "INPUT",
        role: "checkbox",
        ..Control::button(12, 102, "Remember me", (20.0, 140.0, 20.0, 20.0))
    };
    let mut page = PageSpec::of(
        &[
            Control::text_field(10, 100, "Email", (20.0, 20.0, 300.0, 28.0)),
            Control::button(11, 101, "Sign in", (20.0, 80.0, 120.0, 28.0)),
            checkbox,
        ],
        "http://127.0.0.1/form",
        "Form",
    );
    page.dom[0] = page.dom[0]
        .clone()
        .with_compact_state(json!({"v": "ana@example.test", "it": "email"}));
    if email_disabled {
        page.dom[0] = page.dom[0].clone().with_attr("disabled", "");
    }
    page.dom[2] = page.dom[2]
        .clone()
        .with_compact_state(json!({"c": remember, "it": "checkbox"}));
    page
}

fn login(remember: bool) -> PageSpec {
    page(remember, false)
}

#[test]
fn dialog_status_accept_dismiss_and_the_blocked_tool_hint() {
    let opening = |kind: &str, message: &str| {
        event(
            "Page.javascriptDialogOpening",
            json!({"url": "http://127.0.0.1/form", "message": message, "type": kind,
                   "hasBrowserHandler": false, "defaultPrompt": "you"}),
        )
    };
    let closed = || event("Page.javascriptDialogClosed", json!({"result": true}));
    let (mut server, log) = server_with(
        ScriptBuilder::new()
            .call("Page.enable", json!({}))
            // The page is blocked behind the dialog: the read times out.
            .call_error(
                "Runtime.evaluate",
                "no CDP result for `Runtime.evaluate` within 30 s",
            )
            .call("Page.handleJavaScriptDialog", json!({}))
            .call(
                "Runtime.evaluate",
                json!({"result": {"type": "string", "value": "ok"}}),
            )
            .call("Page.handleJavaScriptDialog", json!({})),
        vec![
            ("Runtime.evaluate", opening("prompt", "Your name?")),
            ("Page.handleJavaScriptDialog", closed()),
            ("Runtime.evaluate", opening("confirm", "Leave?")),
        ],
    );

    let err = server
        .call_tool("browser_get_text", &json!({"cdp": CDP}))
        .expect_err("blocked behind a dialog");
    // Typed (`dialog_open`, with the dialog's type and message); the
    // failure text with the hint is `detail`.
    assert!(
        matches!(&err, ToolError::DialogOpen { kind, message, detail }
            if kind == "prompt" && message == "Your name?"
                && detail.ends_with("a prompt dialog is open; call browser_dialog")),
        "{err:?}"
    );
    assert_eq!(err.to_value()["code"], "dialog_open");
    assert_eq!(err.to_value()["variant"], "Browser");

    let status = call(&mut server, "browser_dialog", json!({"cdp": CDP}));
    assert_eq!(status["open"], true, "{status}");
    assert_eq!(status["type"], "prompt");
    assert_eq!(status["message"], "Your name?");
    assert_eq!(status["url"], "http://127.0.0.1/form");
    assert_eq!(status["tracked"], true);

    let accepted = call(
        &mut server,
        "browser_dialog",
        json!({"cdp": CDP, "action": "accept", "text": "Ana \"quoted\""}),
    );
    assert_eq!(accepted["handled"], "accept", "{accepted}");
    assert_eq!(accepted["type"], "prompt");
    assert_eq!(
        calls(&log, "Page.handleJavaScriptDialog")[0],
        json!({"accept": true, "promptText": "Ana \"quoted\""}),
        "the prompt text is a CDP argument"
    );
    let status = call(
        &mut server,
        "browser_dialog",
        json!({"cdp": CDP, "action": "status"}),
    );
    assert_eq!(status["open"], false, "{status}");

    // A tool that works while no dialog is open carries no hint; the confirm
    // it opens is reported on the next status.
    let text = call(&mut server, "browser_get_text", json!({"cdp": CDP}));
    assert_eq!(text["text"], "ok");
    let dismissed = call(
        &mut server,
        "browser_dialog",
        json!({"cdp": CDP, "action": "dismiss"}),
    );
    assert_eq!(dismissed["handled"], "dismiss", "{dismissed}");
    assert_eq!(dismissed["type"], "confirm");
    assert_eq!(
        calls(&log, "Page.handleJavaScriptDialog")[1],
        json!({"accept": false})
    );
    // Page is enabled once per tab.
    assert_eq!(calls(&log, "Page.enable").len(), 1);
}

/// A click whose handler calls `alert()`: Chrome sends the opening event
/// and no reply, so the transport stops waiting with `DialogOpened`. The
/// tool fails at once with the dialog hint, the press is not retried (no
/// pointer or second DOM tier), and the dialog is tracked for
/// `browser_dialog`.
#[test]
fn a_press_that_opens_an_alert_fails_fast_with_the_hint_and_is_not_retried() {
    let (mut server, log) = server_interrupted(
        ScriptBuilder::new()
            .call("Page.enable", json!({}))
            .observe_compact(&login(false))
            .observe_compact(&login(false))
            .dom_click(11)
            .call("Page.handleJavaScriptDialog", json!({})),
        vec![
            (
                "Runtime.callFunctionOn",
                event(
                    "Page.javascriptDialogOpening",
                    json!({"url": "http://127.0.0.1/form", "message": "Saved", "type": "alert",
                           "hasBrowserHandler": false, "defaultPrompt": ""}),
                ),
            ),
            (
                "Page.handleJavaScriptDialog",
                event("Page.javascriptDialogClosed", json!({"result": true})),
            ),
        ],
        Some("Runtime.callFunctionOn"),
    );
    let err = server
        .call_tool("browser_click", &json!({"cdp": CDP, "index": "@e2"}))
        .expect_err("the alert interrupts the press");
    let ToolError::DialogOpen {
        kind,
        detail: message,
        ..
    } = &err
    else {
        panic!("{err:?}");
    };
    assert_eq!(kind, "alert");
    assert!(
        message.contains("dialog opened while `Runtime.callFunctionOn` ran")
            && message.ends_with("a alert dialog is open; call browser_dialog"),
        "{message}"
    );
    assert_eq!(presses(&log), 1, "dispatched once, never retried");
    let status = call(&mut server, "browser_dialog", json!({"cdp": CDP}));
    assert_eq!(status["open"], true, "{status}");
    assert_eq!(status["message"], "Saved");
    let accepted = call(
        &mut server,
        "browser_dialog",
        json!({"cdp": CDP, "action": "accept"}),
    );
    assert_eq!(accepted["handled"], "accept", "{accepted}");
    assert_eq!(presses(&log), 1);
}

#[test]
fn dialog_rejects_an_unknown_action() {
    let (mut server, _log) = server(ScriptBuilder::new());
    let err = server
        .call_tool("browser_dialog", &json!({"cdp": CDP, "action": "close"}))
        .unwrap_err();
    assert!(matches!(err, ToolError::InvalidArguments(_)), "{err:?}");
}

#[test]
fn go_forward_and_reload_are_page_level_calls() {
    let (mut server, log) = server(
        ScriptBuilder::new()
            .call(
                "Page.getNavigationHistory",
                json!({"currentIndex": 0, "entries": [
                    {"id": 101, "url": "http://127.0.0.1/a", "title": "A"},
                    {"id": 102, "url": "http://127.0.0.1/b", "title": "B"},
                ]}),
            )
            .call("Page.navigateToHistoryEntry", json!({}))
            .call("Page.reload", json!({}))
            .call(
                "Page.getNavigationHistory",
                json!({"currentIndex": 1, "entries": [{"id": 101}, {"id": 102}]}),
            ),
    );
    let out = call(&mut server, "browser_go_forward", json!({"cdp": CDP}));
    assert_eq!(out["went_forward"], true, "{out}");
    assert_eq!(
        calls(&log, "Page.navigateToHistoryEntry"),
        [json!({"entryId": 102})]
    );
    let out = call(
        &mut server,
        "browser_reload",
        json!({"cdp": CDP, "ignore_cache": true}),
    );
    assert_eq!(out["reloaded"], true, "{out}");
    assert_eq!(calls(&log, "Page.reload"), [json!({"ignoreCache": true})]);
    let err = server
        .call_tool("browser_go_forward", &json!({"cdp": CDP}))
        .expect_err("already at the newest entry");
    assert!(matches!(err, ToolError::Browser(_)), "{err:?}");
}

#[test]
fn check_that_already_holds_does_not_press() {
    let (mut server, log) = server(ScriptBuilder::new().observe_compact(&login(true)));
    let out = call(
        &mut server,
        "browser_check",
        json!({"cdp": CDP, "index": "@e3", "checked": true}),
    );
    assert_eq!(out, json!({"changed": false, "checked": true, "index": 3}));
    assert_eq!(presses(&log), 0);
}

#[test]
fn check_presses_once_through_the_executor_and_reads_back() {
    // observe → executor's fresh observe → DOM click → read-back observe;
    // then a second check sees it already set and does not press again.
    let (mut server, log) = server(
        ScriptBuilder::new()
            .observe_compact(&login(false))
            .observe_compact(&login(false))
            .dom_click(12)
            .observe_compact(&login(true))
            .observe_compact(&login(true)),
    );
    let out = call(
        &mut server,
        "browser_check",
        json!({"cdp": CDP, "index": 3, "checked": true}),
    );
    assert_eq!(out["changed"], true, "{out}");
    assert_eq!(out["checked"], true);
    assert_eq!(calls(&log, "DOM.resolveNode"), [json!({"nodeId": 12})]);
    assert_eq!(calls(&log, "Runtime.callFunctionOn").len(), 1);

    let again = call(
        &mut server,
        "browser_check",
        json!({"cdp": CDP, "index": "3", "checked": true}),
    );
    assert_eq!(again["changed"], false, "{again}");
    assert_eq!(presses(&log), 1, "the ticket was consumed once");
}

/// The snapshot showed it checked, then the page unchecked it: the state
/// is read from the page now, so the check presses and reads back checked,
/// never `changed: false` on the snapshot's old state.
#[test]
fn check_with_a_snapshot_reads_the_state_from_the_page_now() {
    let (mut server, log) = server(
        ScriptBuilder::new()
            .observe_compact(&login(true))
            .observe_compact(&login(false))
            .observe_compact(&login(false))
            .dom_click(12)
            .observe_compact(&login(true)),
    );
    let state = call(&mut server, "browser_get_state", json!({"cdp": CDP}));
    let out = call(
        &mut server,
        "browser_check",
        json!({"cdp": CDP, "index": 3, "checked": true, "snapshot": state["snapshot"]}),
    );
    assert_eq!(out["changed"], true, "{out}");
    assert_eq!(out["checked"], true, "{out}");
    assert_eq!(calls(&log, "DOM.resolveNode"), [json!({"nodeId": 12})]);
}

/// Already in the requested state on the page, but the element the
/// snapshot showed moved: `stale_ref`, not `changed: false`.
#[test]
fn check_with_a_snapshot_refuses_a_moved_element_that_already_holds() {
    // The checkbox moved down 60 px (now checked).
    let checkbox = Control {
        tag: "INPUT",
        role: "checkbox",
        ..Control::button(12, 102, "Remember me", (20.0, 200.0, 20.0, 20.0))
    };
    let mut moved = PageSpec::of(
        &[
            Control::text_field(10, 100, "Email", (20.0, 20.0, 300.0, 28.0)),
            Control::button(11, 101, "Sign in", (20.0, 80.0, 120.0, 28.0)),
            checkbox,
        ],
        "http://127.0.0.1/form",
        "Form",
    );
    moved.dom[2] = moved.dom[2]
        .clone()
        .with_compact_state(json!({"c": true, "it": "checkbox"}));
    let (mut server, log) = server(
        ScriptBuilder::new()
            .observe_compact(&login(false))
            .observe_compact(&moved),
    );
    let state = call(&mut server, "browser_get_state", json!({"cdp": CDP}));
    let err = server
        .call_tool(
            "browser_check",
            &json!({"cdp": CDP, "index": 3, "checked": true, "snapshot": state["snapshot"]}),
        )
        .unwrap_err();
    assert_eq!(err.code(), "stale_ref", "{err:?}");
    assert_eq!(presses(&log), 0);
}

#[test]
fn check_errors_when_the_state_does_not_change() {
    let (mut server, _log) = server(
        ScriptBuilder::new()
            .observe_compact(&login(false))
            .observe_compact(&login(false))
            .dom_click(12)
            .observe_compact(&login(false)),
    );
    let err = server
        .call_tool(
            "browser_check",
            &json!({"cdp": CDP, "index": 3, "checked": true}),
        )
        .unwrap_err();
    assert!(
        matches!(&err, ToolError::Browser(m) if m.contains("did not change")),
        "{err:?}"
    );
}

#[test]
fn check_refuses_a_control_without_checked_state() {
    let (mut server, log) = server(ScriptBuilder::new().observe_compact(&login(false)));
    let err = server
        .call_tool(
            "browser_check",
            &json!({"cdp": CDP, "index": 2, "checked": true}),
        )
        .unwrap_err();
    assert!(matches!(err, ToolError::InvalidArguments(_)), "{err:?}");
    assert_eq!(presses(&log), 0);
}

#[test]
fn focus_tickets_then_focuses_the_node() {
    let (mut server, log) = server(
        ScriptBuilder::new()
            .observe_compact(&login(false))
            .call("DOM.focus", json!({})),
    );
    let out = call(
        &mut server,
        "browser_focus",
        json!({"cdp": CDP, "index": 1}),
    );
    assert_eq!(out["focused"], 1, "{out}");
    assert_eq!(calls(&log, "DOM.focus"), [json!({"nodeId": 10})]);
}

#[test]
fn focus_refuses_a_control_that_turned_disabled() {
    // The host saw Email enabled at index 1; by the focus call it is disabled.
    let (mut server, log) = server(
        ScriptBuilder::new()
            .observe_compact(&login(false))
            .observe_compact(&page(false, true)),
    );
    call(&mut server, "browser_get_state", json!({"cdp": CDP}));
    let err = server
        .call_tool("browser_focus", &json!({"cdp": CDP, "index": 1}))
        .unwrap_err();
    assert!(
        matches!(&err, ToolError::TicketInvalid(m) if m == "disabled"),
        "{err:?}"
    );
    assert!(calls(&log, "DOM.focus").is_empty());
}

#[test]
fn keyboard_refuses_when_nothing_is_focused() {
    let (mut server, log) = server(ScriptBuilder::new().call(
        "Runtime.evaluate",
        json!({"result": {"type": "boolean", "value": false}}),
    ));
    let err = server
        .call_tool("browser_keyboard", &json!({"cdp": CDP, "text": "hi"}))
        .unwrap_err();
    assert!(matches!(err, ToolError::InvalidArguments(_)), "{err:?}");
    assert!(calls(&log, "Input.dispatchKeyEvent").is_empty());
    assert!(calls(&log, "Input.insertText").is_empty());
}

#[test]
fn keyboard_types_keys_or_inserts_text_at_the_focus() {
    let focused = || json!({"result": {"type": "boolean", "value": true}});
    let mut script = ScriptBuilder::new().call("Runtime.evaluate", focused());
    // "a" and "é": the send_key path knows "a"; "é" goes as its own text.
    for _ in 0..4 {
        script = script.call("Input.dispatchKeyEvent", json!({}));
    }
    // The keys ran after the last observation: the next call reads the
    // focus again (which field it is decides whether the text is secret).
    let script = script
        .observe_compact(&login(true))
        .call("Runtime.evaluate", focused())
        .call("Input.insertText", json!({}));
    let (mut server, log) = server(script);
    let out = call(
        &mut server,
        "browser_keyboard",
        json!({"cdp": CDP, "text": "aé"}),
    );
    assert_eq!(out, json!({"typed": 2, "mode": "type"}));
    let keys = calls(&log, "Input.dispatchKeyEvent");
    assert_eq!(keys[0]["text"], "a");
    assert_eq!(keys[2]["text"], "é");
    assert_eq!(keys[3]["type"], "keyUp");

    let out = call(
        &mut server,
        "browser_keyboard",
        json!({"cdp": CDP, "text": "'); alert(1); //", "mode": "insert"}),
    );
    assert_eq!(out["mode"], "insert", "{out}");
    assert_eq!(
        calls(&log, "Input.insertText"),
        [json!({"text": "'); alert(1); //"})]
    );
}

#[test]
fn get_is_and_count_answer_from_the_observation() {
    // Five gets (the attr read follows the fifth observe), four is, two counts.
    let mut script = ScriptBuilder::new();
    for _ in 0..5 {
        script = script.observe_compact(&login(true));
    }
    script = script.call(
        "DOM.getAttributes",
        json!({"attributes": ["type", "email", "name", "em"]}),
    );
    for _ in 0..6 {
        script = script.observe_compact(&login(true));
    }
    let (mut server, _log) = server(script);
    let mut get = |what: &str, extra: Value| {
        let mut args = json!({"cdp": CDP, "index": 1, "what": what});
        if let (Some(args), Some(extra)) = (args.as_object_mut(), extra.as_object()) {
            args.extend(extra.clone());
        }
        call(&mut server, "browser_get", args)["value"].clone()
    };
    assert_eq!(get("value", json!({})), "ana@example.test");
    assert_eq!(get("label", json!({})), "Email");
    assert_eq!(get("role", json!({})), "text_field");
    assert_eq!(
        get("box", json!({})),
        json!({"x": 20.0, "y": 20.0, "w": 300.0, "h": 28.0})
    );
    assert_eq!(get("attr", json!({"name": "NAME"})), "em");

    let mut is = |index: usize, what: &str| {
        call(
            &mut server,
            "browser_is",
            json!({"cdp": CDP, "index": index, "what": what}),
        )["value"]
            .clone()
    };
    assert_eq!(is(1, "visible"), true);
    assert_eq!(is(1, "enabled"), true);
    assert_eq!(is(3, "checked"), true);
    assert_eq!(is(2, "editable"), false);

    let out = call(
        &mut server,
        "browser_count",
        json!({"cdp": CDP, "label": "SIGN IN"}),
    );
    assert_eq!(out, json!({"count": 1, "indexes": [2]}));
    let out = call(
        &mut server,
        "browser_count",
        json!({"cdp": CDP, "label": "sign", "role": "button"}),
    );
    assert_eq!(out["count"], 0, "label match is exact: {out}");
}

#[test]
fn get_never_returns_a_password_value() {
    let mut page = login(false);
    page.dom[0] = page.dom[0]
        .clone()
        .with_compact_state(json!({"it": "password"}));
    let (mut server, log) = server(
        ScriptBuilder::new()
            .observe_compact(&page)
            .observe_compact(&page),
    );
    let out = call(
        &mut server,
        "browser_get",
        json!({"cdp": CDP, "index": 1, "what": "value"}),
    );
    assert_eq!(out["value"], Value::Null, "{out}");
    let err = server
        .call_tool(
            "browser_get",
            &json!({"cdp": CDP, "index": 1, "what": "attr", "name": "value"}),
        )
        .unwrap_err();
    assert!(matches!(err, ToolError::InvalidArguments(_)), "{err:?}");
    assert!(calls(&log, "DOM.getAttributes").is_empty());
    assert!(calls(&log, "Runtime.callFunctionOn").is_empty());
}

#[test]
fn is_reports_a_control_that_turned_disabled() {
    let (mut server, _log) = server(
        ScriptBuilder::new()
            .observe_compact(&login(false))
            .observe_compact(&page(false, true)),
    );
    call(&mut server, "browser_get_state", json!({"cdp": CDP}));
    let out = call(
        &mut server,
        "browser_is",
        json!({"cdp": CDP, "index": 1, "what": "enabled"}),
    );
    assert_eq!(out["value"], false, "{out}");
}

#[test]
fn annotated_screenshot_removes_the_overlay_when_the_capture_fails() {
    let (mut server, log) = server(
        ScriptBuilder::new()
            .observe_compact(&login(false))
            .call(
                "Runtime.evaluate",
                json!({"result": {"type": "object", "objectId": "doc-1"}}),
            )
            .call("Runtime.callFunctionOn", json!({"result": {"value": 3}}))
            .call_error("Page.captureScreenshot", "target crashed")
            .call("Runtime.callFunctionOn", json!({"result": {"value": 0}})),
    );
    let err = server
        .call_tool("browser_screenshot", &json!({"cdp": CDP, "annotate": true}))
        .unwrap_err();
    assert!(matches!(err, ToolError::Browser(_)), "{err:?}");
    let overlay = calls(&log, "Runtime.callFunctionOn");
    assert_eq!(overlay.len(), 2, "drawn, then removed");
    assert_eq!(
        overlay[0]["functionDeclaration"], overlay[1]["functionDeclaration"],
        "one fixed script"
    );
    assert_eq!(overlay[0]["objectId"], "doc-1");
    let boxes = &overlay[0]["arguments"][0]["value"];
    assert_eq!(boxes.as_array().unwrap().len(), 3, "{boxes}");
    assert_eq!(boxes[0], json!([20.0, 20.0, 300.0, 28.0, "@e1"]));
    assert_eq!(overlay[1]["arguments"], json!([{"value": null}]));
}

#[test]
fn annotated_screenshot_returns_the_image_over_mcp() {
    let (mut server, log) = server(
        ScriptBuilder::new()
            .observe_compact(&login(false))
            .call(
                "Runtime.evaluate",
                json!({"result": {"type": "object", "objectId": "doc-1"}}),
            )
            .call("Runtime.callFunctionOn", json!({"result": {"value": 3}}))
            .call("Page.captureScreenshot", json!({"data": "iVBORw0KGgo="}))
            .call("Runtime.callFunctionOn", json!({"result": {"value": 0}})),
    );
    let line = json!({"jsonrpc": "2.0", "id": 1, "method": "tools/call",
        "params": {"name": "browser_screenshot", "arguments": {"cdp": CDP, "annotate": true}}});
    let reply: Value =
        serde_json::from_str(&server.handle_line(&line.to_string()).unwrap()).unwrap();
    assert_eq!(
        reply["result"]["content"][0],
        json!({"type": "image", "data": "iVBORw0KGgo=", "mimeType": "image/png"}),
        "{reply}"
    );
    assert_eq!(calls(&log, "Runtime.callFunctionOn").len(), 2);
}

#[test]
fn max_chars_cuts_text_at_a_multibyte_boundary() {
    let (mut server, _log) = server(
        ScriptBuilder::new()
            .call(
                "Runtime.evaluate",
                json!({"result": {"type": "string", "value": "naïve 漢字🙂 text"}}),
            )
            .observe_compact(&login(false)),
    );
    let out = call(
        &mut server,
        "browser_get_text",
        json!({"cdp": CDP, "max_chars": 9}),
    );
    assert_eq!(out["text"], "naïve 漢字🙂… [truncated 5 chars]", "{out}");

    let out = call(
        &mut server,
        "browser_get_state",
        json!({"cdp": CDP, "max_chars": 200, "format": "json"}),
    );
    let kept = out["elements"].as_array().unwrap().len();
    assert!(kept < 3, "{out}");
    assert_eq!(out["interactive_elements"], 3, "the total stays: {out}");
    assert!(out["truncated"]
        .as_str()
        .unwrap()
        .starts_with("… [truncated "));
}

#[test]
fn tools_list_carries_the_new_tools_and_options() {
    let reply: Value = serde_json::from_str(
        &Server::new()
            .handle_line(r#"{"jsonrpc":"2.0","id":1,"method":"tools/list"}"#)
            .unwrap(),
    )
    .unwrap();
    let tools = reply["result"]["tools"].as_array().unwrap();
    let schema = |name: &str| {
        tools
            .iter()
            .find(|t| t["name"] == name)
            .unwrap_or_else(|| panic!("{name} listed"))["inputSchema"]
            .clone()
    };
    for name in ultra_instinct_mcp::EXTRA_TOOLS {
        assert!(schema(name)["properties"].get("cdp").is_some(), "{name}");
    }
    assert!(schema("browser_screenshot")["properties"]
        .get("annotate")
        .is_some());
    for name in [
        "browser_get_text",
        "browser_get_state",
        "browser_read_network",
    ] {
        assert!(
            schema(name)["properties"].get("max_chars").is_some(),
            "{name}"
        );
    }
    assert_eq!(
        schema("browser_check")["required"],
        json!(["index", "checked"])
    );
}

/// Console API calls and log entries become `{level, text}` entries; other
/// buffered events are not console entries.
#[test]
fn read_console_turns_console_and_log_events_into_entries() {
    let script = ScriptBuilder::new()
        .call("Runtime.enable", json!({}))
        .call("Log.enable", json!({}));
    let (mut server, _log) = server_with(
        script,
        vec![(
            "Log.enable",
            event(
                "Runtime.consoleAPICalled",
                json!({"type": "warn", "args": [{"value": "careful"}, {"description": "obj"}]}),
            ),
        )],
    );
    let out = call(&mut server, "browser_read_console", json!({"cdp": CDP}));
    let entries = out["entries"].as_array().expect("entries");
    assert_eq!(entries.len(), 1, "{out}");
    assert_eq!(entries[0]["level"], "warn");
    assert!(entries[0]["text"].as_str().unwrap().contains("careful"));

    let script = ScriptBuilder::new()
        .call("Runtime.enable", json!({}))
        .call("Log.enable", json!({}));
    let (mut server, _log) = server_with(
        script,
        vec![
            (
                "Runtime.enable",
                event("Network.requestWillBeSent", json!({})),
            ),
            (
                "Log.enable",
                event(
                    "Log.entryAdded",
                    json!({"entry": {"level": "error", "text": "boom", "source": "network"}}),
                ),
            ),
        ],
    );
    let out = call(&mut server, "browser_read_console", json!({"cdp": CDP}));
    assert_eq!(out["entries"].as_array().map(Vec::len), Some(1), "{out}");
    assert_eq!(out["entries"][0]["level"], "error");
    assert_eq!(out["entries"][0]["text"], "boom");
}
