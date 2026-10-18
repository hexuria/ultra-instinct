//! Mock-CDP e2e: the real `BrowserSession` over a `ReplayTransport` driven by
//! the agent. Every CDP call the loop makes must be scripted in order, so these
//! tests pin the exact wire sequence of observe → ticket → input → observe.

use serde_json::{json, Value};
use ultra_instinct_agent::{AgentBuilder, AgentError, AgentOutcome, VerificationKind};
use ultra_instinct_browser::script::{Control, PageSpec, ScriptBuilder};
use ultra_instinct_browser::{
    BrowserSession, ReplayTransport, DOM_SELECT_FUNCTION, DOM_TYPE_FUNCTION,
    SCROLL_VIEWPORT_FRACTION,
};
use ultra_instinct_core::{ActionKind, ActionSpace};
use ultra_instinct_policy::{
    AgentGoal, BrowserPolicy, HistoryEntry, InstinctPolicy, PolicyDecision, PolicyError,
    PolicyOutcome, TextContext, TextError, TextResolution, TextResolver,
};

fn session(script: ScriptBuilder) -> BrowserSession<ReplayTransport> {
    BrowserSession::new(ReplayTransport::parse(&script.to_json()).unwrap())
}

#[derive(Default)]
struct SelectThenDone;

impl BrowserPolicy for SelectThenDone {
    fn decide(
        &mut self,
        space: &ActionSpace,
        _goal: &AgentGoal,
        history: &[HistoryEntry],
    ) -> Result<PolicyOutcome, PolicyError> {
        let action = if history.is_empty() {
            space.targets_of(ActionKind::Select).next()
        } else {
            space.get_str(ActionKind::Done.as_str())
        };
        let Some(action) = action else {
            return Ok(PolicyOutcome::Abstain {
                reason: "select replay action unavailable".into(),
                operation_ranked: Vec::new(),
                target_ranked: Vec::new(),
            });
        };
        Ok(PolicyOutcome::Choice(PolicyDecision {
            action_id: action.id().clone(),
            kind: action.kind(),
            target_label: action.label().to_owned(),
            confidence_millis: 1_000,
            operation_ranked: Vec::new(),
            target_ranked: Vec::new(),
        }))
    }
}

struct AbstainingTextResolver;

impl TextResolver for AbstainingTextResolver {
    fn resolve(&mut self, _context: &TextContext) -> Result<TextResolution, TextError> {
        Err(TextError::Abstain("no resolver value".into()))
    }
}

fn search_page() -> PageSpec {
    PageSpec::of(
        &[
            Control::text_field(10, 100, "Search", (20.0, 20.0, 300.0, 28.0)),
            Control::button(11, 101, "Go", (340.0, 20.0, 60.0, 28.0)),
        ],
        "http://127.0.0.1/search",
        "Search",
    )
}

fn calls_of(log: &[(String, String)], method: &str) -> Vec<Value> {
    log.iter()
        .filter(|(m, _)| m == method)
        .map(|(_, p)| serde_json::from_str(p).unwrap())
        .collect()
}

fn autocomplete_poll_calls(log: &[(String, String)]) -> Vec<Value> {
    calls_of(log, "Runtime.callFunctionOn")
        .into_iter()
        .filter(|call| {
            call["functionDeclaration"]
                .as_str()
                .is_some_and(|function| function.contains("aria-controls"))
        })
        .collect()
}

fn autocomplete_page(with_option: bool, picked: bool) -> PageSpec {
    autocomplete_page_scoped(with_option, picked, true)
}

fn document_wide_autocomplete_page(with_option: bool, picked: bool) -> PageSpec {
    autocomplete_page_scoped(with_option, picked, false)
}

fn autocomplete_page_scoped(with_option: bool, picked: bool, with_controls: bool) -> PageSpec {
    let mut controls = vec![Control::combobox(
        10,
        100,
        "City",
        (10.0, 10.0, 200.0, 28.0),
    )];
    if with_option {
        controls.push(Control::option(
            11,
            110,
            "Manila",
            (10.0, 40.0, 200.0, 28.0),
        ));
    }
    if picked {
        controls.push(Control::button(12, 120, "picked", (10.0, 80.0, 80.0, 24.0)));
    }
    let mut page = PageSpec::of(&controls, "http://127.0.0.1/auto", "Auto");
    if with_controls {
        page.dom[0]
            .attributes
            .push(("aria-controls".to_owned(), "suggestions".to_owned()));
    }
    page
}

#[test]
fn type_text_over_cdp_sends_argument_not_source_and_verifies_value() {
    let page = search_page();
    let script = ScriptBuilder::new()
        .observe(&page) // predict
        .observe(&page) // executor revalidation
        .dom_input(10) // DOM.resolveNode + Runtime.callFunctionOn(type)
        .observe(&page) // after
        .dom_read_value(10, "rust ownership", "") // value read-back
        .observe(&page); // next predict → policy: satisfied → DONE
    let mut agent = AgentBuilder::new(session(script), InstinctPolicy::default())
        .max_steps(4)
        .build(r#"Type "rust ownership" into Search"#);
    let outcome = agent.run();
    assert!(matches!(outcome, AgentOutcome::Done { .. }), "{outcome:?}");
    let step = &outcome.steps()[0];
    assert_eq!(step.kind, ActionKind::TypeText);
    assert_eq!(step.verification, VerificationKind::Success);

    let transport = agent.browser_mut().transport();
    assert_eq!(transport.remaining(), 0, "every scripted call consumed");
    let fns = calls_of(transport.logged_calls(), "Runtime.callFunctionOn");
    let typed = &fns[0];
    assert_eq!(typed["functionDeclaration"], DOM_TYPE_FUNCTION);
    assert_eq!(typed["arguments"][0]["value"], "rust ownership");
    assert!(!DOM_TYPE_FUNCTION.contains("rust"));
    // Bound to the observed node, never coordinates.
    let resolves = calls_of(transport.logged_calls(), "DOM.resolveNode");
    assert_eq!(resolves[0]["nodeId"], 10);
    assert!(calls_of(transport.logged_calls(), "Input.dispatchMouseEvent").is_empty());
}

#[test]
fn readonly_field_rejection_over_cdp_types_nothing() {
    let page = search_page();
    let script = ScriptBuilder::new()
        .observe(&page)
        .observe(&page)
        .dom_input_rejected(10, "readonly");
    let mut agent = AgentBuilder::new(session(script), InstinctPolicy::default())
        .max_steps(2)
        .build(r#"Type "x" into Search"#);
    agent.predict().unwrap();
    let err = agent.act().unwrap_err();
    assert!(
        matches!(err, AgentError::InputRejected(ref m) if m.contains("readonly")),
        "{err}"
    );
}

#[test]
fn stale_rerender_over_cdp_never_reaches_input() {
    let page = search_page();
    // Nearby peer (not a far banner): target-scoped world must go stale.
    let changed = PageSpec::of(
        &[
            Control::text_field(10, 100, "Search", (20.0, 20.0, 300.0, 28.0)),
            Control::button(11, 101, "Go", (340.0, 20.0, 60.0, 28.0)),
            Control::button(12, 102, "Accept cookies", (420.0, 20.0, 160.0, 28.0)),
        ],
        "http://127.0.0.1/search",
        "Search",
    );
    let script = ScriptBuilder::new().observe(&page).observe(&changed);
    let mut agent = AgentBuilder::new(session(script), InstinctPolicy::default())
        .max_steps(2)
        .build("Go");
    agent.predict().unwrap().expect("click Go");
    let err = agent.act().unwrap_err();
    assert!(matches!(err, AgentError::Stale(_)), "{err}");
    let transport = agent.browser_mut().transport();
    assert!(calls_of(transport.logged_calls(), "Runtime.callFunctionOn").is_empty());
    assert!(calls_of(transport.logged_calls(), "Input.dispatchMouseEvent").is_empty());
}

#[test]
fn covered_button_over_cdp_is_not_offered_or_pressed() {
    // The search field's node sits over "Go"'s center (hit-test override),
    // e.g. a cookie banner. Observation marks Go occluded.
    let page = search_page().cover(101, 100);
    let mut s = session(ScriptBuilder::new().observe(&page));
    let manifold = ultra_instinct_agent::BrowserRuntime::observe(&mut s)
        .unwrap()
        .clone();
    let go = manifold
        .regions()
        .find(|r| r.label() == "Go")
        .expect("Go observed");
    assert!(go.flags().occluded(), "hit-test must mark Go occluded");
    let space = ultra_instinct_agent::Agent::<BrowserSession<ReplayTransport>, InstinctPolicy>::action_space(
        &manifold,
    );
    assert!(space
        .targets_of(ActionKind::Click)
        .all(|a| a.label() != "Go"));
    // And the gate refuses it outright if anything ever proposed it.
    assert_eq!(
        ultra_instinct_guard::gate(
            &manifold,
            go.id(),
            ultra_instinct_core::Action::Click,
            None,
            0
        ),
        Err(ultra_instinct_protocol::GuardReason::Occluded)
    );
}

#[test]
fn select_over_cdp_uses_select_function_and_verifies_option_text() {
    let select = Control {
        tag: "SELECT",
        role: "combobox",
        ..Control::button(20, 200, "Cabin class", (20.0, 80.0, 200.0, 28.0))
    };
    let page = PageSpec::of(
        &[
            select,
            Control::button(21, 201, "Find flights", (20.0, 140.0, 140.0, 28.0)),
        ],
        "http://127.0.0.1/travel",
        "Travel",
    );
    let script = ScriptBuilder::new()
        .observe(&page)
        .observe(&page)
        .dom_input(20)
        .observe(&page)
        .dom_read_value(20, "business", "Business");
    let mut agent = AgentBuilder::new(session(script), InstinctPolicy::default())
        .max_steps(2)
        .build(r#"Select "Business" in Cabin class"#);
    let p = agent.predict().unwrap().expect("select").clone();
    assert_eq!(p.decision.kind, ActionKind::Select, "{p:?}");
    let rec = agent.act().unwrap();
    assert_eq!(rec.verification, VerificationKind::Success);
    let transport = agent.browser_mut().transport();
    let fns = calls_of(transport.logged_calls(), "Runtime.callFunctionOn");
    assert_eq!(fns[0]["functionDeclaration"], DOM_SELECT_FUNCTION);
    assert_eq!(fns[0]["arguments"][0]["value"], "Business");
}

fn cabin_page_with_observed_options() -> PageSpec {
    let select = Control {
        tag: "SELECT",
        role: "combobox",
        ..Control::button(20, 200, "Cabin class", (20.0, 80.0, 200.0, 28.0))
    };
    let mut page = PageSpec::of(
        &[
            select,
            Control::button(21, 201, "Find flights", (20.0, 140.0, 140.0, 28.0)),
        ],
        "http://127.0.0.1/travel",
        "Travel",
    );
    page.dom[0] = page.dom[0]
        .clone()
        .with_compact_state(json!({"o": ["Economy", "Business"], "sel": "Economy"}));
    page
}

#[test]
fn unquoted_select_goal_uses_observed_option_text() {
    let page = cabin_page_with_observed_options();
    let script = ScriptBuilder::new()
        .observe_compact(&page)
        .observe_compact(&page)
        .dom_input(20)
        .ready_state("complete")
        .observe_compact(&page)
        .dom_read_value(20, "business", "Business");
    let mut agent = AgentBuilder::new(session(script), SelectThenDone)
        .max_steps(2)
        .build("Set cabin class to business");
    let prediction = agent.predict().unwrap().expect("select").clone();
    assert_eq!(
        prediction.decision.kind,
        ActionKind::Select,
        "{prediction:?}"
    );
    assert_eq!(prediction.payload.as_deref(), Some("Business"));

    let record = agent.act().unwrap();
    assert_eq!(record.verification, VerificationKind::Success);
    let functions = calls_of(
        agent.browser_mut().transport().logged_calls(),
        "Runtime.callFunctionOn",
    );
    assert_eq!(functions[0]["functionDeclaration"], DOM_SELECT_FUNCTION);
    assert_eq!(functions[0]["arguments"][0]["value"], "Business");
}

#[test]
fn unquoted_select_goal_uses_observed_option_text_when_resolver_abstains() {
    let page = cabin_page_with_observed_options();
    let script = ScriptBuilder::new()
        .observe_compact(&page)
        .observe_compact(&page)
        .dom_input(20)
        .ready_state("complete")
        .observe_compact(&page)
        .dom_read_value(20, "business", "Business");
    let mut agent = AgentBuilder::new(session(script), SelectThenDone)
        .text_resolver(AbstainingTextResolver)
        .max_steps(2)
        .build("Set cabin class to business");
    let prediction = agent.predict().unwrap().expect("select").clone();
    assert_eq!(
        prediction.decision.kind,
        ActionKind::Select,
        "{prediction:?}"
    );
    assert_eq!(prediction.payload.as_deref(), Some("Business"));

    let record = agent.act().unwrap();
    assert_eq!(record.verification, VerificationKind::Success);
    let functions = calls_of(
        agent.browser_mut().transport().logged_calls(),
        "Runtime.callFunctionOn",
    );
    assert_eq!(functions[0]["functionDeclaration"], DOM_SELECT_FUNCTION);
    assert_eq!(functions[0]["arguments"][0]["value"], "Business");
}

#[test]
fn unmatched_select_goal_abstains_without_dispatch() {
    let page = cabin_page_with_observed_options();
    let script = ScriptBuilder::new().observe_compact(&page);
    let mut agent = AgentBuilder::new(session(script), SelectThenDone)
        .max_steps(2)
        .build("Set cabin class to Premium");

    let error = agent.predict().expect_err("unmatched SELECT must abstain");
    assert!(
        matches!(error, AgentError::Abstain(ref reason) if reason.contains("select:")),
        "{error:?}"
    );
    assert!(calls_of(
        agent.browser_mut().transport().logged_calls(),
        "Runtime.callFunctionOn"
    )
    .is_empty());
}

#[test]
fn scroll_over_cdp_is_one_wheel_event_at_viewport_center() {
    let page = search_page();
    let script = ScriptBuilder::new().observe(&page).scroll().observe(&page);
    let mut agent = AgentBuilder::new(session(script), InstinctPolicy::default())
        .max_steps(2)
        .build("scroll down");
    agent.predict().unwrap();
    let rec = agent.act().unwrap();
    assert_eq!(rec.kind, ActionKind::ScrollDown);
    let transport = agent.browser_mut().transport();
    let wheels = calls_of(transport.logged_calls(), "Input.dispatchMouseEvent");
    assert_eq!(wheels.len(), 1);
    assert_eq!(wheels[0]["type"], "mouseWheel");
    assert_eq!(wheels[0]["x"], 640.0);
    assert_eq!(wheels[0]["y"], 360.0);
    assert_eq!(
        wheels[0]["deltaY"],
        (720.0 * SCROLL_VIEWPORT_FRACTION).round()
    );
}

#[test]
fn autocomplete_type_then_option_click_over_cdp() {
    let empty = PageSpec::of(
        &[Control::combobox(
            10,
            100,
            "City",
            (10.0, 10.0, 200.0, 28.0),
        )],
        "http://127.0.0.1/auto",
        "Auto",
    );
    let open = PageSpec::of(
        &[
            Control::combobox(10, 100, "City", (10.0, 10.0, 200.0, 28.0)),
            Control::option(11, 110, "Manila", (10.0, 40.0, 200.0, 28.0)),
        ],
        "http://127.0.0.1/auto",
        "Auto",
    );
    // Post-click page keeps Manila (history → DONE) and adds a marker so
    // verify sees state-changed rather than no visible change.
    let picked = PageSpec::of(
        &[
            Control::combobox(10, 100, "City", (10.0, 10.0, 200.0, 28.0)),
            Control::option(11, 110, "Manila", (10.0, 40.0, 200.0, 28.0)),
            Control::button(12, 120, "picked", (10.0, 80.0, 80.0, 24.0)),
        ],
        "http://127.0.0.1/auto",
        "Auto",
    );
    let script = ScriptBuilder::new()
        .observe(&empty) // predict TYPE
        .observe(&empty) // executor revalidate TYPE
        .dom_input(10)
        .observe(&open) // after TYPE (popup open)
        .dom_read_value(10, "man", "")
        .observe(&open) // TYPE clause → DONE
        .observe(&open) // predict CLICK Manila
        .observe(&open) // executor revalidate CLICK
        .pointer_click()
        .observe(&picked) // after CLICK (state-changed)
        .observe(&picked); // CLICK clause → DONE
    let mut agent = AgentBuilder::new(session(script), InstinctPolicy::default())
        .max_steps(6)
        .build(r#"Type "man" into City then click Manila"#);
    let outcome = agent.run();
    assert!(matches!(outcome, AgentOutcome::Done { .. }), "{outcome:?}");
    let steps = outcome.steps();
    assert_eq!(steps.len(), 2, "{steps:?}");
    assert_eq!(steps[0].kind, ActionKind::TypeText);
    assert_eq!(steps[1].kind, ActionKind::Click);
    assert_eq!(steps[1].label, "Manila");
    let transport = agent.browser_mut().transport();
    assert_eq!(transport.remaining(), 0, "every scripted call consumed");
}

#[test]
fn autocomplete_waits_for_stable_delayed_options_over_cdp() {
    let empty = autocomplete_page(false, false);
    let open = autocomplete_page(true, false);
    let picked = autocomplete_page(true, true);
    let script = ScriptBuilder::new()
        .observe(&empty) // predict TYPE
        .observe(&empty) // executor revalidate TYPE
        .dom_input(10)
        .dom_read_autocomplete_signature(10, serde_json::json!("o0:"))
        .dom_read_autocomplete_signature(10, serde_json::json!("o1:Manila"))
        .dom_read_autocomplete_signature(10, serde_json::json!("o1:Manila"))
        .observe(&open) // after TYPE
        .dom_read_value(10, "man", "")
        .observe(&open) // TYPE clause → DONE
        .observe(&open) // predict CLICK Manila
        .observe(&open) // executor revalidate CLICK
        .pointer_click()
        .observe(&picked) // after CLICK
        .observe(&picked); // goal → DONE
    let mut agent = AgentBuilder::new(session(script), InstinctPolicy::default())
        .max_steps(6)
        .build(r#"Type "man" into City then click Manila"#);
    let outcome = agent.run();
    assert!(matches!(outcome, AgentOutcome::Done { .. }), "{outcome:?}");
    let steps = outcome.steps();
    assert_eq!(steps.len(), 2, "{steps:?}");
    assert_eq!(steps[0].kind, ActionKind::TypeText);
    assert_eq!(steps[1].kind, ActionKind::Click);
    assert_eq!(steps[1].label, "Manila");

    let transport = agent.browser_mut().transport();
    assert_eq!(transport.remaining(), 0, "every scripted call consumed");
    assert_eq!(autocomplete_poll_calls(transport.logged_calls()).len(), 3);
}

#[test]
fn autocomplete_with_no_options_uses_all_bounded_polls_over_cdp() {
    let empty = autocomplete_page(false, false);
    let mut script = ScriptBuilder::new()
        .observe(&empty) // predict TYPE
        .observe(&empty) // executor revalidate TYPE
        .dom_input(10);
    for _ in 0..10 {
        script = script.dom_read_autocomplete_signature(10, serde_json::json!("o0:"));
    }
    let script = script
        .observe(&empty) // after TYPE
        .dom_read_value(10, "man", "")
        .observe(&empty); // goal → DONE
    let mut agent = AgentBuilder::new(session(script), InstinctPolicy::default())
        .max_steps(4)
        .build(r#"Type "man" into City"#);
    let outcome = agent.run();
    assert!(matches!(outcome, AgentOutcome::Done { .. }), "{outcome:?}");
    assert_eq!(outcome.steps().len(), 1);
    assert_eq!(outcome.steps()[0].kind, ActionKind::TypeText);

    let transport = agent.browser_mut().transport();
    assert_eq!(transport.remaining(), 0, "all ten polls were consumed");
    assert_eq!(autocomplete_poll_calls(transport.logged_calls()).len(), 10);
}

#[test]
fn autocomplete_polling_stops_when_interrupted_over_cdp() {
    let empty = autocomplete_page(false, false);
    let script = ScriptBuilder::new()
        .observe(&empty) // predict TYPE
        .observe(&empty) // executor revalidate TYPE
        .dom_input(10)
        .dom_read_autocomplete_signature(10, serde_json::json!("o0:"))
        .dom_read_autocomplete_signature(10, serde_json::Value::Null)
        .ready_state("complete") // settle after interruption
        .observe(&empty) // after TYPE
        .dom_read_value(10, "man", "")
        .observe(&empty); // goal → DONE
    let mut agent = AgentBuilder::new(session(script), InstinctPolicy::default())
        .max_steps(4)
        .build(r#"Type "man" into City"#);
    let outcome = agent.run();
    assert!(matches!(outcome, AgentOutcome::Done { .. }), "{outcome:?}");
    assert_eq!(outcome.steps().len(), 1);
    assert_eq!(outcome.steps()[0].kind, ActionKind::TypeText);

    let transport = agent.browser_mut().transport();
    assert_eq!(transport.remaining(), 0, "every scripted call consumed");
    assert_eq!(autocomplete_poll_calls(transport.logged_calls()).len(), 2);
}

#[test]
fn autocomplete_waits_past_stable_unrelated_document_options_over_cdp() {
    let empty = document_wide_autocomplete_page(false, false);
    let mut script = ScriptBuilder::new()
        .observe(&empty) // predict TYPE
        .observe(&empty) // executor revalidation TYPE
        .dom_input(10);
    for _ in 0..10 {
        script = script.dom_read_autocomplete_signature(10, serde_json::json!("d1:Canada"));
    }
    let script = script
        .observe(&empty) // after TYPE
        .dom_read_value(10, "man", "")
        .observe(&empty); // goal → DONE
    let mut agent = AgentBuilder::new(session(script), InstinctPolicy::default())
        .max_steps(4)
        .build(r#"Type "man" into City"#);
    let outcome = agent.run();
    assert!(matches!(outcome, AgentOutcome::Done { .. }), "{outcome:?}");
    assert_eq!(outcome.steps().len(), 1);
    assert_eq!(outcome.steps()[0].kind, ActionKind::TypeText);

    let transport = agent.browser_mut().transport();
    assert_eq!(transport.remaining(), 0, "all ten polls were consumed");
    assert_eq!(autocomplete_poll_calls(transport.logged_calls()).len(), 10);
}

#[test]
fn autocomplete_stops_when_document_options_change_over_cdp() {
    let empty = document_wide_autocomplete_page(false, false);
    let open = document_wide_autocomplete_page(true, false);
    let script = ScriptBuilder::new()
        .observe(&empty) // predict TYPE
        .observe(&empty) // executor revalidation TYPE
        .dom_input(10)
        .dom_read_autocomplete_signature(10, serde_json::json!("d1:Canada"))
        .dom_read_autocomplete_signature(10, serde_json::json!("d2:Canada\u{001f}Manila"))
        .dom_read_autocomplete_signature(10, serde_json::json!("d2:Canada\u{001f}Manila"))
        .observe(&open) // after TYPE
        .dom_read_value(10, "man", "")
        .observe(&open); // goal → DONE
    let mut agent = AgentBuilder::new(session(script), InstinctPolicy::default())
        .max_steps(4)
        .build(r#"Type "man" into City"#);
    let outcome = agent.run();
    assert!(matches!(outcome, AgentOutcome::Done { .. }), "{outcome:?}");
    assert_eq!(outcome.steps().len(), 1);
    assert_eq!(outcome.steps()[0].kind, ActionKind::TypeText);

    let transport = agent.browser_mut().transport();
    assert_eq!(transport.remaining(), 0, "every scripted call consumed");
    assert_eq!(autocomplete_poll_calls(transport.logged_calls()).len(), 3);
}
