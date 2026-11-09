//! Label folding through `browser_get_state`: a field named by a
//! label is listed once, as the field, and a custom checkbox is listed as a
//! checkbox on its label's box.

use std::cell::RefCell;
use std::rc::Rc;

use serde_json::{json, Value};
use ultra_instinct_browser::script::{PageSpec, ScriptBuilder};
use ultra_instinct_browser::{CdpError, CdpTransport, ReplayTransport};
use ultra_instinct_mcp::Server;

const CDP: &str = "http://127.0.0.1:9222";

/// Records the params of every `DOM.resolveNode`: the node a press lands on.
struct Logged {
    inner: ReplayTransport,
    resolved: Rc<RefCell<Vec<Value>>>,
}

impl CdpTransport for Logged {
    fn call(&mut self, method: &str, params_json: &str) -> Result<String, CdpError> {
        if method == "DOM.resolveNode" {
            self.resolved
                .borrow_mut()
                .push(serde_json::from_str(params_json).unwrap());
        }
        self.inner.call(method, params_json)
    }
}

fn server(script: ScriptBuilder) -> (Server, Rc<RefCell<Vec<Value>>>) {
    let resolved = Rc::new(RefCell::new(Vec::new()));
    let shared = Rc::clone(&resolved);
    let mut script = Some(script);
    let server = Server::with_connector(move |_url| {
        let script = script.take().ok_or_else(|| CdpError::Transport {
            message: "one connect per test".into(),
        })?;
        Ok(Box::new(Logged {
            inner: ReplayTransport::parse(&script.to_json())?,
            resolved: Rc::clone(&shared),
        }) as Box<dyn CdpTransport>)
    });
    (server, resolved)
}

fn call(server: &mut Server, tool: &str, arguments: Value) -> Value {
    server
        .call_tool(tool, &arguments)
        .unwrap_or_else(|err| panic!("{tool} {arguments}: {err:?}"))
}

#[test]
fn get_state_lists_each_labelled_field_once() {
    let page = PageSpec::labelled_form(false);
    let (mut server, _) = server(ScriptBuilder::new().observe_compact(&page));
    let out = call(&mut server, "browser_get_state", json!({"cdp": CDP}));
    let text = out["text"].as_str().expect("text");
    let lines: Vec<&str> = text.lines().skip(1).collect();
    assert_eq!(
        lines,
        [
            "@e1 textbox \"Email\"",
            "@e2 textbox \"Password\"",
            "@e3 checkbox \"Remember me\"  unchecked",
            "@e4 link \"terms\"",
            "@e5 checkbox \"I agree to the terms\"",
            "@e6 textbox \"Nickname\"",
            "@e7 checkbox \"Send me news\"  unchecked",
            "@e8 button \"Sign in\"",
            &format!("8 controls · snapshot {}", out["snapshot"]),
        ],
        "{text}"
    );
}

#[test]
fn a_custom_checkbox_is_clicked_through_its_label() {
    let page = PageSpec::labelled_form(false);
    let (mut server, resolved) = server(
        ScriptBuilder::new()
            .observe_compact(&page) // get_state
            .observe_compact(&page) // the click reads its index
            .observe_compact(&page) // the executor's fresh observe
            .dom_click(61), // a DOM click on the label node toggles its input
    );
    let state = call(
        &mut server,
        "browser_get_state",
        json!({"cdp": CDP, "format": "json"}),
    );
    let news = &state["elements"][6];
    assert_eq!(
        (&news["label"], &news["role"]),
        (&json!("Send me news"), &json!("checkbox"))
    );
    assert_eq!(news["rect"]["w"], 120.0, "the label's box");
    let out = call(
        &mut server,
        "browser_click",
        json!({"cdp": CDP, "index": 7}),
    );
    assert_eq!(out["acted"], "click", "{out}");
    assert_eq!(resolved.borrow()[0]["nodeId"], 61, "the label's node");
}
