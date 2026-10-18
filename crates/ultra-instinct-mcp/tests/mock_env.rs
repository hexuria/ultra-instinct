//! Mock environment: firewall flows against a scripted Chrome.
//!
//! Each test drives one [`Server`] through `call_tool` over a `cdp` endpoint.
//! A connector hands the server a [`ReplayTransport`] instead of a websocket.
//! Ultra-Instinct never clicks: these tests assert `guard` decisions and that no
//! press CDP methods are logged.

use std::cell::RefCell;
use std::collections::{BTreeMap, VecDeque};
use std::rc::Rc;

use serde_json::{json, Value};
use ultra_instinct_browser::script::{Control, PageSpec, ScriptBuilder};
use ultra_instinct_browser::{CdpError, CdpTransport, ReplayTransport};
use ultra_instinct_mcp::Server;

struct Logged {
    inner: ReplayTransport,
    log: Rc<RefCell<Vec<String>>>,
}

impl CdpTransport for Logged {
    fn call(&mut self, method: &str, params_json: &str) -> Result<String, CdpError> {
        self.log.borrow_mut().push(method.to_owned());
        self.inner.call(method, params_json)
    }
}

#[derive(Clone, Default)]
struct MockChrome {
    scripts: Rc<RefCell<BTreeMap<String, VecDeque<ScriptBuilder>>>>,
    log: Rc<RefCell<Vec<String>>>,
}

impl MockChrome {
    fn tab(&self, endpoint: &str, script: ScriptBuilder) -> &Self {
        self.scripts
            .borrow_mut()
            .entry(endpoint.to_owned())
            .or_default()
            .push_back(script);
        self
    }

    fn server(&self) -> Server {
        let mock = self.clone();
        Server::with_connector(move |url| {
            let script = mock
                .scripts
                .borrow_mut()
                .get_mut(url)
                .and_then(VecDeque::pop_front)
                .ok_or_else(|| CdpError::Transport {
                    message: format!("no mock tab at {url}"),
                })?;
            let inner = ReplayTransport::parse(&script.to_json())?;
            Ok(Box::new(Logged {
                inner,
                log: Rc::clone(&mock.log),
            }) as Box<dyn CdpTransport>)
        })
    }

    fn presses(&self) -> usize {
        self.log
            .borrow()
            .iter()
            .filter(|m| *m == "DOM.resolveNode" || *m == "Input.dispatchMouseEvent")
            .count()
    }
}

fn rect(x: f64, y: f64) -> (f64, f64, f64, f64) {
    (x, y, 80.0, 32.0)
}

fn sign_in_page() -> PageSpec {
    PageSpec::of(
        &[
            Control::button(10, 100, "Sign in", rect(600.0, 340.0)),
            Control::link(20, 200, "Forgot password", rect(600.0, 400.0)),
        ],
        "https://example.test/sign-in",
        "Sign in",
    )
}

fn twins_page() -> PageSpec {
    PageSpec::of(
        &[
            Control::button(10, 100, "Send", rect(100.0, 340.0)),
            Control::button(20, 200, "Send", rect(400.0, 340.0)),
        ],
        "https://example.test/compose",
        "Compose",
    )
}

fn call(server: &mut Server, tool: &str, arguments: Value) -> Value {
    server
        .call_tool(tool, &arguments)
        .unwrap_or_else(|err| panic!("{tool} {arguments}: {err:?}"))
}

#[test]
fn observe_then_guard_allows_sign_in_without_clicking() {
    let tab = "ws://mock/allow";
    let mock = MockChrome::default();
    mock.tab(
        tab,
        ScriptBuilder::new()
            .observe(&sign_in_page())
            .observe(&sign_in_page()),
    );
    let mut server = mock.server();

    let observed = call(&mut server, "observe", json!({"cdp": tab}));
    assert_eq!(observed["snapshot"], 1);
    assert!(observed["regions"].as_array().unwrap().len() >= 2);

    let guarded = call(
        &mut server,
        "guard",
        json!({"cdp": tab, "target": "Sign in", "role": "button"}),
    );
    assert_eq!(guarded["decision"], "allow", "{guarded}");
    assert_eq!(guarded["executed"], false);
    assert_eq!(guarded["target"]["label"], "Sign in");
    assert!(
        guarded.get("ticket").is_some(),
        "Allow must issue ActionTicket: {guarded}"
    );
    assert_eq!(guarded["ticket"]["target_label"], "Sign in");
    assert_eq!(guarded["ticket"]["action"], "click");
    assert!(guarded["ticket"]["ticket_id"].as_u64().unwrap() >= 1);
    assert_eq!(mock.presses(), 0);
}

#[test]
fn guard_escalates_identical_twin_sends() {
    let tab = "ws://mock/twins";
    let mock = MockChrome::default();
    mock.tab(tab, ScriptBuilder::new().observe(&twins_page()));
    let mut server = mock.server();

    let guarded = call(
        &mut server,
        "guard",
        json!({"cdp": tab, "target": "Send", "role": "button"}),
    );
    assert_eq!(guarded["decision"], "escalate", "{guarded}");
    assert_eq!(guarded["reason"], "ambiguous");
    assert_eq!(guarded["executed"], false);
    assert!(guarded["candidates"].as_array().unwrap().len() >= 2);
    assert_eq!(mock.presses(), 0);
}

#[test]
fn guard_refuses_a_text_miss() {
    let tab = "ws://mock/miss";
    let mock = MockChrome::default();
    mock.tab(tab, ScriptBuilder::new().observe(&sign_in_page()));
    let mut server = mock.server();

    let guarded = call(
        &mut server,
        "guard",
        json!({"cdp": tab, "target": "Delete forever", "role": "button"}),
    );
    assert_eq!(guarded["decision"], "refuse", "{guarded}");
    assert_eq!(guarded["executed"], false);
    assert_eq!(mock.presses(), 0);
}

#[test]
fn deprecated_act_is_an_alias_of_guard_and_never_clicks() {
    let tab = "ws://mock/act-alias";
    let mock = MockChrome::default();
    mock.tab(tab, ScriptBuilder::new().observe(&sign_in_page()));
    let mut server = mock.server();

    let acted = call(
        &mut server,
        "act",
        json!({"cdp": tab, "target": "Sign in", "role": "button"}),
    );
    assert_eq!(acted["tool"], "guard");
    assert_eq!(acted["decision"], "allow");
    assert_eq!(acted["executed"], false);
    assert_eq!(mock.presses(), 0);
}

#[test]
fn verify_checks_expected_text_without_clicking() {
    let tab = "ws://mock/verify";
    let mock = MockChrome::default();
    mock.tab(tab, ScriptBuilder::new().observe(&sign_in_page()));
    let mut server = mock.server();

    let verified = call(
        &mut server,
        "verify",
        json!({"cdp": tab, "expect_text": "Sign in"}),
    );
    assert_eq!(verified["verified"], true);
    assert_eq!(mock.presses(), 0);
}

#[test]
fn session_ring_observe_then_diff() {
    let tab = "ws://mock/diff";
    let mock = MockChrome::default();
    mock.tab(
        tab,
        ScriptBuilder::new()
            .observe(&sign_in_page())
            .observe(&twins_page()),
    );
    let mut server = mock.server();

    let first = call(&mut server, "observe", json!({"cdp": tab}));
    let second = call(&mut server, "observe", json!({"cdp": tab}));
    let delta = call(
        &mut server,
        "diff",
        json!({
            "before_snapshot": first["snapshot"],
            "after_snapshot": second["snapshot"],
        }),
    );
    assert!(delta["state_delta"]["added"].as_array().is_some());
    assert_eq!(mock.presses(), 0);
}
