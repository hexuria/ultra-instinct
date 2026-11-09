//! Observe, identity, press tiers, focus and history over CDP replay
//! scripts, through the public API only.

use ultra_instinct_browser::*;
use ultra_instinct_core::{Action, RegionId, Role, SourceMask};
use ultra_instinct_observe::diff;

#[test]
fn observe_sign_in_on_cdp_snapshot_fuses_the_button() {
    let transport =
        ReplayTransport::parse(include_str!("../../../fixtures/sign-in.cdp.json")).unwrap();
    let mut session = BrowserSession::new(transport);
    let manifold = session.observe().unwrap().clone();
    assert_eq!(manifold.len(), 2);
    let sign_in = manifold.get_str("n100").unwrap();
    assert_eq!(sign_in.label(), "Sign in");
    assert!(sign_in.sources().contains(SourceMask::DOM));
    assert!(sign_in.sources().contains(SourceMask::ACCESSIBILITY));
    assert_eq!(sign_in.rect().x(), 400.0);
}

#[test]
fn press_before_observe_is_not_observed() {
    let transport = ReplayTransport::parse(r#"{"calls":[]}"#).unwrap();
    let mut session = BrowserSession::new(transport);
    let err = session
        .press(&RegionId::try_new("n100").unwrap(), Action::Click)
        .unwrap_err();
    assert_eq!(err, BrowserError::NotObserved);
    assert_eq!(err.to_string(), "browser session has no observation yet");
    assert!(session.transport().logged_methods().is_empty());
}

#[test]
fn bad_viewport_and_missing_object_id_are_exact() {
    let transport = ReplayTransport::parse(
        r#"{"calls":[{"method":"Page.getLayoutMetrics","params":{},"result":{}}]}"#,
    )
    .unwrap();
    let mut session = BrowserSession::new(transport);
    let err = session.observe().unwrap_err();
    assert_eq!(
        err,
        BrowserError::BadViewport("missing cssLayoutViewport".into())
    );
    assert_eq!(err.to_string(), "viewport: missing cssLayoutViewport");

    let mut transport =
        ReplayTransport::parse(include_str!("../../../fixtures/sign-in.cdp.json")).unwrap();
    transport
        .append(
            r#"{"calls":[{"method":"DOM.resolveNode","params":{"nodeId":10},"result":{"object":{"type":"object"}}}]}"#,
        )
        .unwrap();
    let mut session = BrowserSession::new(transport);
    session.observe().unwrap();
    let err = session
        .press(&RegionId::try_new("n100").unwrap(), Action::Click)
        .unwrap_err();
    assert_eq!(err, BrowserError::MissingObjectId);
    assert_eq!(err.to_string(), "DOM.resolveNode returned no objectId");
    assert!(session
        .transport()
        .logged_methods()
        .iter()
        .all(|method| method != "Runtime.callFunctionOn" && method != "Input.dispatchMouseEvent"));
}

#[test]
fn press_uses_dom_semantic_click_when_a_dom_node_id_exists() {
    let mut transport =
        ReplayTransport::parse(include_str!("../../../fixtures/sign-in.cdp.json")).unwrap();
    transport
        .append(include_str!("../../../fixtures/press-only.cdp.json"))
        .unwrap();
    let mut session = BrowserSession::new(transport);
    session.observe().unwrap();
    let id = RegionId::try_new("n100").unwrap();
    let mechanism = session.press(&id, Action::Click).unwrap();
    assert_eq!(mechanism, ActMechanism::DomSemantic);
    let methods = session.transport().logged_methods();
    assert!(methods
        .iter()
        .any(|method| method == "Runtime.callFunctionOn"));
    assert!(methods
        .iter()
        .all(|method| method != "Input.dispatchMouseEvent"));
    let err = session.press(&id, Action::Type).unwrap_err();
    assert_eq!(err, BrowserError::UnsupportedAction("type".into()));
}

fn sign_in_then(script: &str) -> BrowserSession<ReplayTransport> {
    let mut transport =
        ReplayTransport::parse(include_str!("../../../fixtures/sign-in.cdp.json")).unwrap();
    transport.append(script).unwrap();
    let mut session = BrowserSession::new(transport);
    session.observe().unwrap();
    session
}

#[test]
fn node_id_failure_retries_by_backend_id_before_coordinates() {
    let mut session = sign_in_then(
        r#"{"calls":[
            {"method":"DOM.resolveNode","params":{"nodeId":10},"error":"node gone"},
            {"method":"DOM.resolveNode","params":{"backendNodeId":100},"result":{"object":{"objectId":"obj-100"}}},
            {"method":"Runtime.callFunctionOn","params":{"functionDeclaration":"function(){this.click()}","objectId":"obj-100","returnByValue":true},"result":{"result":{"type":"undefined"}}}
        ]}"#,
    );
    let mechanism = session
        .press(&RegionId::try_new("n100").unwrap(), Action::Click)
        .unwrap();
    assert_eq!(mechanism, ActMechanism::DomSemantic);
    let methods = session.transport().logged_methods();
    assert!(methods.iter().all(|method| method != "DOM.focus"));
    assert!(methods
        .iter()
        .all(|method| method != "Input.dispatchMouseEvent"));
}

#[test]
fn both_semantic_tiers_fail_then_coordinates() {
    let mut session = sign_in_then(
        r#"{"calls":[
            {"method":"DOM.resolveNode","params":{"nodeId":10},"error":"node gone"},
            {"method":"DOM.resolveNode","params":{"backendNodeId":100},"error":"node gone"},
            {"method":"Input.dispatchMouseEvent","params":{"type":"mousePressed","x":440.0,"y":316.0,"button":"left","clickCount":1},"result":{}},
            {"method":"Input.dispatchMouseEvent","params":{"type":"mouseReleased","x":440.0,"y":316.0,"button":"left","clickCount":1},"result":{}}
        ]}"#,
    );
    let mechanism = session
        .press(&RegionId::try_new("n100").unwrap(), Action::Click)
        .unwrap();
    assert_eq!(mechanism, ActMechanism::Coordinate);
    assert_eq!(mechanism.as_str(), "coordinate");
    assert!(session
        .transport()
        .logged_methods()
        .iter()
        .all(|method| method != "DOM.focus"));
}

#[test]
fn click_exception_is_a_tier_failure() {
    let mut session = sign_in_then(
        r#"{"calls":[
            {"method":"DOM.resolveNode","params":{"nodeId":10},"result":{"object":{"objectId":"obj-10"}}},
            {"method":"Runtime.callFunctionOn","params":{"functionDeclaration":"function(){this.click()}","objectId":"obj-10","returnByValue":true},"result":{"result":{"type":"object"},"exceptionDetails":{"text":"this.click is not a function"}}},
            {"method":"DOM.resolveNode","params":{"backendNodeId":100},"result":{"object":{"objectId":"obj-100"}}},
            {"method":"Runtime.callFunctionOn","params":{"functionDeclaration":"function(){this.click()}","objectId":"obj-100","returnByValue":true},"result":{"result":{"type":"undefined"}}}
        ]}"#,
    );
    let mechanism = session
        .press(&RegionId::try_new("n100").unwrap(), Action::Click)
        .unwrap();
    assert_eq!(mechanism, ActMechanism::DomSemantic);
    let resolves = session
        .transport()
        .logged_methods()
        .iter()
        .filter(|method| *method == "DOM.resolveNode")
        .count();
    assert_eq!(resolves, 2);
}

/// sign-in.cdp.json with the Sign in button re-rendered as node 90 /
/// backend 900 at `x`.
fn rerendered_sign_in(x: f64) -> String {
    let mut value: serde_json::Value =
        serde_json::from_str(include_str!("../../../fixtures/sign-in.cdp.json")).unwrap();
    let calls = value["calls"].as_array_mut().unwrap();
    let button = &mut calls[1]["result"]["root"]["children"][0];
    button["nodeId"] = 90.into();
    button["backendNodeId"] = 900.into();
    calls[2]["result"]["nodes"][0]["backendDOMNodeId"] = 900.into();
    let quad = serde_json::json!([x, 300, x + 80.0, 300, x + 80.0, 332, x, 332]);
    calls[3]["params"] = serde_json::json!({"nodeId": 90});
    calls[3]["result"]["model"]["content"] = quad.clone();
    calls[5]["params"] = serde_json::json!({"backendNodeId": 900});
    calls[5]["result"]["model"]["content"] = quad;
    // Stable ids stay n100/n200; replace the n100 hit backend in place.
    for call in calls.iter_mut() {
        if call["method"] == "DOM.getNodeForLocation"
            && call["result"]["backendNodeId"].as_i64() == Some(100)
        {
            call["result"]["backendNodeId"] = 900.into();
        }
    }
    value.to_string()
}

#[test]
fn rerendered_button_keeps_its_id_with_a_new_backend_node() {
    let mut transport =
        ReplayTransport::parse(include_str!("../../../fixtures/sign-in.cdp.json")).unwrap();
    transport.append(&rerendered_sign_in(400.0)).unwrap();
    transport
        .append(
            r#"{"calls":[
                {"method":"DOM.resolveNode","params":{"nodeId":90},"result":{"object":{"objectId":"obj-90"}}},
                {"method":"Runtime.callFunctionOn","params":{"functionDeclaration":"function(){this.click()}","objectId":"obj-90","returnByValue":true},"result":{"result":{"type":"undefined"}}}
            ]}"#,
        )
        .unwrap();
    let mut session = BrowserSession::new(transport);
    let before = session.observe().unwrap().clone();
    let after = session.observe().unwrap().clone();
    let ids: Vec<_> = after.ids().map(RegionId::as_str).collect();
    assert_eq!(ids, ["n100", "n200"]);
    assert!(diff(&before, &after).is_empty());
    let mechanism = session
        .press(&RegionId::try_new("n100").unwrap(), Action::Click)
        .unwrap();
    assert_eq!(mechanism, ActMechanism::DomSemantic);
}

#[test]
fn distant_same_label_does_not_inherit_identity() {
    let mut transport =
        ReplayTransport::parse(include_str!("../../../fixtures/sign-in.cdp.json")).unwrap();
    transport.append(&rerendered_sign_in(1100.0)).unwrap();
    let mut session = BrowserSession::new(transport);
    session.observe().unwrap();
    let after = session.observe().unwrap();
    let ids: Vec<_> = after.ids().map(RegionId::as_str).collect();
    assert_eq!(ids, ["n200", "n900"]);
}

#[test]
fn observe_records_the_nearest_dom_parent() {
    let mut value: serde_json::Value =
        serde_json::from_str(include_str!("../../../fixtures/sign-in.cdp.json")).unwrap();
    let calls = value["calls"].as_array_mut().unwrap();
    let buttons = calls[1]["result"]["root"]["children"].take();
    calls[1]["result"]["root"]["children"] = serde_json::json!([{
        "nodeId": 5,
        "backendNodeId": 50,
        "nodeType": 1,
        "nodeName": "NAV",
        "attributes": ["aria-label", "Account"],
        "children": buttons
    }]);
    calls.insert(
        3,
        serde_json::json!({
            "method": "DOM.getBoxModel",
            "params": {"nodeId": 5},
            "result": {"model": {"content": [380, 280, 500, 280, 500, 400, 380, 400]}}
        }),
    );
    let transport = ReplayTransport::parse(&value.to_string()).unwrap();
    let mut session = BrowserSession::new(transport);
    let manifold = session.observe().unwrap();
    assert_eq!(manifold.len(), 3);
    let parent = |id: &str| {
        manifold
            .get_str(id)
            .unwrap()
            .parent()
            .map(|parent| parent.to_string())
    };
    assert_eq!(parent("n100").as_deref(), Some("n50"));
    assert_eq!(parent("n200").as_deref(), Some("n50"));
    assert_eq!(parent("n50"), None);
}

#[test]
fn single_observation_ids_are_unchanged() {
    let transport =
        ReplayTransport::parse(include_str!("../../../fixtures/sign-in.cdp.json")).unwrap();
    let mut session = BrowserSession::new(transport);
    let ids: Vec<_> = session
        .observe()
        .unwrap()
        .ids()
        .map(|id| id.to_string())
        .collect();
    assert_eq!(ids, ["n100", "n200"]);
}

#[test]
fn observe_omits_a_node_whose_box_model_is_a_protocol_error() {
    let transport =
        ReplayTransport::parse(include_str!("../../../fixtures/hidden-node.cdp.json")).unwrap();
    let mut session = BrowserSession::new(transport);
    let manifold = session.observe().unwrap();
    assert_eq!(manifold.len(), 2);
    assert!(manifold.get_str("n300").is_none());
    let sign_in = manifold.get_str("n100").unwrap();
    assert!(sign_in.sources().contains(SourceMask::ACCESSIBILITY));
}

#[test]
fn observe_still_fails_when_the_box_model_step_is_missing() {
    let full = include_str!("../../../fixtures/sign-in.cdp.json");
    let mut value: serde_json::Value = serde_json::from_str(full).unwrap();
    value["calls"].as_array_mut().unwrap().truncate(4);
    let transport = ReplayTransport::parse(&value.to_string()).unwrap();
    let mut session = BrowserSession::new(transport);
    let err = session.observe().unwrap_err();
    assert_eq!(
        err,
        BrowserError::Cdp(CdpError::NoScriptedResponse {
            method: "DOM.getBoxModel".into()
        })
    );
}

#[test]
fn box_model_params_mismatch_is_still_fatal() {
    let full = include_str!("../../../fixtures/sign-in.cdp.json");
    let mut value: serde_json::Value = serde_json::from_str(full).unwrap();
    value["calls"][3]["params"] = serde_json::json!({"nodeId": 99});
    let transport = ReplayTransport::parse(&value.to_string()).unwrap();
    let mut session = BrowserSession::new(transport);
    let err = session.observe().unwrap_err();
    assert_eq!(
        err,
        BrowserError::Cdp(CdpError::ParamsMismatch {
            method: "DOM.getBoxModel".into()
        })
    );
}

#[test]
fn refresh_then_verify_sees_welcome_and_diffs_the_old_button() {
    let mut transport =
        ReplayTransport::parse(include_str!("../../../fixtures/sign-in.cdp.json")).unwrap();
    transport
        .append(include_str!("../../../fixtures/press-only.cdp.json"))
        .unwrap();
    transport
        .append(include_str!("../../../fixtures/welcome.cdp.json"))
        .unwrap();
    let mut session = BrowserSession::new(transport);
    let before = session.observe().unwrap().clone();
    session
        .press(&RegionId::try_new("n100").unwrap(), Action::Click)
        .unwrap();
    let after = session.observe().unwrap().clone();
    let delta = diff(&before, &after);
    assert!(delta.removed().iter().any(|id| id.as_str() == "n100"));
    assert!(delta.added().iter().any(|id| id.as_str() == "n300"));
    session
        .verify(&Expectation::text_present("Welcome").unwrap())
        .unwrap();
    let err = verify(&before, &Expectation::text_present("Welcome").unwrap()).unwrap_err();
    assert_eq!(
        err,
        VerifyError::ExpectedTextMissing {
            expected: "Welcome".into()
        }
    );
}

#[test]
#[ignore = "read-only attach to a local Chrome; set ULTRA_INSTINCT_CDP=http://127.0.0.1:PORT"]
fn live_browser_get_version_reads_a_cdp_socket() {
    let endpoint =
        std::env::var("ULTRA_INSTINCT_CDP").unwrap_or_else(|_| DEFAULT_CDP_HTTP.to_owned());
    let mut socket = WebSocketTransport::connect(&endpoint).expect("connect");
    let version = socket.call("Browser.getVersion", "{}").expect("version");
    assert!(
        version.contains("Chrome") || version.contains("protocolVersion"),
        "{version}"
    );
}

fn history_url(script: &str, url: &str) -> String {
    let mut value: serde_json::Value = serde_json::from_str(script).unwrap();
    let history = value["calls"]
        .as_array_mut()
        .unwrap()
        .iter_mut()
        .find(|call| call["method"] == "Page.getNavigationHistory")
        .unwrap();
    history["result"]["entries"][0]["url"] = serde_json::json!(url);
    value.to_string()
}

#[test]
fn url_change_is_reported_without_a_region_change() {
    let base = include_str!("../../../fixtures/sign-in.cdp.json");
    let mut transport =
        ReplayTransport::parse(&history_url(base, "https://example.test/sign-in")).unwrap();
    transport
        .append(&history_url(base, "https://example.test/account"))
        .unwrap();
    let mut session = BrowserSession::new(transport);
    let before = session.observe().unwrap().clone();
    let before_page = session.page().unwrap().clone();
    let after = session.observe().unwrap().clone();
    let after_page = session.page().unwrap().clone();
    assert!(diff(&before, &after).is_empty());
    assert_eq!(before.captured_at_ms(), 0);
    assert_eq!(after.captured_at_ms(), 0);
    let pages = page_delta(&before_page, &after_page);
    assert!(pages.url_changed());
    assert!(!pages.focus_changed());
    assert_eq!(after_page.url(), Some("https://example.test/account"));
    verify_delta(&diff(&before, &after), &pages, &Expectation::url_changed()).unwrap();
}

fn focus_sign_in(script: &str) -> String {
    let mut value: serde_json::Value = serde_json::from_str(script).unwrap();
    value["calls"][2]["result"]["nodes"][0]["properties"] = serde_json::json!([{
        "name": "focused",
        "value": {"type": "boolean", "value": true}
    }]);
    value.to_string()
}

fn with_focused_email(script: &str) -> String {
    let mut value: serde_json::Value = serde_json::from_str(script).unwrap();
    let calls = value["calls"].as_array_mut().unwrap();
    calls[1]["result"]["root"]["children"]
        .as_array_mut()
        .unwrap()
        .push(serde_json::json!({
            "nodeId": 30,
            "backendNodeId": 300,
            "nodeType": 1,
            "nodeName": "INPUT",
            "attributes": ["type", "email", "aria-label", "Email"],
            "children": []
        }));
    calls[2]["result"]["nodes"]
        .as_array_mut()
        .unwrap()
        .push(serde_json::json!({
            "nodeId": "ax30",
            "backendDOMNodeId": 300,
            "ignored": false,
            "role": {"value": "textbox"},
            "name": {"value": "Email"},
            "properties": [{
                "name": "focused",
                "value": {"type": "boolean", "value": true}
            }]
        }));
    let node_20 = calls
        .iter()
        .position(|call| {
            call["method"] == "DOM.getBoxModel"
                && call["params"].get("nodeId").and_then(|id| id.as_i64()) == Some(20)
        })
        .unwrap();
    calls.insert(
        node_20 + 1,
        serde_json::json!({
            "method": "DOM.getBoxModel",
            "params": {"nodeId": 30},
            "result": {"model": {"content": [400, 360, 560, 360, 560, 392, 400, 392]}}
        }),
    );
    let history_at = calls
        .iter()
        .position(|call| call["method"] == "Page.getNavigationHistory")
        .unwrap();
    calls.insert(
        history_at,
        serde_json::json!({
            "method": "DOM.getBoxModel",
            "params": {"backendNodeId": 300},
            "result": {"model": {"content": [400, 360, 560, 360, 560, 392, 400, 392]}}
        }),
    );
    // Email is clickable; hit-test order is n100, n200, n300.
    let history_at = calls
        .iter()
        .position(|call| call["method"] == "Page.getNavigationHistory")
        .unwrap();
    let insert_at = history_at + 1;
    // The fixture already has hits for 100 and 200 after history.
    calls.insert(
        insert_at + 2,
        serde_json::json!({
            "method": "DOM.getNodeForLocation",
            "result": {"backendNodeId": 300}
        }),
    );
    value.to_string()
}

#[test]
fn focus_moves_to_the_text_field() {
    let base = include_str!("../../../fixtures/sign-in.cdp.json");
    let mut transport = ReplayTransport::parse(&focus_sign_in(base)).unwrap();
    transport.append(&with_focused_email(base)).unwrap();
    let mut session = BrowserSession::new(transport);
    session.observe().unwrap();
    let before_page = session.page().unwrap().clone();
    assert_eq!(
        before_page
            .focused()
            .map(ultra_instinct_core::RegionId::as_str),
        Some("n100")
    );
    let after = session.observe().unwrap();
    let email = after.get_str("n300").unwrap();
    assert_eq!(email.role(), Role::TextField);
    assert_eq!(email.label(), "Email");
    let after_page = session.page().unwrap();
    assert_eq!(
        after_page
            .focused()
            .map(ultra_instinct_core::RegionId::as_str),
        Some("n300")
    );
    let pages = page_delta(&before_page, after_page);
    assert!(pages.focus_changed());
    assert!(!pages.url_changed());
}

#[test]
fn navigation_history_protocol_error_omits_url_and_title() {
    let mut value: serde_json::Value =
        serde_json::from_str(include_str!("../../../fixtures/sign-in.cdp.json")).unwrap();
    let history = value["calls"]
        .as_array_mut()
        .unwrap()
        .iter_mut()
        .find(|call| call["method"] == "Page.getNavigationHistory")
        .unwrap();
    history.as_object_mut().unwrap().remove("result");
    history["error"] = serde_json::json!("Inspector not attached");
    let transport = ReplayTransport::parse(&value.to_string()).unwrap();
    let mut session = BrowserSession::new(transport);
    let manifold = session.observe().unwrap();
    assert_eq!(manifold.captured_at_ms(), 0);
    let page = session.page().unwrap();
    assert_eq!(page.url(), None);
    assert_eq!(page.title(), None);
    assert!(page.focused().is_none());
}

#[test]
fn missing_navigation_history_step_is_fatal() {
    let mut value: serde_json::Value =
        serde_json::from_str(include_str!("../../../fixtures/sign-in.cdp.json")).unwrap();
    let calls = value["calls"].as_array_mut().unwrap();
    let history_at = calls
        .iter()
        .position(|call| call["method"] == "Page.getNavigationHistory")
        .unwrap();
    let removed = calls.remove(history_at);
    assert_eq!(removed["method"], "Page.getNavigationHistory");
    // Drop hit-tests that followed history so the next scripted method is
    // not a leftover getNodeForLocation.
    while history_at < calls.len() && calls[history_at]["method"] == "DOM.getNodeForLocation" {
        calls.remove(history_at);
    }
    let transport = ReplayTransport::parse(&value.to_string()).unwrap();
    let mut session = BrowserSession::new(transport);
    let err = session.observe().unwrap_err();
    assert_eq!(
        err,
        BrowserError::Cdp(CdpError::NoScriptedResponse {
            method: "Page.getNavigationHistory".into(),
        })
    );
    assert_eq!(
        err.to_string(),
        "no scripted CDP response for `Page.getNavigationHistory`"
    );
}
