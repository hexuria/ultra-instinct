//! Exact errors and press bookkeeping of a `BrowserSession` over a replay
//! script (moved out of `lib.rs` to keep the crate under its line ceiling).

use ultra_instinct_browser::script::{AxSpec, DomSpec, PageSpec, ScriptBuilder};
use ultra_instinct_browser::*;
use ultra_instinct_core::{Action, RegionId};

#[test]
fn a_scripted_cdp_error_is_the_exact_protocol_error() {
    let mut transport =
        ReplayTransport::parse(r#"{"calls":[{"method":"DOM.resolveNode","error":"boom"}]}"#)
            .unwrap();
    assert_eq!(
        transport.call("DOM.resolveNode", "{}"),
        Err(CdpError::Protocol {
            message: "boom".into()
        })
    );
}

#[test]
fn session_verify_wraps_the_exact_verify_error() {
    let transport =
        ReplayTransport::parse(include_str!("../../../fixtures/sign-in.cdp.json")).unwrap();
    let mut session = BrowserSession::new(transport);
    session.observe().unwrap();
    let err = session
        .verify(&Expectation::text_present("Welcome").unwrap())
        .unwrap_err();
    assert_eq!(
        err,
        BrowserError::Verify(VerifyError::ExpectedTextMissing {
            expected: "Welcome".into()
        })
    );
}

#[test]
fn press_on_an_unknown_region_is_exact() {
    let transport =
        ReplayTransport::parse(include_str!("../../../fixtures/sign-in.cdp.json")).unwrap();
    let mut session = BrowserSession::new(transport);
    session.observe().unwrap();
    let err = session
        .press(&RegionId::try_new("nope").unwrap(), Action::Click)
        .unwrap_err();
    assert_eq!(err, BrowserError::UnknownRegion("nope".into()));
}

#[test]
fn two_dom_nodes_with_one_backend_id_are_the_exact_duplicate_error() {
    let page = PageSpec::new(
        vec![
            DomSpec::button(10, 100, "Sign in", (400.0, 300.0, 80.0, 32.0)),
            DomSpec::button(20, 100, "Cancel", (400.0, 360.0, 80.0, 32.0)),
        ],
        Vec::new(),
        "https://example.test/",
        "Example",
    );
    let script = ScriptBuilder::new().observe(&page).to_json();
    let mut session = BrowserSession::new(ReplayTransport::parse(&script).unwrap());
    let err = session.observe().unwrap_err();
    assert_eq!(
        err,
        BrowserError::DuplicateRegion("duplicate region id `n100`".into())
    );
}

fn sign_in_page() -> PageSpec {
    PageSpec::new(
        vec![DomSpec::button(
            10,
            100,
            "Sign in",
            (400.0, 300.0, 80.0, 32.0),
        )],
        vec![AxSpec::new(
            100,
            "button",
            "Sign in",
            (400.0, 300.0, 80.0, 32.0),
        )],
        "https://example.test/sign-in",
        "Sign in",
    )
}

#[test]
fn pointer_click_is_a_trusted_move_press_release_at_the_center() {
    let script = ScriptBuilder::new()
        .observe(&sign_in_page())
        .pointer_click()
        .to_json();
    let mut session = BrowserSession::new(ReplayTransport::parse(&script).unwrap());
    session.observe().unwrap();
    let mechanism = session
        .pointer_click(&RegionId::try_new("n100").unwrap())
        .unwrap();
    assert_eq!(mechanism, ActMechanism::Coordinate);
    let calls = session.transport().logged_calls();
    assert!(calls.iter().all(|(m, _)| m != "Runtime.callFunctionOn"));
    let mouse: Vec<serde_json::Value> = calls
        .iter()
        .filter(|(m, _)| m == "Input.dispatchMouseEvent")
        .map(|(_, p)| serde_json::from_str(p).unwrap())
        .collect();
    let kinds: Vec<&str> = mouse.iter().map(|p| p["type"].as_str().unwrap()).collect();
    assert_eq!(kinds, ["mouseMoved", "mousePressed", "mouseReleased"]);
    assert!(mouse.iter().all(|p| p["x"] == 440.0 && p["y"] == 316.0));
    assert_eq!(session.transport().remaining(), 0);
}

#[test]
fn pointer_click_refused_by_cdp_falls_back_to_the_dom_click() {
    let script = ScriptBuilder::new()
        .observe(&sign_in_page())
        .pointer_refused_then_dom_click(10)
        .to_json();
    let mut session = BrowserSession::new(ReplayTransport::parse(&script).unwrap());
    session.observe().unwrap();
    let mechanism = session
        .pointer_click(&RegionId::try_new("n100").unwrap())
        .unwrap();
    assert_eq!(mechanism, ActMechanism::DomSemantic);
    assert_eq!(session.transport().remaining(), 0);
}

#[test]
fn press_marks_the_observation_stale_and_observe_clears_it() {
    let page = PageSpec::new(
        vec![DomSpec::button(
            10,
            100,
            "Sign in",
            (400.0, 300.0, 80.0, 32.0),
        )],
        vec![AxSpec::new(
            100,
            "button",
            "Sign in",
            (400.0, 300.0, 80.0, 32.0),
        )],
        "https://example.test/sign-in",
        "Sign in",
    );
    let after = PageSpec::new(
        vec![DomSpec::button(
            20,
            200,
            "Welcome",
            (400.0, 300.0, 80.0, 32.0),
        )],
        vec![AxSpec::new(
            200,
            "button",
            "Welcome",
            (400.0, 300.0, 80.0, 32.0),
        )],
        "https://example.test/welcome",
        "Welcome",
    );
    let script = ScriptBuilder::new()
        .observe(&page)
        .dom_click(10)
        .observe(&after)
        .to_json();
    let mut session = BrowserSession::new(ReplayTransport::parse(&script).unwrap());
    session.observe().unwrap();
    assert!(!session.is_stale());
    assert!(session.fresh_manifold().is_some());
    session
        .press(&RegionId::try_new("n100").unwrap(), Action::Click)
        .unwrap();
    assert!(session.is_stale());
    assert!(session.fresh_manifold().is_none());
    // The stored manifold is still there for inspect of the last view, but
    // it must not be reused as an act's before.
    assert!(session.manifold().is_some());
    session.observe().unwrap();
    assert!(!session.is_stale());
    assert_eq!(
        session.manifold().unwrap().get_str("n200").unwrap().label(),
        "Welcome"
    );
}
