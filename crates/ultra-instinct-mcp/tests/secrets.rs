//! A secret typed into a password field never comes back out of any tool.
//!
//! The scripted page echoes the secret everywhere a page can (its text, its
//! HTML, a console log, a GET URL, an exception, the HAR), and every read
//! tool runs after the type. No output, error, or file may contain it.

use std::collections::VecDeque;

use serde_json::{json, Value};
use ultra_instinct_browser::script::{Control, PageSpec, ScriptBuilder};
use ultra_instinct_browser::{CdpError, CdpTransport, ReplayTransport};
use ultra_instinct_cdp::CdpEvent;
use ultra_instinct_mcp::{Server, ToolError, MASK};

const CDP: &str = "http://127.0.0.1:9222";
const SECRET: &str = "hunter2-Zq9!";

/// Replay plus events released by the call that triggers them.
struct Echoing {
    inner: ReplayTransport,
    triggers: VecDeque<(&'static str, CdpEvent)>,
    pending: Vec<CdpEvent>,
}

impl CdpTransport for Echoing {
    fn call(&mut self, method: &str, params_json: &str) -> Result<String, CdpError> {
        let result = self.inner.call(method, params_json);
        if result.is_ok() && self.triggers.front().is_some_and(|(m, _)| *m == method) {
            let (_, event) = self.triggers.pop_front().unwrap();
            self.pending.push(event);
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

/// Email @e1 (a text field), Password @e2.
fn page() -> PageSpec {
    let mut page = PageSpec::of(
        &[
            Control::text_field(10, 100, "Email", (20.0, 20.0, 300.0, 28.0)),
            Control::text_field(11, 101, "Password", (20.0, 80.0, 300.0, 28.0)),
        ],
        "https://shop.example/login",
        "Log in",
    );
    page.dom[0] = page.dom[0]
        .clone()
        .with_compact_state(json!({"it": "email", "v": "me@shop.example"}));
    page.dom[1] = page.dom[1]
        .clone()
        .with_compact_state(json!({"it": "password"}));
    page
}

fn string_result(text: &str) -> Value {
    json!({"result": {"type": "string", "value": text}})
}

#[test]
fn a_typed_password_appears_in_no_output_error_or_file() {
    let echo_url = format!("https://shop.example/login?pw={SECRET}");
    let script = ScriptBuilder::new()
        .observe_compact(&page()) // get_state json
        .observe_compact(&page()) // get_state text
        .observe_compact(&page()) // type: resolve
        .observe_compact(&page()) // type: executor
        .dom_input(11)
        .call(
            "Runtime.evaluate",
            string_result(&format!("Welcome back. Your password {SECRET} is weak.")),
        ) // get_text
        .call(
            "Runtime.evaluate",
            string_result(&format!("<input type=password value=\"{SECRET}\">")),
        ) // get_html
        .call(
            "Runtime.evaluate",
            string_result(&format!("# Log in\nyou typed {SECRET}")),
        ) // extract_content
        .call("Runtime.enable", json!({}))
        .call("Log.enable", json!({})) // read_console
        .call("Network.enable", json!({})) // read_network
        .observe_compact(&page()) // get value
        .observe_compact(&page()) // get attr value
        .call(
            "Runtime.evaluate",
            json!({"result": {"type": "string", "value": SECRET}}),
        ) // exec
        .call_error(
            "Runtime.evaluate",
            &format!("ReferenceError: {SECRET} is not defined"),
        ) // exec error
        .call("Page.reload", json!({})); // a call while HAR records
    let triggers: VecDeque<(&'static str, CdpEvent)> = [
        (
            "Log.enable",
            event(
                "Runtime.consoleAPICalled",
                json!({"type": "log", "args": [{"value": format!("pw={SECRET}")}]}),
            ),
        ),
        (
            "Network.enable",
            event(
                "Network.requestWillBeSent",
                json!({"requestId": "r1", "request": {"url": echo_url, "method": "GET"}}),
            ),
        ),
    ]
    .into_iter()
    .collect();
    let har_event = event(
        "Network.requestWillBeSent",
        json!({"requestId": "r2", "timestamp": 1.0, "wallTime": 1.0,
               "request": {"url": echo_url, "method": "GET", "headers": {}}}),
    );
    let mut connect = Some((script, triggers, har_event));
    let mut server = Server::with_connector(move |_url| {
        let (script, mut triggers, har_event) =
            connect.take().ok_or_else(|| CdpError::Transport {
                message: "one connect".into(),
            })?;
        // The request a page sends while the HAR records.
        triggers.push_back(("Page.reload", har_event));
        Ok(Box::new(Echoing {
            inner: ReplayTransport::parse(&script.to_json())?,
            triggers,
            pending: Vec::new(),
        }) as Box<dyn CdpTransport>)
    });

    let mut outputs: Vec<String> = Vec::new();
    let mut run = |server: &mut Server, tool: &str, arguments: Value| -> Result<Value, ToolError> {
        let result = server.call_tool(tool, &arguments);
        match &result {
            Ok(body) => outputs.push(format!("{tool}: {body}")),
            Err(err) => outputs.push(format!("{tool}: {} {err}", err.to_json_string())),
        }
        result
    };

    let state = run(
        &mut server,
        "browser_get_state",
        json!({"cdp": CDP, "format": "json"}),
    )
    .unwrap();
    let password = &state["elements"][1];
    assert_eq!(password["label"], "Password");
    assert_eq!(password["sensitive"], true, "{state}");
    assert_eq!(password["input_type"], "password");
    assert_eq!(state["elements"][0]["sensitive"], false, "{state}");
    let text = run(&mut server, "browser_get_state", json!({"cdp": CDP})).unwrap();
    let snapshot = text["snapshot"].as_u64().unwrap();

    let typed = run(
        &mut server,
        "browser_type",
        json!({"cdp": CDP, "index": 2, "text": SECRET, "snapshot": snapshot}),
    )
    .unwrap();
    assert_eq!(typed["acted"], "type", "{typed}");

    let page_text = run(&mut server, "browser_get_text", json!({"cdp": CDP})).unwrap();
    assert!(
        page_text["text"].as_str().unwrap().contains(MASK),
        "{page_text}"
    );
    run(&mut server, "browser_get_html", json!({"cdp": CDP})).unwrap();
    run(&mut server, "browser_extract_content", json!({"cdp": CDP})).unwrap();
    let console = run(&mut server, "browser_read_console", json!({"cdp": CDP})).unwrap();
    assert_eq!(console["entries"].as_array().unwrap().len(), 1, "{console}");
    let network = run(&mut server, "browser_read_network", json!({"cdp": CDP})).unwrap();
    assert_eq!(network["entries"].as_array().unwrap().len(), 1, "{network}");
    let value = run(
        &mut server,
        "browser_get",
        json!({"cdp": CDP, "index": 2, "what": "value"}),
    )
    .unwrap();
    assert_eq!(value["value"], Value::Null, "{value}");
    let attr = run(
        &mut server,
        "browser_get",
        json!({"cdp": CDP, "index": 2, "what": "attr", "name": "value"}),
    );
    assert!(
        attr.is_err(),
        "a sensitive field's value attribute is refused"
    );
    run(
        &mut server,
        "browser_exec",
        json!({"cdp": CDP, "script": "pw()"}),
    )
    .unwrap();
    assert!(run(
        &mut server,
        "browser_exec",
        json!({"cdp": CDP, "script": "x"})
    )
    .is_err());

    let har = std::env::temp_dir().join(format!("ui-secret-{}.har", std::process::id()));
    run(
        &mut server,
        "browser_network",
        json!({"cdp": CDP, "action": "har_start"}),
    )
    .unwrap();
    run(&mut server, "browser_reload", json!({"cdp": CDP})).unwrap();
    run(
        &mut server,
        "browser_network",
        json!({"cdp": CDP, "action": "har_stop", "path": har.to_str().unwrap()}),
    )
    .unwrap();
    let har_text = std::fs::read_to_string(&har).unwrap();
    let _ = std::fs::remove_file(&har);
    assert!(har_text.contains("shop.example/login?pw="), "{har_text}");
    outputs.push(format!("har: {har_text}"));

    for output in &outputs {
        assert!(!output.contains(SECRET), "leaked: {output}");
    }
    assert!(
        outputs.iter().filter(|o| o.contains(MASK)).count() >= 6,
        "{outputs:#?}"
    );
}

/// A replay whose `Runtime.evaluate` echoes `SECRET` once `echo` is on (the
/// page shows what was typed); every other call follows the script.
struct EchoAfter {
    inner: ReplayTransport,
    echo: std::rc::Rc<std::cell::Cell<bool>>,
}

impl CdpTransport for EchoAfter {
    fn call(&mut self, method: &str, params_json: &str) -> Result<String, CdpError> {
        if self.echo.get() && method == "Runtime.evaluate" {
            return Ok(string_result(&format!("Signed in with {SECRET}")).to_string());
        }
        self.inner.call(method, params_json)
    }
}

/// `Server::run_goal` types into the password field through the agent loop
/// (not a `browser_*` tool): the text is masked in every later output.
#[test]
fn a_password_typed_by_run_goal_is_masked_in_later_outputs() {
    let echo = std::rc::Rc::new(std::cell::Cell::new(false));
    let on = std::rc::Rc::clone(&echo);
    let script = (0..6)
        .fold(ScriptBuilder::new(), |s, _| s.observe_compact(&page()))
        .dom_input(11);
    let mut connect = Some(script);
    let mut server = Server::with_connector(move |_url| {
        let script = connect.take().ok_or_else(|| CdpError::Transport {
            message: "one connect".into(),
        })?;
        Ok(Box::new(EchoAfter {
            inner: ReplayTransport::parse(&script.to_json())?,
            echo: std::rc::Rc::clone(&on),
        }) as Box<dyn CdpTransport>)
    });
    let report = server.run_goal(
        &format!("type \"{SECRET}\" into Password"),
        ultra_instinct_mcp::RunLimits {
            max_steps: 2,
            deadline: None,
        },
    );
    assert!(
        report.steps.iter().any(|s| s.action == "TYPE_TEXT") || !report.typed.is_empty(),
        "the run typed: {report:?}"
    );
    assert!(!format!("{report:?}").contains(SECRET), "{report:?}");
    echo.set(true);
    let text = server
        .call_tool("browser_get_text", &json!({"cdp": CDP}))
        .expect("text");
    assert_eq!(text["text"], format!("Signed in with {MASK}"), "{text}");
}

/// Masking runs on every tool's result, not only `browser_*` ones: an
/// observe label that echoes a typed secret is masked.
#[test]
fn observe_output_is_masked_too() {
    let mut echoed = page();
    echoed.dom[0].label = format!("Email ({SECRET})");
    echoed.ax[0].name = format!("Email ({SECRET})");
    let mut connect = Some(
        ScriptBuilder::new()
            .observe_compact(&page())
            .observe_compact(&page())
            .dom_input(11)
            .observe_compact(&echoed),
    );
    let mut server = Server::with_connector(move |_url| {
        let script = connect.take().ok_or_else(|| CdpError::Transport {
            message: "one connect".into(),
        })?;
        Ok(Box::new(ReplayTransport::parse(&script.to_json())?) as Box<dyn CdpTransport>)
    });
    server
        .call_tool(
            "browser_type",
            &json!({"cdp": CDP, "index": 2, "text": SECRET}),
        )
        .expect("typed");
    let observed = server
        .call_tool("observe", &json!({"cdp": CDP}))
        .expect("observe");
    let text = observed.to_string();
    assert!(!text.contains(SECRET), "{text}");
    assert!(text.contains(MASK), "{text}");
}

/// A server that has typed `text` into field `index` of `page` (an
/// unchecked snapshot-free `browser_type`), then answers `then` in order.
fn typed_server(
    page: PageSpec,
    index: i64,
    text: &str,
    then: impl FnOnce(ScriptBuilder) -> ScriptBuilder,
) -> Server {
    let script = then(
        ScriptBuilder::new()
            .observe_compact(&page)
            .observe_compact(&page)
            .dom_input(9 + index),
    );
    let mut connect = Some(script);
    let mut server = Server::with_connector(move |_url| {
        let script = connect.take().ok_or_else(|| CdpError::Transport {
            message: "one connect".into(),
        })?;
        Ok(Box::new(ReplayTransport::parse(&script.to_json())?) as Box<dyn CdpTransport>)
    });
    server
        .call_tool(
            "browser_type",
            &json!({"cdp": CDP, "index": index, "text": text}),
        )
        .expect("typed");
    server
}

/// A 3-digit CVV typed into a CVV field is masked as a whole token; the
/// same digits inside a longer number stay.
#[test]
fn a_short_cvv_is_masked_as_a_whole_token_only() {
    let mut card = page();
    card.dom[1].label = "CVV".into();
    card.ax[1].name = "CVV".into();
    card.dom[1] = card.dom[1]
        .clone()
        .with_compact_state(json!({"it": "text"}));
    let then = |s: ScriptBuilder| {
        s.call(
            "Runtime.evaluate",
            string_result("CVV 123 accepted for order 41123."),
        )
    };
    let mut server = typed_server(card, 2, "123", then);
    let text = server
        .call_tool("browser_get_text", &json!({"cdp": CDP}))
        .expect("text");
    assert_eq!(
        text["text"],
        format!("CVV {MASK} accepted for order 41123."),
        "{text}"
    );
}

/// `browser_exec` returning `{data: <secret>}` is masked: only the
/// screenshot / PDF binary field is exempt, not every `data` key.
#[test]
fn an_exec_result_under_a_data_key_is_masked() {
    let then = |s: ScriptBuilder| {
        s.call(
            "Runtime.evaluate",
            json!({"result": {"type": "object", "value": {"data": SECRET, "base64": SECRET}}}),
        )
    };
    let mut server = typed_server(page(), 2, SECRET, then);
    let out = server
        .call_tool("browser_exec", &json!({"cdp": CDP, "script": "leak()"}))
        .expect("exec");
    assert!(!out.to_string().contains(SECRET), "{out}");
    assert_eq!(out["result"]["data"], MASK, "{out}");
}

/// A password stays masked however many other secrets are typed after it
/// (only plain entries are capped).
#[test]
fn a_typed_password_is_never_evicted_by_later_typing() {
    let then = |mut s: ScriptBuilder| {
        for _ in 0..40 {
            s = s
                .observe_compact(&page())
                .observe_compact(&page())
                .dom_input(11);
        }
        s.call(
            "Runtime.evaluate",
            string_result(&format!("password {SECRET}")),
        )
    };
    let mut server = typed_server(page(), 2, SECRET, then);
    for n in 0..40 {
        server
            .call_tool(
                "browser_type",
                &json!({"cdp": CDP, "index": 2, "text": format!("other-pass-{n}")}),
            )
            .expect("typed");
    }
    let text = server
        .call_tool("browser_get_text", &json!({"cdp": CDP}))
        .expect("text");
    assert_eq!(text["text"], format!("password {MASK}"), "{text}");
}

/// Nickname @e1 (a plain text field, or the same box turned into a password
/// input), Password @e2.
fn nickname(kind: &str) -> PageSpec {
    let mut page = page();
    page.dom[0].label = "Nickname".into();
    page.ax[0].name = "Nickname".into();
    page.dom[0] = page.dom[0]
        .clone()
        .with_compact_state(json!({"it": kind, "v": ""}));
    page
}

fn connect_once(script: ScriptBuilder) -> Server {
    let mut connect = Some(script);
    Server::with_connector(move |_url| {
        let script = connect.take().ok_or_else(|| CdpError::Transport {
            message: "one connect".into(),
        })?;
        Ok(Box::new(ReplayTransport::parse(&script.to_json())?) as Box<dyn CdpTransport>)
    })
}

/// The field the caller saw was plain text; by the time the executor
/// observes, the page made it a password input (`input_type` is not in the
/// fingerprint, so revalidation passes). The text typed into it is a secret.
#[test]
fn a_field_that_became_a_password_after_get_state_masks_what_is_typed() {
    let mut server = connect_once(
        ScriptBuilder::new()
            .observe_compact(&nickname("text")) // get_state
            .observe_compact(&nickname("text")) // type: resolve
            .observe_compact(&nickname("password")) // type: executor
            .dom_input(10)
            .call(
                "Runtime.evaluate",
                string_result(&format!("Saved {SECRET}")),
            ),
    );
    let state = server
        .call_tool("browser_get_state", &json!({"cdp": CDP, "format": "json"}))
        .expect("state");
    assert_eq!(state["elements"][0]["sensitive"], false, "{state}");
    server
        .call_tool(
            "browser_type",
            &json!({"cdp": CDP, "index": 1, "text": SECRET, "snapshot": state["snapshot"]}),
        )
        .expect("typed");
    let text = server
        .call_tool("browser_get_text", &json!({"cdp": CDP}))
        .expect("text");
    assert_eq!(text["text"], format!("Saved {MASK}"), "{text}");
}

/// `browser_keyboard` types at the focus. The last observation (get_state,
/// Email focused) is stale once a click moved the focus into Password: the
/// focus is read again, and the text typed there is a secret.
#[test]
fn keyboard_text_after_a_click_into_a_password_field_is_masked() {
    let focused = |at: usize| {
        let mut page = page();
        page.ax[at] = page.ax[at].clone().focused();
        page
    };
    let mut script = ScriptBuilder::new()
        .observe_compact(&focused(0)) // get_state
        .observe_compact(&focused(0)) // click: resolve
        .observe_compact(&focused(0)) // click: executor
        .dom_click(11)
        .observe_compact(&focused(1)) // keyboard: the focus now
        .call(
            "Runtime.evaluate",
            json!({"result": {"type": "boolean", "value": true}}),
        );
    for _ in 0.."hunter2".len() * 2 {
        script = script.call("Input.dispatchKeyEvent", json!({}));
    }
    let script = script.call(
        "Runtime.evaluate",
        string_result("Your password hunter2 is weak."),
    );
    let mut server = connect_once(script);
    let state = server
        .call_tool("browser_get_state", &json!({"cdp": CDP}))
        .expect("state");
    server
        .call_tool(
            "browser_click",
            &json!({"cdp": CDP, "index": 2, "snapshot": state["snapshot"]}),
        )
        .expect("clicked");
    let typed = server
        .call_tool("browser_keyboard", &json!({"cdp": CDP, "text": "hunter2"}))
        .expect("typed");
    assert_eq!(typed["typed"], 7, "{typed}");
    let text = server
        .call_tool("browser_get_text", &json!({"cdp": CDP}))
        .expect("text");
    assert_eq!(
        text["text"],
        format!("Your password {MASK} is weak."),
        "{text}"
    );
}

/// A replay that logs `(method, params)` of every call.
struct Logging {
    inner: ReplayTransport,
    log: std::rc::Rc<std::cell::RefCell<Vec<(String, Value)>>>,
}

impl CdpTransport for Logging {
    fn call(&mut self, method: &str, params_json: &str) -> Result<String, CdpError> {
        let params = serde_json::from_str(params_json).unwrap_or(Value::Null);
        self.log.borrow_mut().push((method.to_owned(), params));
        self.inner.call(method, params_json)
    }
}

type CallLog = std::rc::Rc<std::cell::RefCell<Vec<(String, Value)>>>;

/// Email @e1, CVV @e2 (a type=text field the label makes sensitive).
fn card() -> PageSpec {
    let mut card = page();
    card.dom[1].label = "CVV".into();
    card.ax[1].name = "CVV".into();
    card.dom[1] = card.dom[1]
        .clone()
        .with_compact_state(json!({"it": "text"}));
    card
}

/// A server that typed "123" into the CVV, then answers `then`.
fn typed_cvv(then: impl FnOnce(ScriptBuilder) -> ScriptBuilder) -> (Server, CallLog) {
    let script = then(
        ScriptBuilder::new()
            .observe_compact(&card())
            .observe_compact(&card())
            .dom_input(11),
    );
    let log: CallLog = std::rc::Rc::default();
    let shared = std::rc::Rc::clone(&log);
    let mut connect = Some(script);
    let mut server = Server::with_connector(move |_url| {
        let script = connect.take().ok_or_else(|| CdpError::Transport {
            message: "one connect".into(),
        })?;
        Ok(Box::new(Logging {
            inner: ReplayTransport::parse(&script.to_json())?,
            log: std::rc::Rc::clone(&shared),
        }) as Box<dyn CdpTransport>)
    });
    server
        .call_tool(
            "browser_type",
            &json!({"cdp": CDP, "index": 2, "text": "123"}),
        )
        .expect("typed");
    log.borrow_mut().clear();
    (server, log)
}

/// What a capture sends around itself: resolve the CVV's node, hide it
/// (the node as an argument, never in the script), capture, restore the
/// style it had, free the handles.
fn hide_then(
    script: ScriptBuilder,
    capture: impl FnOnce(ScriptBuilder) -> ScriptBuilder,
) -> ScriptBuilder {
    let script = script
        .observe_compact(&card())
        .call(
            "DOM.resolveNode",
            json!({"object": {"type": "object", "objectId": "cvv-1"}}),
        )
        .call(
            "Runtime.callFunctionOn",
            json!({"result": {"type": "string", "value": "width: 80px"}}),
        );
    capture(script)
        .call(
            "Runtime.callFunctionOn",
            json!({"result": {"type": "boolean", "value": true}}),
        )
        .call("Runtime.releaseObjectGroup", json!({}))
}

fn capture_calls(log: &CallLog) -> Vec<String> {
    log.borrow()
        .iter()
        .map(|(m, _)| m.clone())
        .filter(|m| {
            m.starts_with("DOM.resolveNode")
                || m.starts_with("Runtime.callFunctionOn")
                || m.starts_with("Runtime.releaseObjectGroup")
                || m.starts_with("Page.captureScreenshot")
                || m.starts_with("Page.printToPDF")
        })
        .collect()
}

#[test]
fn a_screenshot_hides_typed_sensitive_values_and_restores_them() {
    let (mut server, log) = typed_cvv(|s| {
        hide_then(s, |s| {
            s.call("Page.captureScreenshot", json!({"data": "iVBORw0K"}))
        })
    });
    let shot = server
        .call_tool("browser_screenshot", &json!({"cdp": CDP}))
        .expect("screenshot");
    assert_eq!(shot["base64"], "iVBORw0K", "{shot}");
    assert!(shot.get("restore_error").is_none(), "{shot}");
    assert_eq!(
        capture_calls(&log),
        [
            "DOM.resolveNode",
            "Runtime.callFunctionOn",
            "Page.captureScreenshot",
            "Runtime.callFunctionOn",
            "Runtime.releaseObjectGroup",
        ]
    );
    let calls = log.borrow();
    let of = |method: &str| -> Vec<Value> {
        calls
            .iter()
            .filter(|(m, _)| m == method)
            .map(|(_, p)| p.clone())
            .collect()
    };
    assert_eq!(of("DOM.resolveNode")[0]["nodeId"], 11, "the CVV, not Email");
    let functions = of("Runtime.callFunctionOn");
    let (hide, restore) = (&functions[0], &functions[1]);
    assert!(hide["functionDeclaration"]
        .as_str()
        .unwrap()
        .contains("-webkit-text-security"));
    assert_eq!(
        hide["objectId"], "cvv-1",
        "the field is the handle, not script text"
    );
    assert!(hide.get("arguments").is_none(), "{hide}");
    assert_eq!(restore["objectId"], "cvv-1");
    assert_eq!(
        restore["arguments"],
        json!([{"value": "width: 80px"}]),
        "the style it had goes back"
    );
}

#[test]
fn the_restore_runs_when_the_capture_fails() {
    let (mut server, log) =
        typed_cvv(|s| hide_then(s, |s| s.call_error("Page.printToPDF", "Printing failed")));
    let err = server
        .call_tool("browser_save_as_pdf", &json!({"cdp": CDP}))
        .expect_err("the capture fails");
    assert!(
        matches!(&err, ToolError::Browser(m) if m.contains("Printing failed")),
        "{err:?}"
    );
    assert_eq!(
        capture_calls(&log),
        [
            "DOM.resolveNode",
            "Runtime.callFunctionOn",
            "Page.printToPDF",
            "Runtime.callFunctionOn",
            "Runtime.releaseObjectGroup",
        ]
    );
}

#[test]
fn a_field_that_cannot_be_hidden_refuses_the_capture() {
    let (mut server, log) = typed_cvv(|s| {
        s.observe_compact(&card())
            .call_error("DOM.resolveNode", "No node with given id found")
    });
    let err = server
        .call_tool("browser_screenshot", &json!({"cdp": CDP}))
        .expect_err("refused");
    assert_eq!(err.code(), "secret_on_screen", "{err:?}");
    assert!(!capture_calls(&log).contains(&"Page.captureScreenshot".to_owned()));
}

/// Nothing secret typed yet: a screenshot is the one capture call it was.
#[test]
fn a_screenshot_with_no_typed_secret_hides_nothing() {
    let log: CallLog = std::rc::Rc::default();
    let shared = std::rc::Rc::clone(&log);
    let mut connect =
        Some(ScriptBuilder::new().call("Page.captureScreenshot", json!({"data": "iVBORw0K"})));
    let mut server = Server::with_connector(move |_url| {
        let script = connect.take().ok_or_else(|| CdpError::Transport {
            message: "one connect".into(),
        })?;
        Ok(Box::new(Logging {
            inner: ReplayTransport::parse(&script.to_json())?,
            log: std::rc::Rc::clone(&shared),
        }) as Box<dyn CdpTransport>)
    });
    server
        .call_tool("browser_screenshot", &json!({"cdp": CDP}))
        .expect("screenshot");
    assert_eq!(capture_calls(&log), ["Page.captureScreenshot"]);
}

/// Two sensitive fields; the second cannot be resolved: the first one,
/// already hidden, is put back before the refusal, and nothing is captured.
#[test]
fn a_refused_capture_puts_back_the_fields_it_already_hid() {
    let mut two = card();
    two.dom[0].label = "PIN".into();
    two.ax[0].name = "PIN".into();
    two.dom[0] = two.dom[0]
        .clone()
        .with_compact_state(json!({"it": "text", "v": ""}));
    let (mut server, log) = typed_cvv(move |s| {
        s.observe_compact(&two)
            .call(
                "DOM.resolveNode",
                json!({"object": {"type": "object", "objectId": "pin-1"}}),
            )
            .call(
                "Runtime.callFunctionOn",
                json!({"result": {"type": "object", "subtype": "null", "value": null}}),
            )
            .call_error("DOM.resolveNode", "No node with given id found")
            .call(
                "Runtime.callFunctionOn",
                json!({"result": {"type": "boolean", "value": true}}),
            )
            .call("Runtime.releaseObjectGroup", json!({}))
    });
    let err = server
        .call_tool("browser_screenshot", &json!({"cdp": CDP}))
        .expect_err("refused");
    assert_eq!(err.code(), "secret_on_screen", "{err:?}");
    assert_eq!(
        capture_calls(&log),
        [
            "DOM.resolveNode",
            "Runtime.callFunctionOn",
            "DOM.resolveNode",
            "Runtime.callFunctionOn",
            "Runtime.releaseObjectGroup",
        ]
    );
    let calls = log.borrow();
    let restore = calls
        .iter()
        .filter(|(m, _)| m == "Runtime.callFunctionOn")
        .nth(1)
        .unwrap();
    assert_eq!(restore.1["objectId"], "pin-1");
    assert_eq!(
        restore.1["arguments"],
        json!([{"value": null}]),
        "no style before: none after"
    );
}

/// `values: true` on `browser_get_state` JSON: a plain field's value, none
/// for the password field (even with the typed secret reported back), and a
/// secret the page copied into a plain field masked like every output.
/// Without `values` no element carries one.
#[test]
fn get_state_values_never_carry_a_secret() {
    let mut echoed = page();
    echoed.dom[0] = echoed.dom[0]
        .clone()
        .with_compact_state(json!({"it": "email", "v": format!("me+{SECRET}@shop.example")}));
    echoed.dom[1] = echoed.dom[1]
        .clone()
        .with_compact_state(json!({"it": "password", "v": SECRET}));
    let mut server = typed_server(page(), 2, SECRET, |s| {
        s.observe_compact(&echoed)
            .observe_compact(&page())
            .observe_compact(&page())
            .observe_compact(&page())
    });
    let state = |server: &mut Server, values: Option<bool>| {
        let mut arguments = json!({"cdp": CDP, "format": "json"});
        if let Some(values) = values {
            arguments["values"] = json!(values);
        }
        server.call_tool("browser_get_state", &arguments).unwrap()
    };
    let out = state(&mut server, Some(true));
    assert!(!out.to_string().contains(SECRET), "{out}");
    let elements = &out["elements"];
    assert_eq!(
        elements[0]["value"],
        format!("me+{MASK}@shop.example"),
        "{out}"
    );
    assert_eq!(elements[1]["sensitive"], true);
    assert!(elements[1].get("value").is_none(), "{out}");
    let out = state(&mut server, Some(true));
    assert_eq!(out["elements"][0]["value"], "me@shop.example", "{out}");
    assert!(out["elements"][1].get("value").is_none(), "{out}");
    for values in [None, Some(false)] {
        let out = state(&mut server, values);
        let elements = out["elements"].as_array().unwrap();
        assert!(elements.iter().all(|e| e.get("value").is_none()), "{out}");
        assert_eq!(elements[0]["label"], "Email");
    }
}

/// A Tab inside `browser_keyboard` text moves the focus after the focus was
/// read (Email → Password here): everything after the first Tab may be a
/// secret and is masked in later outputs, at any length.
#[test]
fn keyboard_text_after_a_tab_is_masked() {
    let mut email_focused = page();
    email_focused.ax[0] = email_focused.ax[0].clone().focused();
    let focused = json!({"result": {"type": "boolean", "value": true}});
    // "bob\tpw123": 3 + 1 + 5 keys, each a keyDown and a keyUp.
    let mut script = ScriptBuilder::new()
        .observe_compact(&email_focused)
        .call("Runtime.evaluate", focused);
    for _ in 0..18 {
        script = script.call("Input.dispatchKeyEvent", json!({}));
    }
    let script = script.call("Runtime.evaluate", string_result("signed in as bob, pw123"));
    let mut server = connect_once(script);
    server
        .call_tool("browser_get_state", &json!({"cdp": CDP}))
        .expect("state");
    server
        .call_tool(
            "browser_keyboard",
            &json!({"cdp": CDP, "text": "bob\tpw123"}),
        )
        .expect("typed");
    let text = server
        .call_tool("browser_get_text", &json!({"cdp": CDP}))
        .expect("text");
    assert_eq!(text["text"], format!("signed in as bob, {MASK}"), "{text}");
}
