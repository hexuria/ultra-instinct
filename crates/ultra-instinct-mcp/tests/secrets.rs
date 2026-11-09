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
/// (the list used to keep the newest 32).
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
