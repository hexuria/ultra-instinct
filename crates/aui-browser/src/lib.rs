//! Browser surface for ultra-instinct.
//!
//! Phase 2 speaks Chrome DevTools Protocol through [`CdpTransport`].
//! [`ReplayTransport`] replays a recorded script. [`WebSocketTransport`] is
//! the same calls on a live `ws://` socket. There is no browser framework and
//! no computer-use vision path.
//!
//! The default HTTP endpoint is [`DEFAULT_CDP_HTTP`] (`http://127.0.0.1:9222`).
//! ultra-instinct does not launch Chrome. macOS and CUA stay outside this crate.

#![forbid(unsafe_code)]

mod compact;
mod error;
mod extract;
mod fusion;
mod identity;
mod page;
mod session;
mod stacking;
mod verify;

pub use aui_cdp::{
    open_tab, script, CdpError, CdpTransport, ReplayTransport, WebSocketTransport, DEFAULT_CDP_HTTP,
};
pub use error::{ActMechanism, BrowserError};
pub use fusion::{MAX_CENTROID_PX, MIN_IOU, MIN_LABEL_JACCARD};
pub use page::{page_delta, PageDelta, PageState};
pub use session::{
    BrowserSession, ScrollDirection, DOM_CLICK_FUNCTION, DOM_READ_VALUE_FUNCTION,
    DOM_SELECT_FUNCTION, DOM_TYPE_FUNCTION, DRAG_STEPS, DROP_PAST_CENTER, SCROLL_VIEWPORT_FRACTION,
};
pub use verify::{verify, verify_delta, Expectation, VerifyError};

#[cfg(feature = "fuzz-support")]
pub use compact::compact_parse_fuzzable;

/// A session can be opened. This is not a claim that a browser is running.
pub const STATUS: &str = "browser CDP client ready; act requires a session";

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct BrowserStub;

impl BrowserStub {
    pub const fn status(self) -> &'static str {
        STATUS
    }
}

#[cfg(test)]
mod proptest_parse {
    use super::*;
    use proptest::prelude::*;

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(16))]
        #[test]
        fn garbage_scripts_do_not_panic(raw in "\\PC{0,80}") {
            let _ = ReplayTransport::parse(&raw);
        }
    }

    // Structurally valid CDP pages. Labels are short ASCII so region ids stay
    // unique. Cases stay at 16 so CI stays fast.
    proptest! {
        #![proptest_config(ProptestConfig::with_cases(16))]
        #[test]
        fn structured_cdp_pages_observe_with_unique_ids(
            count in 1usize..=4,
            labels in prop::collection::vec("[A-Za-z][a-z]{1,5}", 1..=4),
            history_kind in 0u8..3,
        ) {
            use super::script::{AxSpec, DomSpec, HistorySpec, PageSpec, ScriptBuilder};
            let count = count.min(labels.len());
            let mut dom = Vec::with_capacity(count);
            let mut ax = Vec::with_capacity(count);
            for (index, label) in labels.iter().take(count).enumerate() {
                let node = 10 + index as i64 * 10;
                let backend = 100 + index as i64 * 100;
                let rect = (40.0 + 100.0 * index as f64, 300.0, 80.0, 32.0);
                dom.push(DomSpec::button(node, backend, label, rect));
                ax.push(AxSpec::new(backend, "button", label, rect));
            }
            let mut page = PageSpec::new(dom, ax, "https://example.test/", "Example");
            page.history = match history_kind {
                0 => HistorySpec::Entry {
                    url: "https://example.test/".into(),
                    title: "Example".into(),
                },
                1 => HistorySpec::ProtocolError,
                _ => HistorySpec::NoEntries,
            };
            let script = ScriptBuilder::new().observe(&page).to_json();
            let mut session = BrowserSession::new(ReplayTransport::parse(&script).unwrap());
            let count_seen = session.observe().expect("structured CDP must observe").len();
            let manifold = session.manifold().unwrap();
            let mut seen = std::collections::BTreeSet::new();
            for region in manifold.regions() {
                prop_assert!(seen.insert(region.id().clone()), "duplicate {}", region.id());
            }
            prop_assert_eq!(count_seen, count);
            match history_kind {
                0 => prop_assert!(session.page().unwrap().is_known()),
                _ => prop_assert!(!session.page().unwrap().is_known()),
            }
        }
    }
}

#[cfg(test)]
mod phase2 {
    use super::*;
    use aui_core::{Action, LocateQuery, RegionId, Role, SourceMask};
    use aui_observe::diff;
    use aui_resonance::{default_matcher, RegionMatcher};

    #[test]
    fn locate_sign_in_on_cdp_snapshot_ranks_the_button_first() {
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
        let query = LocateQuery::new().text("Sign in").unwrap();
        let ranked = default_matcher().rank(&query, &manifold).unwrap();
        assert_eq!(ranked[0].id().as_str(), "n100");
        assert_eq!(ranked[0].rank(), 1);
        assert!(ranked[0].confidence() > ranked[1].confidence());
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
            .all(
                |method| method != "Runtime.callFunctionOn" && method != "Input.dispatchMouseEvent"
            ));
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
        // Existing fixture already has hits for 100 and 200 after history.
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
            before_page.focused().map(aui_core::RegionId::as_str),
            Some("n100")
        );
        let after = session.observe().unwrap();
        let email = after.get_str("n300").unwrap();
        assert_eq!(email.role(), Role::TextField);
        assert_eq!(email.label(), "Email");
        let after_page = session.page().unwrap();
        assert_eq!(
            after_page.focused().map(aui_core::RegionId::as_str),
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
}

#[cfg(test)]
mod exact_errors {
    use super::script::{AxSpec, DomSpec, PageSpec, ScriptBuilder};
    use super::*;
    use aui_core::{Action, RegionId};

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
}

#[cfg(test)]
mod compact_observe {
    use super::script::{AxSpec, DomSpec, PageSpec, ScriptBuilder};
    use super::*;
    use crate::compact;

    fn page() -> PageSpec {
        PageSpec::new(
            vec![
                DomSpec::button(10, 100, "Sign in", (400.0, 300.0, 80.0, 32.0)),
                DomSpec::button(20, 200, "Email", (400.0, 200.0, 160.0, 32.0)),
            ],
            vec![
                AxSpec::new(100, "button", "Sign in", (400.0, 300.0, 80.0, 32.0)),
                AxSpec::new(200, "textbox", "Email", (400.0, 200.0, 160.0, 32.0)),
            ],
            "https://example.test/sign-in",
            "Sign in",
        )
    }

    #[test]
    fn compact_observe_uses_six_calls_total() {
        let script = ScriptBuilder::new().observe_compact(&page()).to_json();
        let mut session = BrowserSession::new(ReplayTransport::parse(&script).unwrap());
        let manifold = session.observe().unwrap();
        assert!(manifold.get_str("n100").is_some());
        assert!(manifold.get_str("n200").is_some());
        assert_eq!(
            session.transport().logged_methods(),
            vec![
                "Runtime.evaluate",
                "Page.getLayoutMetrics",
                "DOM.getDocument",
                "DOMSnapshot.captureSnapshot",
                "Accessibility.getFullAXTree",
                "Page.getNavigationHistory",
            ]
        );
    }

    #[test]
    fn a_script_listener_makes_a_box_clickable_and_a_canvas_a_surface() {
        let cell = |node, backend, x| {
            DomSpec::button(node, backend, "", (x, 100.0, 40.0, 40.0))
                .with_tag("SPAN")
                .with_attr("id", &format!("cell-{backend}"))
        };
        let page = PageSpec::new(
            vec![
                cell(10, 100, 10.0),
                cell(20, 200, 60.0),
                DomSpec::button(30, 300, "", (200.0, 0.0, 160.0, 210.0)).with_tag("CANVAS"),
                DomSpec::button(40, 400, "", (400.0, 0.0, 160.0, 210.0)).with_tag("CANVAS"),
            ],
            vec![],
            "https://example.test/",
            "Board",
        )
        .listener(100)
        .listener(400);
        let script = ScriptBuilder::new().observe_compact(&page).to_json();
        let mut session = BrowserSession::new(ReplayTransport::parse(&script).unwrap());
        let manifold = session.observe().unwrap();
        let cell = manifold.get_str("n100").expect("listener cell kept");
        assert_eq!(cell.label(), "cell");
        assert!(cell.actions().contains(&aui_core::Action::Click));
        assert!(
            manifold.get_str("n200").is_none(),
            "no listener, no hint: no region"
        );
        let points = |id: &str| {
            manifold
                .get_str(id)
                .is_some_and(|r| r.actions().contains(&aui_core::Action::Point))
        };
        assert!(
            !points("n300"),
            "a display-only canvas takes no point clicks"
        );
        assert!(points("n400"), "a canvas with a click listener does");
    }

    #[test]
    fn a_canvas_with_its_own_pointer_cursor_is_a_surface() {
        let page = PageSpec::new(
            vec![DomSpec::button(30, 300, "", (200.0, 0.0, 160.0, 210.0))
                .with_tag("CANVAS")
                .with_compact_state(serde_json::json!({"ck": 1}))],
            vec![],
            "https://example.test/",
            "Draw",
        );
        let script = ScriptBuilder::new().observe_compact(&page).to_json();
        let mut session = BrowserSession::new(ReplayTransport::parse(&script).unwrap());
        let region = session.observe().unwrap().get_str("n300").unwrap().clone();
        assert!(region.actions().contains(&aui_core::Action::Click));
        assert!(region.actions().contains(&aui_core::Action::Point));
    }

    #[test]
    fn script_clickable_text_is_offered_click_only_with_its_hint() {
        let span = |node, backend, label: &str, x, hint: bool| {
            let spec =
                DomSpec::button(node, backend, label, (x, 100.0, 30.0, 14.0)).with_tag("SPAN");
            if hint {
                spec.with_compact_state(serde_json::json!({"ck": 1}))
            } else {
                spec
            }
        };
        let page = PageSpec::new(
            vec![
                span(10, 100, "sem", 100.0, true),
                span(20, 200, "tellus", 200.0, false),
            ],
            vec![],
            "https://example.test/",
            "Links",
        );
        let script = ScriptBuilder::new().observe_compact(&page).to_json();
        let mut session = BrowserSession::new(ReplayTransport::parse(&script).unwrap());
        let manifold = session.observe().unwrap();
        let clickable = |id: &str| {
            manifold
                .get_str(id)
                .unwrap_or_else(|| panic!("{id} kept"))
                .actions()
                .contains(&aui_core::Action::Click)
        };
        assert!(clickable("n100"), "hinted span offers CLICK");
        assert!(!clickable("n200"), "plain text stays unclickable");
    }

    #[test]
    fn a_scrollable_box_claims_scroll_but_not_click() {
        let page = PageSpec::new(
            vec![DomSpec::button(10, 100, "", (10.0, 60.0, 140.0, 120.0))
                .with_tag("DIV")
                .with_compact_state(serde_json::json!({"sc": 1}))],
            vec![],
            "https://example.test/",
            "List",
        );
        let script = ScriptBuilder::new().observe_compact(&page).to_json();
        let mut session = BrowserSession::new(ReplayTransport::parse(&script).unwrap());
        let region = session
            .observe()
            .unwrap()
            .get_str("n100")
            .expect("scroll box kept")
            .clone();
        assert!(region.actions().contains(&aui_core::Action::Scroll));
        assert!(!region.actions().contains(&aui_core::Action::Click));
    }

    #[test]
    fn a_text_less_box_becomes_a_region_only_with_its_hint() {
        let boxed = |node, backend, x, hint: bool| {
            let spec = DomSpec::button(node, backend, "", (x, 100.0, 30.0, 30.0))
                .with_tag("DIV")
                .with_attr("style", "background-color: red;");
            if hint {
                spec.with_compact_state(serde_json::json!({"ck": 1}))
            } else {
                spec
            }
        };
        let page = PageSpec::new(
            vec![boxed(10, 100, 100.0, true), boxed(20, 200, 200.0, false)],
            vec![],
            "https://example.test/",
            "Boxes",
        );
        let script = ScriptBuilder::new().observe_compact(&page).to_json();
        let mut session = BrowserSession::new(ReplayTransport::parse(&script).unwrap());
        let manifold = session.observe().unwrap();
        let hinted = manifold.get_str("n100").expect("hinted box kept");
        assert_eq!(hinted.label(), "red");
        assert!(hinted.actions().contains(&aui_core::Action::Click));
        assert!(manifold.get_str("n200").is_none(), "no hint, no region");
    }

    #[test]
    fn compact_and_per_node_observe_build_the_same_manifold() {
        let mut legacy = BrowserSession::new(
            ReplayTransport::parse(&ScriptBuilder::new().observe(&page()).to_json()).unwrap(),
        );
        let mut compact = BrowserSession::new(
            ReplayTransport::parse(&ScriptBuilder::new().observe_compact(&page()).to_json())
                .unwrap(),
        );
        let snapshot = |session: &mut BrowserSession<_>| {
            session
                .observe()
                .unwrap()
                .regions()
                .map(|region| {
                    (
                        region.id().to_string(),
                        region.label().to_owned(),
                        region.rect(),
                        region.actions().to_vec(),
                        region.flags(),
                    )
                })
                .collect::<Vec<_>>()
        };
        assert_eq!(snapshot(&mut legacy), snapshot(&mut compact));
    }

    #[test]
    fn covered_scroll_and_drag_targets_are_occluded_too() {
        let mut overlay =
            DomSpec::container(30, 300, "banner", "Cookies", (0.0, 0.0, 800.0, 400.0));
        overlay.attributes.clear();
        let page = PageSpec::new(
            vec![
                DomSpec::button(10, 100, "Inbox", (20.0, 20.0, 300.0, 200.0))
                    .with_tag("DIV")
                    .with_compact_state(serde_json::json!({"sc": 1})),
                DomSpec::button(20, 200, "Card", (20.0, 250.0, 100.0, 40.0))
                    .with_tag("DIV")
                    .with_compact_state(serde_json::json!({"dg": 1})),
                DomSpec::button(40, 400, "Open", (20.0, 500.0, 100.0, 40.0))
                    .with_tag("DIV")
                    .with_compact_state(serde_json::json!({"sc": 1})),
                overlay,
            ],
            Vec::new(),
            "https://example.test/mail",
            "Mail",
        )
        .cover(100, 300)
        .cover(200, 300);
        let script = ScriptBuilder::new().observe_compact(&page).to_json();
        let mut session = BrowserSession::new(ReplayTransport::parse(&script).unwrap());
        let manifold = session.observe().unwrap();
        let inbox = manifold.get_str("n100").unwrap();
        assert!(
            inbox.actions().contains(&aui_core::Action::Scroll),
            "{inbox:?}"
        );
        assert!(inbox.flags().occluded(), "covered scroll box");
        assert!(
            manifold.get_str("n200").unwrap().flags().occluded(),
            "covered drag source"
        );
        assert!(!manifold.get_str("n400").unwrap().flags().occluded());
    }

    #[test]
    fn compact_observe_buries_a_covered_button_via_blob_hit() {
        // Unkept overlay: it earns a `data-hu-k` tag but no region, so the
        // blob hit-test (not stacking) is the only mechanism that can bury
        // n100. Its rect covers n100's center but not n200's.
        let overlay =
            DomSpec::container(30, 300, "banner", "Cookies", (350.0, 250.0, 200.0, 130.0));
        let mut overlay = overlay;
        overlay.attributes.clear();
        let page = PageSpec::new(
            vec![
                DomSpec::button(10, 100, "Sign in", (400.0, 300.0, 80.0, 32.0)),
                DomSpec::button(20, 200, "Email", (400.0, 200.0, 160.0, 32.0)),
                overlay,
            ],
            vec![AxSpec::new(
                100,
                "button",
                "Sign in",
                (400.0, 300.0, 80.0, 32.0),
            )],
            "https://example.test/sign-in",
            "Sign in",
        )
        .cover(100, 300);
        let script = ScriptBuilder::new().observe_compact(&page).to_json();
        let mut session = BrowserSession::new(ReplayTransport::parse(&script).unwrap());
        let manifold = session.observe().unwrap();
        assert!(manifold.get_str("n100").unwrap().flags().occluded());
        assert!(!manifold.get_str("n200").unwrap().flags().occluded());
    }

    #[test]
    fn compact_eval_throwing_falls_back_to_the_per_node_path() {
        // A page that breaks the walk (e.g. replaced DOM globals) must not
        // make observe fail: the eval is dropped, every element is joined
        // per node, and stale tags from the partial walk are ignored.
        let mut value: serde_json::Value =
            serde_json::from_str(&ScriptBuilder::new().observe(&page()).to_json()).unwrap();
        value["calls"].as_array_mut().unwrap().insert(
            0,
            serde_json::json!({
                "method": "Runtime.evaluate",
                "result": {
                    "result": {"type": "object"},
                    "exceptionDetails": {"text": "TypeError"}
                }
            }),
        );
        let legacy = ScriptBuilder::new().observe(&page()).to_json();
        let legacy_calls = serde_json::from_str::<serde_json::Value>(&legacy).unwrap()["calls"]
            .as_array()
            .unwrap()
            .len();
        let transport = ReplayTransport::parse(&value.to_string()).unwrap();
        let mut session = BrowserSession::new(transport);
        let manifold = session.observe().unwrap();
        assert!(manifold.get_str("n100").is_some());
        assert!(manifold.get_str("n200").is_some());
        assert_eq!(session.transport().logged_methods().len(), legacy_calls + 1);
    }

    /// The compact script with `data-hu-k` rewritten on the DOM node whose
    /// backend id is `backend`.
    fn retag(value: &mut serde_json::Value, backend: i64, k: &str) {
        fn walk(node: &mut serde_json::Value, backend: i64, k: &str) {
            if node["backendNodeId"] == backend {
                let attrs = node["attributes"].as_array_mut().unwrap();
                let at = attrs.iter().position(|a| a == "data-hu-k").unwrap();
                attrs[at + 1] = serde_json::json!(k);
            }
            if let Some(children) = node["children"].as_array_mut() {
                for child in children {
                    walk(child, backend, k);
                }
            }
        }
        walk(&mut value["calls"][2]["result"]["root"], backend, k);
    }

    #[test]
    fn duplicate_hu_k_tags_fall_back_per_node_for_every_holder() {
        // A page clone (or forged value) gives n200 the same key as n100.
        // Neither may consume the blob: both must take the per-node path.
        let mut value: serde_json::Value =
            serde_json::from_str(&ScriptBuilder::new().observe_compact(&page()).to_json()).unwrap();
        let k100 = {
            let root = &value["calls"][2]["result"]["root"];
            let node = root["children"]
                .as_array()
                .unwrap()
                .iter()
                .find(|n| n["backendNodeId"] == 100)
                .unwrap();
            let attrs = node["attributes"].as_array().unwrap();
            let at = attrs.iter().position(|a| a == "data-hu-k").unwrap();
            attrs[at + 1].as_str().unwrap().to_owned()
        };
        retag(&mut value, 200, &k100);
        let dom = extract::dom_document(&value["calls"][2]["result"].to_string()).unwrap();
        assert!(dom.hu_k_of_backend.is_empty());
        assert!(dom.backend_of_hu_k.is_empty());
        assert!(dom.elements.iter().all(|e| e.hu_k.is_none()));
    }

    #[test]
    fn compact_hit_naming_an_unknown_key_falls_back_to_get_node_for_location() {
        // A hit key no element carries (ambiguous and dropped, or never
        // present) must not read as "nothing here" — that would fail open.
        let mut value: serde_json::Value =
            serde_json::from_str(&ScriptBuilder::new().observe_compact(&page()).to_json()).unwrap();
        let nodes = value["calls"][0]["result"]["result"]["value"]["nodes"]
            .as_object_mut()
            .unwrap();
        for record in nodes.values_mut() {
            if record["h"].is_u64() {
                record["h"] = serde_json::json!(4_000_000);
            }
        }
        // n200's fallback lands on n100: only the per-node path can see it.
        for hit in [100, 100] {
            value["calls"]
                .as_array_mut()
                .unwrap()
                .push(serde_json::json!({
                    "method": "DOM.getNodeForLocation",
                    "result": {"backendNodeId": hit}
                }));
        }
        let transport = ReplayTransport::parse(&value.to_string()).unwrap();
        let mut session = BrowserSession::new(transport);
        let manifold = session.observe().unwrap();
        assert!(!manifold.get_str("n100").unwrap().flags().occluded());
        assert!(manifold.get_str("n200").unwrap().flags().occluded());
        assert_eq!(
            session
                .transport()
                .logged_methods()
                .iter()
                .filter(|m| *m == "DOM.getNodeForLocation")
                .count(),
            2
        );
    }

    #[test]
    fn compact_record_without_style_or_hit_falls_back_per_node() {
        // Non-candidate elements carry a rect only. If such an element still
        // becomes a region, its style and hit come from the per-node calls.
        let mut value: serde_json::Value =
            serde_json::from_str(&ScriptBuilder::new().observe_compact(&page()).to_json()).unwrap();
        let nodes = value["calls"][0]["result"]["result"]["value"]["nodes"]
            .as_object_mut()
            .unwrap();
        for record in nodes.values_mut() {
            let record = record.as_object_mut().unwrap();
            record.remove("s");
            record.remove("h");
        }
        let calls = value["calls"].as_array_mut().unwrap();
        calls.push(serde_json::json!({"method": "CSS.enable", "result": {}}));
        for node_id in [10, 20] {
            calls.push(serde_json::json!({
                "method": "CSS.getComputedStyleForNode",
                "params": {"nodeId": node_id},
                "result": {"computedStyle": [{"name": "z-index", "value": "auto"}]}
            }));
        }
        for backend in [100, 200] {
            calls.push(serde_json::json!({
                "method": "DOM.getNodeForLocation",
                "result": {"backendNodeId": backend}
            }));
        }
        let transport = ReplayTransport::parse(&value.to_string()).unwrap();
        let mut session = BrowserSession::new(transport);
        let manifold = session.observe().unwrap();
        assert!(!manifold.get_str("n100").unwrap().flags().occluded());
        let methods = session.transport().logged_methods();
        assert_eq!(
            methods
                .iter()
                .filter(|m| *m == "DOM.getNodeForLocation")
                .count(),
            2
        );
        assert_eq!(
            methods
                .iter()
                .filter(|m| *m == "CSS.getComputedStyleForNode")
                .count(),
            2
        );
    }

    #[test]
    fn unscripted_compact_eval_falls_through_to_the_per_node_path() {
        // A fixture that never scripted Runtime.evaluate still works: the
        // probe misses, every element is untagged, the legacy budget runs.
        let script = ScriptBuilder::new().observe(&page()).to_json();
        let mut session = BrowserSession::new(ReplayTransport::parse(&script).unwrap());
        let manifold = session.observe().unwrap();
        assert!(manifold.get_str("n100").is_some());
        assert_eq!(
            session.transport().logged_methods()[0],
            "Page.getLayoutMetrics"
        );
    }

    #[test]
    fn compact_partial_coverage_falls_back_for_the_untagged_node() {
        // Strip data-hu-k from n200: it must take the per-node path while
        // n100 still consumes blob evidence (only fallback calls scripted).
        let mut value: serde_json::Value =
            serde_json::from_str(&ScriptBuilder::new().observe_compact(&page()).to_json()).unwrap();
        let calls = value["calls"].as_array_mut().unwrap();
        let children = calls[2]["result"]["root"]["children"]
            .as_array_mut()
            .unwrap();
        children[1]["attributes"]
            .as_array_mut()
            .unwrap()
            .retain(|attr| attr != &serde_json::json!("data-hu-k"));
        calls.insert(
            5,
            serde_json::json!({
                "method": "DOM.getBoxModel",
                "params": {"nodeId": 20},
                "result": {"model": {"content": [400.0, 200.0, 560.0, 200.0, 560.0, 232.0, 400.0, 232.0]}}
            }),
        );
        calls.insert(
            6,
            serde_json::json!({
                "method": "DOM.getBoxModel",
                "params": {"backendNodeId": 200},
                "result": {"model": {"content": [400.0, 200.0, 560.0, 200.0, 560.0, 232.0, 400.0, 232.0]}}
            }),
        );
        calls.insert(
            8,
            serde_json::json!({"method": "CSS.enable", "params": {}, "result": {}}),
        );
        calls.insert(
            9,
            serde_json::json!({
                "method": "CSS.getComputedStyleForNode",
                "params": {"nodeId": 20},
                "result": {"computedStyle": [
                    {"name": "z-index", "value": "auto"},
                    {"name": "position", "value": "static"},
                    {"name": "opacity", "value": "1"},
                    {"name": "transform", "value": "none"},
                    {"name": "filter", "value": "none"},
                    {"name": "isolation", "value": "auto"},
                    {"name": "mix-blend-mode", "value": "normal"},
                    {"name": "will-change", "value": "auto"},
                    {"name": "pointer-events", "value": "auto"}
                ]}
            }),
        );
        calls.insert(
            10,
            serde_json::json!({
                "method": "DOM.getNodeForLocation",
                "params": {"x": 480, "y": 216},
                "result": {"backendNodeId": 200}
            }),
        );
        let transport = ReplayTransport::parse(&value.to_string()).unwrap();
        let mut session = BrowserSession::new(transport);
        let manifold = session.observe().unwrap();
        assert!(manifold.get_str("n100").is_some());
        assert!(manifold.get_str("n200").is_some());
        let methods = session.transport().logged_methods();
        assert_eq!(methods.len(), 11);
        for method in [
            "DOM.getBoxModel",
            "CSS.getComputedStyleForNode",
            "DOM.getNodeForLocation",
        ] {
            assert!(methods.iter().any(|m| m == method), "missing {method}");
        }
    }

    /// Live transport that refuses the compact eval, forcing the per-node
    /// path on the same page: the legacy oracle for parity.
    struct NoCompact<T>(T);

    impl<T: CdpTransport> CdpTransport for NoCompact<T> {
        fn call(&mut self, method: &str, params_json: &str) -> Result<String, CdpError> {
            if method == "Runtime.evaluate" && params_json.contains(compact::HU_K_ATTR) {
                return Err(CdpError::Protocol {
                    message: "compact eval disabled for parity".into(),
                });
            }
            self.0.call(method, params_json)
        }
    }

    fn live_flags<T: CdpTransport>(
        transport: T,
    ) -> std::collections::BTreeMap<String, (bool, bool)> {
        let mut session = BrowserSession::new(transport);
        session
            .observe()
            .expect("observe")
            .regions()
            .filter(|region| region.actions().contains(&aui_core::Action::Click))
            .map(|region| {
                (
                    region.label().to_owned(),
                    (region.flags().occluded(), region.flags().disabled()),
                )
            })
            .collect()
    }

    #[test]
    #[ignore = "needs live Chrome on fixtures/live/compact-observe.html (served over HTTP); set ULTRA_INSTINCT_CDP=http://127.0.0.1:PORT"]
    fn live_compact_observe_matches_the_per_node_path_on_real_chrome() {
        let endpoint =
            std::env::var("ULTRA_INSTINCT_CDP").unwrap_or_else(|_| DEFAULT_CDP_HTTP.to_owned());
        let compact_flags = live_flags(WebSocketTransport::connect(&endpoint).expect("connect"));
        let legacy_flags = live_flags(NoCompact(
            WebSocketTransport::connect(&endpoint).expect("connect"),
        ));
        let occluded = |label: &str| {
            compact_flags
                .iter()
                .find(|(l, _)| l.contains(label))
                .unwrap_or_else(|| panic!("missing {label}: {compact_flags:?}"))
                .1
                 .0
        };
        // Open shadow content is reachable (ADR 0007), not the host's hit.
        assert!(!occluded("Shadow action"), "{compact_flags:?}");
        assert!(!occluded("Sign in"), "{compact_flags:?}");
        assert!(!occluded("Frame action"), "{compact_flags:?}");
        // A parent-document veil over iframe content buries it.
        assert!(occluded("Frame covered"), "{compact_flags:?}");
        assert!(occluded("Under banner"), "{compact_flags:?}");
        // Compact evidence and the per-node calls agree on every clickable.
        assert_eq!(compact_flags, legacy_flags);
        // The walk left its tags on the live DOM.
        let mut probe = WebSocketTransport::connect(&endpoint).expect("probe");
        let body = probe
            .call(
                "Runtime.evaluate",
                &serde_json::json!({
                    "expression": "document.querySelectorAll('[data-hu-k]').length",
                    "returnByValue": true
                })
                .to_string(),
            )
            .expect("eval");
        let value: serde_json::Value = serde_json::from_str(&body).unwrap();
        assert!(value["result"]["value"].as_i64().unwrap_or(0) > 0, "{body}");
    }

    #[test]
    fn compact_blob_decodes_rects_styles_and_hits() {
        let body = serde_json::json!({
            "result": {"type": "object", "value": {"nodes": {
                "0": {"r": [400.0, 300.0, 80.0, 32.0], "s": [["z-index", "auto"]], "h": 0},
                "1": {"r": null},
                "2": {"r": [0.0, 0.0, 10.0, 10.0], "s": [], "h": null}
            }}}
        });
        let snapshot = compact::parse(&body.to_string()).unwrap();
        let node = snapshot.node(0).unwrap();
        assert_eq!(node.rect.unwrap().x(), 400.0);
        assert_eq!(
            node.style,
            Some(vec![("z-index".to_owned(), "auto".to_owned())])
        );
        assert_eq!(node.hit, compact::Hit::Key(0));
        let bare = snapshot.node(1).unwrap();
        assert!(bare.rect.is_none());
        assert_eq!(bare.style, None);
        assert_eq!(bare.hit, compact::Hit::NotCollected);
        assert_eq!(snapshot.node(2).unwrap().hit, compact::Hit::Nothing);
        assert!(snapshot.node(9).is_none());
    }

    #[test]
    fn compact_hit_above_u32_is_rejected_not_truncated() {
        let body = serde_json::json!({
            "result": {"value": {"nodes": {"0": {"r": null, "h": 4_294_967_296_u64}}}}
        });
        assert!(matches!(
            compact::parse(&body.to_string()),
            Err(BrowserError::Cdp(CdpError::BadJson { .. }))
        ));
    }

    proptest::proptest! {
        #[test]
        fn compact_parse_never_panics_on_arbitrary_text(input in ".{0,256}") {
            let _ = compact::parse(&input);
        }

        #[test]
        fn compact_parse_never_panics_on_arbitrary_records(
            key in "[0-9a-z-]{0,12}",
            rect in proptest::collection::vec(proptest::num::f64::ANY, 0..6),
            hit in proptest::prelude::any::<Option<i64>>(),
            style in proptest::collection::vec((".{0,8}", ".{0,8}"), 0..4),
        ) {
            let body = serde_json::json!({
                "result": {"value": {"nodes": {key: {"r": rect, "s": style, "h": hit}}}}
            });
            let _ = compact::parse(&body.to_string());
        }
    }
}
