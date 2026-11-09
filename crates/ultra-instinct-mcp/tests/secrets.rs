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
