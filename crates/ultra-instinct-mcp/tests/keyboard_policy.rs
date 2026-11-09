//! Input with no target (`browser_keyboard`, `browser_send_key` and
//! `browser_hold_key` without an index) runs under the guard `Policy` the
//! way presses do: the page host is checked on an observation taken now,
//! before any `Input.*` call, and an Enter / Space on a focused control
//! whose label needs a confirm is that press.

use std::cell::RefCell;
use std::rc::Rc;

use serde_json::{json, Value};
use ultra_instinct_browser::script::{Control, HistorySpec, PageSpec, ScriptBuilder};
use ultra_instinct_browser::{CdpError, CdpTransport, ReplayTransport};
use ultra_instinct_mcp::{Policy, Server, ToolError};

const CDP: &str = "http://127.0.0.1:9222";

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

fn host(script: ScriptBuilder, policy: Policy) -> (Server, Rc<RefCell<Vec<String>>>) {
    let log = Rc::new(RefCell::new(Vec::new()));
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
    })
    .with_policy(policy);
    (server, log)
}

fn policy(allow: &str, confirm: &str) -> Policy {
    Policy::from_lists(allow, confirm).unwrap()
}

fn inputs(log: &Rc<RefCell<Vec<String>>>) -> usize {
    log.borrow()
        .iter()
        .filter(|m| m.starts_with("Input."))
        .count()
}

/// A focused text field "Note" and a button "Pay $18" on `url`; `pay`
/// focuses the button instead.
fn page(url: &str, pay: bool) -> PageSpec {
    let note = Control::text_field(10, 100, "Note", (20.0, 20.0, 300.0, 28.0));
    let button = Control::button(11, 101, "Pay $18", (20.0, 80.0, 120.0, 28.0));
    let (note, button) = if pay {
        (note, button.focused())
    } else {
        (note.focused(), button)
    };
    PageSpec::of(&[note, button], url, "Checkout")
}

fn focused_true() -> Value {
    json!({"result": {"type": "boolean", "value": true}})
}

fn keys(script: ScriptBuilder, n: usize) -> ScriptBuilder {
    (0..n).fold(script, |s, _| s.call("Input.dispatchKeyEvent", json!({})))
}

/// The three tools, each with its argument and the CDP replies it needs
/// after the observation once the policy lets it through.
type Rest = fn(ScriptBuilder) -> ScriptBuilder;

fn tools() -> [(&'static str, Value, Rest); 3] {
    [
        ("browser_keyboard", json!({"cdp": CDP, "text": "hi"}), |s| {
            keys(s.call("Runtime.evaluate", focused_true()), 4)
        }),
        ("browser_send_key", json!({"cdp": CDP, "key": "a"}), |s| {
            keys(s, 2)
        }),
        (
            "browser_hold_key",
            json!({"cdp": CDP, "key": "a", "hold_ms": 1}),
            |s| keys(s, 2),
        ),
    ]
}

#[test]
fn off_the_allow_list_each_tool_refuses_before_any_input() {
    for (tool, arguments, _) in tools() {
        let script = ScriptBuilder::new().observe_compact(&page("https://evil.test/x", false));
        let (mut server, log) = host(script, policy("127.0.0.1", ""));
        let err = server.call_tool(tool, &arguments).expect_err(tool);
        assert_eq!(
            err,
            ToolError::DomainNotAllowed {
                host: "evil.test".into()
            },
            "{tool}"
        );
        assert_eq!(err.to_value()["code"], "domain_not_allowed");
        assert_eq!(inputs(&log), 0, "{tool}: {:?}", log.borrow());
    }
}

#[test]
fn on_the_allow_list_and_on_about_blank_each_tool_sends_its_input() {
    for url in ["http://127.0.0.1/form", "about:blank"] {
        for (tool, arguments, rest) in tools() {
            let script = rest(ScriptBuilder::new().observe_compact(&page(url, false)));
            let (mut server, log) = host(script, policy("127.0.0.1", ""));
            let out = server
                .call_tool(tool, &arguments)
                .unwrap_or_else(|err| panic!("{tool} on {url}: {err:?}"));
            assert!(out.is_object(), "{out}");
            assert!(inputs(&log) >= 2, "{tool} on {url}: {:?}", log.borrow());
        }
    }
}

#[test]
fn an_unreadable_url_fails_closed_while_the_list_is_set() {
    for (tool, arguments, _) in tools() {
        let mut page = page("http://127.0.0.1/form", false);
        page.history = HistorySpec::NoEntries;
        let (mut server, log) = host(
            ScriptBuilder::new().observe_compact(&page),
            policy("127.0.0.1", ""),
        );
        let err = server.call_tool(tool, &arguments).expect_err(tool);
        assert_eq!(
            err,
            ToolError::TicketInvalid("domain-not-allowed".into()),
            "{tool}"
        );
        assert_eq!(inputs(&log), 0, "{tool}");
    }
}

/// Enter / Space on a focused "Pay $18" button is that press: it needs the
/// same `confirm` a click does. Other keys, and text in a field, do not.
#[test]
fn enter_or_space_on_a_focused_confirm_label_needs_confirm() {
    let pay = page("http://127.0.0.1/form", true);
    let cases = [
        ("browser_send_key", json!({"cdp": CDP, "key": "Enter"})),
        (
            "browser_hold_key",
            json!({"cdp": CDP, "key": " ", "hold_ms": 1}),
        ),
        ("browser_keyboard", json!({"cdp": CDP, "text": "\n"})),
    ];
    for (tool, arguments) in cases {
        let (mut server, log) = host(
            ScriptBuilder::new().observe_compact(&pay),
            policy("", "pay*"),
        );
        let err = server.call_tool(tool, &arguments).expect_err(tool);
        assert_eq!(
            err,
            ToolError::NeedsConfirm {
                label: "Pay $18".into()
            },
            "{tool}"
        );
        assert_eq!(inputs(&log), 0, "{tool}");
    }
    // `confirm` naming the label lets this call press it.
    let script = keys(ScriptBuilder::new().observe_compact(&pay), 2);
    let (mut server, log) = host(script, policy("", "pay*"));
    let out = server
        .call_tool(
            "browser_send_key",
            &json!({"cdp": CDP, "key": "Enter", "confirm": "pay $18"}),
        )
        .unwrap();
    assert_eq!(out["key"], "Enter");
    assert_eq!(inputs(&log), 2);
    // Another key on the button is no press.
    let script = keys(ScriptBuilder::new().observe_compact(&pay), 2);
    let (mut server, _log) = host(script, policy("", "pay*"));
    server
        .call_tool("browser_send_key", &json!({"cdp": CDP, "key": "Escape"}))
        .unwrap();
    // A space typed into a field labelled like a confirm is text, not a press.
    let mut field = page("http://127.0.0.1/form", false);
    field.dom[0] = field.dom[0].clone().with_compact_state(json!({"v": ""}));
    let script = keys(
        ScriptBuilder::new()
            .observe_compact(&field)
            .call("Runtime.evaluate", focused_true()),
        6,
    );
    let (mut server, _log) = host(script, policy("", "note*"));
    server
        .call_tool("browser_keyboard", &json!({"cdp": CDP, "text": "a b"}))
        .unwrap();
}

/// A Tab moves the focus to a control not observed yet: with confirm
/// patterns set, an Enter / Space after it must go in its own call.
#[test]
fn a_press_after_a_tab_must_be_its_own_call_under_confirm_patterns() {
    let (mut server, log) = host(ScriptBuilder::new(), policy("", "pay*"));
    let err = server
        .call_tool("browser_keyboard", &json!({"cdp": CDP, "text": "x\t\n"}))
        .expect_err("tab then enter");
    assert!(matches!(err, ToolError::InvalidArguments(_)), "{err:?}");
    // Only the Page domain every browser_* call enables: no observe, no input.
    assert_eq!(*log.borrow(), ["Page.enable"]);
}

/// A ticketed KEY (`browser_send_key` with an index) runs the guard's same
/// press rule: Enter on a button-like control that takes keys (a slider
/// handle labelled "Pay $18"), or Enter in a field whose form submits with
/// "Pay $18", refuses `needs_confirm` naming that label with zero `Input.*`
/// calls; with `confirm` it is sent.
#[test]
fn a_ticketed_enter_that_presses_a_confirm_label_needs_confirm() {
    let handle = Control::button(11, 101, "Pay $18", (20.0, 80.0, 120.0, 28.0));
    let field = Control::text_field(10, 100, "Promo code", (20.0, 20.0, 300.0, 28.0));
    // @e1 the field (in a form that submits with Pay), @e2 the handle.
    let mut page = PageSpec::of(&[field, handle], "http://127.0.0.1/pay", "Pay");
    page.dom[0] = page.dom[0]
        .clone()
        .with_compact_state(json!({"it": "text", "fm": 0}));
    page.forms = vec![json!("Pay $18")];
    page.dom[1] = page.dom[1].clone().with_attr("class", "ui-slider-handle");
    for (index, node) in [(1, 10), (2, 11)] {
        let script = ScriptBuilder::new()
            .observe_compact(&page)
            .observe_compact(&page)
            .observe_compact(&page)
            .observe_compact(&page)
            .dom_input(node)
            .call("Input.dispatchKeyEvent", json!({}))
            .call("Input.dispatchKeyEvent", json!({}));
        let (mut server, log) = host(script, policy("", "pay*"));
        let state = server
            .call_tool("browser_get_state", &json!({"cdp": CDP}))
            .unwrap();
        let snapshot = state["snapshot"].clone();
        let press = json!({"cdp": CDP, "index": index, "key": "Enter", "snapshot": snapshot});
        let err = server
            .call_tool("browser_send_key", &press)
            .expect_err("confirm");
        assert_eq!(
            err,
            ToolError::NeedsConfirm {
                label: "Pay $18".into()
            },
            "@e{index}"
        );
        assert_eq!(inputs(&log), 0, "@e{index}: {:?}", log.borrow());
        let mut confirmed = press.clone();
        confirmed["confirm"] = json!("Pay $18");
        let out = server
            .call_tool("browser_send_key", &confirmed)
            .unwrap_or_else(|err| panic!("@e{index}: {err:?} {:?}", log.borrow()));
        assert_eq!(out["acted"], "key", "{out}");
        assert_eq!(inputs(&log), 2, "@e{index}");
    }
}

/// Splitting a Tab and an Enter across two calls does not get around the
/// same-call refusal: when the observation names no focused control (focus
/// in a cross-origin frame, say), an activating key refuses `needs_confirm`
/// naming `(unobserved focus)`, with no `Input.*` call, until that exact
/// label is confirmed.
#[test]
fn an_activating_key_with_no_observed_focus_needs_confirm() {
    let unfocused = PageSpec::of(
        &[Control::button(
            11,
            101,
            "Pay $18",
            (20.0, 80.0, 120.0, 28.0),
        )],
        "http://127.0.0.1/form",
        "Checkout",
    );
    let script = keys(ScriptBuilder::new().observe_compact(&unfocused), 2)
        .observe_compact(&unfocused)
        .observe_compact(&unfocused);
    let script = keys(script, 2);
    let (mut server, log) = host(script, policy("", "pay*"));
    server
        .call_tool("browser_send_key", &json!({"cdp": CDP, "key": "Tab"}))
        .expect("tab presses nothing");
    assert_eq!(inputs(&log), 2);
    let enter = json!({"cdp": CDP, "key": "Enter"});
    let err = server
        .call_tool("browser_send_key", &enter)
        .expect_err("enter");
    assert_eq!(
        err,
        ToolError::NeedsConfirm {
            label: "(unobserved focus)".into()
        }
    );
    assert_eq!(
        inputs(&log),
        2,
        "no input for the Enter: {:?}",
        log.borrow()
    );
    let mut confirmed = enter.clone();
    confirmed["confirm"] = json!("(unobserved focus)");
    server.call_tool("browser_send_key", &confirmed).unwrap();
    assert_eq!(inputs(&log), 4);
}

/// A control that is no form control (a `div` combobox walking its
/// options) submits no form: observe reads it as in no form, so Enter on it
/// needs no confirm even with confirm patterns set.
#[test]
fn enter_on_a_non_form_control_needs_no_confirm() {
    let combo = Control {
        tag: "DIV",
        role: "combobox",
        ..Control::button(10, 100, "Country", (20.0, 20.0, 200.0, 28.0))
    };
    let page = PageSpec::of(&[combo], "http://127.0.0.1/pick", "Pick");
    let script = ScriptBuilder::new()
        .observe_compact(&page)
        .observe_compact(&page)
        .observe_compact(&page)
        .dom_input(10);
    let (mut server, log) = host(keys(script, 2), policy("", "pay*"));
    let state = server
        .call_tool("browser_get_state", &json!({"cdp": CDP}))
        .unwrap();
    let out = server
        .call_tool(
            "browser_send_key",
            &json!({"cdp": CDP, "index": 1, "key": "Enter", "snapshot": state["snapshot"]}),
        )
        .unwrap_or_else(|err| panic!("{err:?}"));
    assert_eq!(out["acted"], "key", "{out}");
    assert_eq!(inputs(&log), 2);
}

/// The executor's own fresh look decides the label a refusal names: the
/// caller saw a form that submits with "Continue", the page now submits
/// with "Pay $18", and the refusal names "Pay $18" (confirming "Continue"
/// would only loop).
#[test]
fn a_refusal_names_the_label_on_the_executors_fresh_look() {
    let field = Control::text_field(10, 100, "Promo code", (20.0, 20.0, 300.0, 28.0));
    let with_submit = |label: &str| {
        let mut page = PageSpec::of(std::slice::from_ref(&field), "http://127.0.0.1/pay", "Pay");
        page.dom[0] = page.dom[0]
            .clone()
            .with_compact_state(json!({"it": "text", "fm": 0}));
        page.forms = vec![json!(label)];
        page
    };
    let script = ScriptBuilder::new()
        .observe_compact(&with_submit("Continue"))
        .observe_compact(&with_submit("Continue"))
        .observe_compact(&with_submit("Pay $18"));
    let (mut server, log) = host(script, policy("", "pay*"));
    let state = server
        .call_tool("browser_get_state", &json!({"cdp": CDP}))
        .unwrap();
    let err = server
        .call_tool(
            "browser_send_key",
            &json!({"cdp": CDP, "index": 1, "key": "Enter", "snapshot": state["snapshot"]}),
        )
        .expect_err("needs confirm");
    assert_eq!(
        err,
        ToolError::NeedsConfirm {
            label: "Pay $18".into()
        }
    );
    assert_eq!(inputs(&log), 0);
}

/// A Space after a Tab is ordinary text unless it is the first thing after
/// the Tab; an Enter after a Tab still refuses; insert mode sends no keys,
/// so nothing in it presses.
#[test]
fn only_an_enter_or_a_leading_space_after_a_tab_refuses() {
    let typed = |text: &str| {
        let chars = text.chars().count();
        let script = keys(
            ScriptBuilder::new()
                .observe_compact(&page("http://127.0.0.1/form", false))
                .call("Runtime.evaluate", focused_true()),
            chars * 2,
        );
        let (mut server, _log) = host(script, policy("", "pay*"));
        server.call_tool("browser_keyboard", &json!({"cdp": CDP, "text": text}))
    };
    typed("Jane\tDoe Smith").expect("form fill");
    for text in ["x\t\n", "x\t y"] {
        let err = typed(text).expect_err(text);
        assert!(
            matches!(err, ToolError::InvalidArguments(_)),
            "{text}: {err:?}"
        );
    }
    let script = ScriptBuilder::new()
        .observe_compact(&page("http://127.0.0.1/form", false))
        .call("Runtime.evaluate", focused_true())
        .call("Input.insertText", json!({}));
    let (mut server, log) = host(script, policy("", "pay*"));
    server
        .call_tool(
            "browser_keyboard",
            &json!({"cdp": CDP, "text": "x\t\n", "mode": "insert"}),
        )
        .expect("insert sends no keys");
    assert_eq!(inputs(&log), 1);
}
