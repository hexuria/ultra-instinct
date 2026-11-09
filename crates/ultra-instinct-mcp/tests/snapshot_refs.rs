//! Presses bound to what the caller saw (`snapshot` from browser_get_state).
//!
//! A page that shifts its elements between get_state and the press would
//! make a bare index name another element. With `snapshot`, the index
//! resolves in that snapshot, the ticket binds that region, and the press
//! refuses `stale_ref` with no input sent. Each test pins the CDP calls.

use std::cell::RefCell;
use std::rc::Rc;

use serde_json::{json, Value};
use ultra_instinct_browser::script::{Control, PageSpec, ScriptBuilder};
use ultra_instinct_browser::{CdpError, CdpTransport, ReplayTransport};
use ultra_instinct_mcp::{Server, ToolError};

const CDP: &str = "http://127.0.0.1:9222";

type Log = Rc<RefCell<Vec<String>>>;

struct Logged {
    inner: ReplayTransport,
    log: Log,
}

impl CdpTransport for Logged {
    fn call(&mut self, method: &str, params_json: &str) -> Result<String, CdpError> {
        self.log.borrow_mut().push(method.to_owned());
        self.inner.call(method, params_json)
    }
}

fn replay(script: ScriptBuilder) -> (Server, Log) {
    let log: Log = Rc::new(RefCell::new(Vec::new()));
    let shared = Rc::clone(&log);
    let mut script = Some(script);
    let server = Server::with_connector(move |_url| {
        let script = script.take().ok_or_else(|| CdpError::Transport {
            message: "one connect per test".into(),
        })?;
        Ok(Box::new(Logged {
            inner: ReplayTransport::parse(&script.to_json())?,
            log: Rc::clone(&shared),
        }) as Box<dyn CdpTransport>)
    });
    (server, log)
}

fn call(server: &mut Server, tool: &str, arguments: Value) -> Value {
    server
        .call_tool(tool, &arguments)
        .unwrap_or_else(|err| panic!("{tool} {arguments}: {err:?}"))
}

/// Input that reached the page: DOM clicks / inputs and mouse events.
fn presses(log: &Log) -> Vec<String> {
    log.borrow()
        .iter()
        .filter(|m| {
            matches!(
                m.as_str(),
                "DOM.resolveNode" | "Runtime.callFunctionOn" | "Input.dispatchMouseEvent"
            )
        })
        .cloned()
        .collect()
}

/// Email @e1 (y 20), Sign in @e2 (y 80).
fn page() -> PageSpec {
    PageSpec::of(
        &[
            Control::text_field(10, 100, "Email", (20.0, 20.0, 300.0, 28.0)),
            Control::button(11, 101, "Sign in", (20.0, 80.0, 120.0, 28.0)),
        ],
        "http://127.0.0.1/form",
        "Form",
    )
}

/// A banner button lands on top and pushes everything down: @e2 is now
/// Email, and Sign in moved to @e3.
fn shifted() -> PageSpec {
    PageSpec::of(
        &[
            Control::button(13, 103, "Delete account", (20.0, 20.0, 160.0, 28.0)),
            Control::text_field(10, 100, "Email", (20.0, 60.0, 300.0, 28.0)),
            Control::button(11, 101, "Sign in", (20.0, 120.0, 120.0, 28.0)),
        ],
        "http://127.0.0.1/form",
        "Form",
    )
}

fn snapshot_of(state: &Value) -> u64 {
    state["snapshot"]
        .as_u64()
        .expect("get_state returns a snapshot id")
}

#[test]
fn get_state_returns_one_snapshot_id_in_both_formats() {
    let (mut server, _log) = replay(
        ScriptBuilder::new()
            .observe_compact(&page())
            .observe_compact(&page()),
    );
    let text = call(&mut server, "browser_get_state", json!({"cdp": CDP}));
    let id = snapshot_of(&text);
    assert!(
        text["text"]
            .as_str()
            .unwrap()
            .ends_with(&format!("snapshot {id}")),
        "{text}"
    );
    let json_state = call(
        &mut server,
        "browser_get_state",
        json!({"cdp": CDP, "format": "json"}),
    );
    assert_eq!(snapshot_of(&json_state), id + 1, "{json_state}");
}

/// The bug a bare index has: the page shifted, so index 2 presses Email.
/// The same press bound to the snapshot refuses and sends no input.
#[test]
fn a_shifted_page_refuses_a_snapshot_bound_click_with_no_press() {
    let (mut server, log) = replay(
        ScriptBuilder::new()
            .observe_compact(&page())
            .observe_compact(&shifted())
            .observe_compact(&shifted()),
    );
    let state = call(&mut server, "browser_get_state", json!({"cdp": CDP}));
    let snapshot = snapshot_of(&state);
    let err = server
        .call_tool(
            "browser_click",
            &json!({"cdp": CDP, "index": "@e2", "snapshot": snapshot}),
        )
        .expect_err("Sign in moved");
    assert!(matches!(err, ToolError::StaleRef { .. }), "{err:?}");
    assert_eq!(err.to_value()["code"], "stale_ref");
    assert!(presses(&log).is_empty(), "{:?}", log.borrow());

    // Without the snapshot the same index names another element now.
    let (mut bare, log) = replay(
        ScriptBuilder::new()
            .observe_compact(&shifted())
            .observe_compact(&shifted())
            .dom_click(10),
    );
    let out = call(
        &mut bare,
        "browser_click",
        json!({"cdp": CDP, "index": "@e2"}),
    );
    assert_eq!(out["region"], "n100", "bare index 2 is Email now: {out}");
    assert_eq!(presses(&log).len(), 2);
}

#[test]
fn an_unchanged_page_presses_the_snapshot_region() {
    let (mut server, log) = replay(
        ScriptBuilder::new()
            .observe_compact(&page())
            .observe_compact(&page())
            .observe_compact(&page())
            .dom_click(11),
    );
    let state = call(&mut server, "browser_get_state", json!({"cdp": CDP}));
    let out = call(
        &mut server,
        "browser_click",
        json!({"cdp": CDP, "index": 2, "snapshot": snapshot_of(&state)}),
    );
    assert_eq!(out["region"], "n101", "{out}");
    assert_eq!(
        presses(&log),
        ["DOM.resolveNode", "Runtime.callFunctionOn"],
        "{:?}",
        log.borrow()
    );
}

/// The consume_ticket_once path (double click) and hover refuse the same
/// way: revalidated against the fresh observation before any mouse event.
#[test]
fn double_click_and_hover_on_a_shifted_page_refuse_with_no_input() {
    for tool in ["browser_double_click", "browser_hover"] {
        let (mut server, log) = replay(
            ScriptBuilder::new()
                .observe_compact(&page())
                .observe_compact(&shifted()),
        );
        let state = call(&mut server, "browser_get_state", json!({"cdp": CDP}));
        let err = server
            .call_tool(
                tool,
                &json!({"cdp": CDP, "index": 2, "snapshot": snapshot_of(&state)}),
            )
            .expect_err("moved");
        assert_eq!(err.code(), "stale_ref", "{tool}: {err:?}");
        assert!(presses(&log).is_empty(), "{tool}: {:?}", log.borrow());
    }
}

/// Indexes resolve in the snapshot: an index the snapshot never had is
/// unknown_ref, even when the page has that many elements now.
#[test]
fn an_index_past_the_snapshot_is_unknown_ref() {
    let (mut server, log) = replay(
        ScriptBuilder::new()
            .observe_compact(&page())
            .observe_compact(&shifted()),
    );
    let state = call(&mut server, "browser_get_state", json!({"cdp": CDP}));
    let err = server
        .call_tool(
            "browser_click",
            &json!({"cdp": CDP, "index": 3, "snapshot": snapshot_of(&state)}),
        )
        .expect_err("the snapshot had two");
    assert_eq!(
        err,
        ToolError::UnknownRef {
            index: 3,
            indexed: 2
        }
    );
    assert!(presses(&log).is_empty());
    let err = server
        .call_tool(
            "browser_click",
            &json!({"cdp": CDP, "index": 1, "snapshot": 999}),
        )
        .expect_err("never recorded");
    assert_eq!(err, ToolError::UnknownSnapshot(999));
}

#[test]
fn require_snapshot_refuses_a_bare_index_before_any_cdp_call() {
    let (server_, log) = replay(
        ScriptBuilder::new()
            .observe_compact(&page())
            .observe_compact(&page())
            .observe_compact(&page())
            .dom_click(11),
    );
    let mut server = server_.require_snapshot(true);
    for (tool, arguments) in [
        ("browser_click", json!({"cdp": CDP, "index": 2})),
        ("browser_type", json!({"cdp": CDP, "index": 1, "text": "a"})),
        ("browser_right_click", json!({"cdp": CDP, "index": 2})),
        ("browser_hover", json!({"cdp": CDP, "index": 2})),
        (
            "browser_drag",
            json!({"cdp": CDP, "from_index": 1, "to_index": 2}),
        ),
        (
            "browser_check",
            json!({"cdp": CDP, "index": 2, "checked": true}),
        ),
        ("browser_focus", json!({"cdp": CDP, "index": 1})),
        (
            "browser_send_key",
            json!({"cdp": CDP, "index": 1, "key": "Enter"}),
        ),
    ] {
        let err = server.call_tool(tool, &arguments).expect_err(tool);
        assert_eq!(err, ToolError::SnapshotRequired, "{tool}");
        assert_eq!(err.to_value()["code"], "snapshot_required");
    }
    assert!(
        log.borrow().iter().all(|m| m == "Page.enable"),
        "only dialog tracking ran: {:?}",
        log.borrow()
    );
    let state = call(&mut server, "browser_get_state", json!({"cdp": CDP}));
    let out = call(
        &mut server,
        "browser_click",
        json!({"cdp": CDP, "index": 2, "snapshot": snapshot_of(&state)}),
    );
    assert_eq!(out["region"], "n101", "{out}");
}

/// get_state shows the first `max_elements` controls only: an index past
/// them, read in that snapshot, names nothing the caller was shown and
/// refuses `unknown_ref` with nothing pressed.
#[test]
fn a_snapshot_press_past_the_shown_elements_is_unknown() {
    let controls: Vec<Control> = (0..1200)
        .map(|i| {
            let id = 10 + i64::from(i);
            let y = 20.0 + f64::from(i % 20) * 30.0;
            let x = 20.0 + f64::from(i / 20) * 10.0;
            Control::button(id, 1000 + id, &format!("Item {i}"), (x, y, 8.0, 20.0))
        })
        .collect();
    let many = PageSpec::of(&controls, "http://127.0.0.1/list", "List");
    let (mut server, log) = replay(
        ScriptBuilder::new()
            .observe_compact(&many)
            .observe_compact(&many),
    );
    let state = call(
        &mut server,
        "browser_get_state",
        json!({"cdp": CDP, "format": "json"}),
    );
    assert_eq!(
        state["interactive_elements"], 1000,
        "{}",
        state["dropped_elements"]
    );
    assert_eq!(state["dropped_elements"], 200);
    let err = server
        .call_tool(
            "browser_click",
            &json!({"cdp": CDP, "index": 1150, "snapshot": snapshot_of(&state)}),
        )
        .unwrap_err();
    assert_eq!(err.code(), "unknown_ref", "{err:?}");
    assert_eq!(
        err.to_value()["indexed"],
        1000,
        "the caller was shown 1000: {err:?}"
    );
    assert!(presses(&log).is_empty(), "{:?}", presses(&log));
}

/// A drag's drop target reads the same shown prefix as its source: with
/// one element shown, `to_index: 2` names nothing the caller saw and
/// refuses `unknown_ref` with nothing pressed.
#[test]
fn a_drag_onto_an_element_past_the_shown_ones_is_unknown() {
    let (server, log) = replay(
        ScriptBuilder::new()
            .observe_compact(&page())
            .observe_compact(&page()),
    );
    let mut server = server.with_limits(ultra_instinct_mcp::ToolLimits {
        max_elements: 1,
        ..ultra_instinct_mcp::ToolLimits::default()
    });
    let state = call(
        &mut server,
        "browser_get_state",
        json!({"cdp": CDP, "format": "json"}),
    );
    assert_eq!(state["dropped_elements"], 1, "{state}");
    let err = server
        .call_tool(
            "browser_drag",
            &json!({"cdp": CDP, "from_index": 1, "to_index": 2, "snapshot": snapshot_of(&state)}),
        )
        .unwrap_err();
    assert_eq!(
        err,
        ToolError::UnknownRef {
            index: 2,
            indexed: 1
        }
    );
    assert!(presses(&log).is_empty(), "{:?}", log.borrow());
}
