//! In-process JSON-RPC and exact tool errors. No Chrome.

use std::path::Path;

use serde_json::{json, Value};
use ultra_instinct_mcp::{call_tool, handle_line, Server, ToolError, TOOLS};

fn fixture(name: &str) -> String {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../fixtures")
        .join(name)
        .to_str()
        .unwrap()
        .to_owned()
}

fn call(name: &str, arguments: Value) -> Result<Value, ToolError> {
    call_tool(name, &arguments)
}

fn rpc(line: &str) -> Value {
    let response = handle_line(line).expect("response");
    serde_json::from_str(&response).expect("json")
}

fn tool_text(response: &Value) -> Value {
    let text = response["result"]["content"][0]["text"]
        .as_str()
        .expect("text");
    serde_json::from_str(text).expect("tool json")
}

#[test]
fn tools_list_includes_product_and_legacy_verbs() {
    let response = rpc(r#"{"jsonrpc":"2.0","id":2,"method":"tools/list"}"#);
    assert_eq!(response["id"], 2);
    let names: Vec<&str> = response["result"]["tools"]
        .as_array()
        .unwrap()
        .iter()
        .map(|tool| tool["name"].as_str().unwrap())
        .collect();
    assert_eq!(names, TOOLS);
    assert!(names.contains(&"observe"));
    assert!(names.contains(&"guard"));
    assert!(names.contains(&"verify"));
    assert!(names.contains(&"act")); // deprecated alias of guard
    assert!(!names.contains(&"navigate"));
    let guard = response["result"]["tools"]
        .as_array()
        .unwrap()
        .iter()
        .find(|tool| tool["name"] == "guard")
        .unwrap();
    let description = guard["description"].as_str().unwrap();
    assert!(description.contains("Never clicks"), "{description}");
    let init = rpc(
        r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2024-11-05","capabilities":{},"clientInfo":{"name":"jev","version":"0"}}}"#,
    );
    assert_eq!(init["result"]["serverInfo"]["name"], "ultra-instinct");
    assert_eq!(init["result"]["protocolVersion"], "2024-11-05");
    let instructions = init["result"]["instructions"].as_str().unwrap();
    assert!(instructions.contains("action firewall"), "{instructions}");
    assert!(
        instructions.contains("never clicks") || instructions.contains("ActionTicket"),
        "{instructions}"
    );
    assert!(handle_line(r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#).is_none());
}

#[ignore = "actuation removed in action-firewall pivot"]
#[test]
fn locate_defaults_to_weighted_and_hgra_is_not_a_benchmark() {
    let sign_in = fixture("sign-in.cdp.json");
    let body = call("locate", json!({"fixture": sign_in, "text": "Sign in"})).unwrap();
    assert_eq!(body["product"], "ultra-instinct");
    assert_eq!(body["tool"], "locate");
    assert_eq!(body["matcher"], "weighted");
    assert_eq!(body["benchmark"], false);
    assert!(body.get("executed").is_none(), "{body}");
    assert!(body.get("verified").is_none(), "{body}");
    assert!(body.get("mechanism").is_none(), "{body}");
    assert!(body["action"].is_null());
    assert_eq!(body["target"]["id"], "n100");
    assert_eq!(body["target"]["label"], "Sign in");
    assert!(body["target"]["role"].is_string());
    assert!(body["confidence"].as_f64().unwrap().is_finite());
    assert_eq!(body["candidates"][0]["id"], "n100");
    assert_eq!(body["candidates"][0]["rank"].as_u64(), Some(1));
    assert!(body["state_delta"]["added"].as_array().unwrap().is_empty());

    let sidebar = fixture("sidebar.manifold");
    let hgra = call(
        "locate",
        json!({
            "fixture": sidebar,
            "text": "Settings",
            "role": "button",
            "position": "left",
            "matcher": "hgra"
        }),
    )
    .unwrap();
    assert_eq!(hgra["matcher"], "hgra");
    assert_eq!(hgra["benchmark"], false);
    assert_eq!(hgra["target"]["id"], "nav-settings");
    assert!(hgra.get("executed").is_none(), "{hgra}");
    assert!(hgra.get("verified").is_none(), "{hgra}");
    assert!(hgra.get("mechanism").is_none(), "{hgra}");
}

#[test]
fn observe_inspect_diff_and_verify_round_trip() {
    let observed = call("observe", json!({"fixture": fixture("sign-in.cdp.json")})).unwrap();
    assert_eq!(observed["tool"], "observe");
    assert_eq!(observed["executed"], false);
    assert_eq!(observed["verified"], false);
    let ids: Vec<&str> = observed["regions"]
        .as_array()
        .unwrap()
        .iter()
        .map(|region| region["id"].as_str().unwrap())
        .collect();
    assert!(ids.contains(&"n100"), "{ids:?}");

    let inspected = call(
        "inspect",
        json!({"fixture": fixture("sign-in.cdp.json"), "region": "n100"}),
    )
    .unwrap();
    assert_eq!(inspected["target"]["id"], "n100");
    assert_eq!(inspected["target"]["label"], "Sign in");
    assert!(inspected["target"]["width"].as_f64().unwrap() > 0.0);
    assert!(inspected.get("executed").is_none(), "{inspected}");
    assert!(inspected.get("verified").is_none(), "{inspected}");
    assert!(inspected.get("mechanism").is_none(), "{inspected}");

    let delta = call(
        "diff",
        json!({
            "before": fixture("sign-in.cdp.json"),
            "after": fixture("welcome.cdp.json")
        }),
    )
    .unwrap();
    assert_eq!(delta["tool"], "diff");
    assert_eq!(delta["executed"], false);
    let removed = &delta["state_delta"]["removed"];
    let added = &delta["state_delta"]["added"];
    assert!(removed.as_array().unwrap().iter().any(|id| id == "n100"));
    assert!(added.as_array().unwrap().iter().any(|id| id == "n300"));

    let verified = call(
        "verify",
        json!({"fixture": fixture("welcome.cdp.json"), "expect_text": "Welcome"}),
    )
    .unwrap();
    assert_eq!(verified["verified"], true);
    assert_eq!(verified["executed"], false);
}

#[ignore = "actuation removed in action-firewall pivot"]
#[test]
fn act_press_uses_dom_click_and_low_confidence_does_not() {
    let pressed = call(
        "act",
        json!({
            "fixture": fixture("sign-in-press.cdp.json"),
            "region": "n100",
            "action": "press"
        }),
    )
    .unwrap();
    assert_eq!(pressed["executed"], true);
    assert_eq!(pressed["verified"], false);
    assert_eq!(pressed["action"], "click");
    assert_eq!(pressed["executor"], "browser");
    assert_eq!(pressed["mechanism"], "dom-semantic");
    assert_eq!(pressed["target"]["id"], "n100");
    assert_eq!(pressed["target"]["label"], "Sign in");
    assert!(pressed["fallback"].is_null());

    // sign-in.cdp.json has no press responses. A click would be a browser error.
    let refused = call(
        "act",
        json!({
            "fixture": fixture("sign-in.cdp.json"),
            "region": "n100",
            "confidence": 0.49
        }),
    )
    .unwrap();
    assert_eq!(refused["executed"], false);
    assert_eq!(refused["fallback"], "low-confidence");
    assert_eq!(refused["executor"], Value::Null);
    assert!(refused["mechanism"].is_null());
    assert_eq!(refused["target"]["id"], "n100");
    assert!(!refused["target"]["role"].as_str().unwrap().is_empty());
    assert!((refused["confidence"].as_f64().unwrap() - 0.49).abs() < 1e-9);
    assert_eq!(refused["action"], "click");
    assert_eq!(refused["state_delta"]["added"].as_array().unwrap().len(), 0);
}

#[ignore = "actuation removed in action-firewall pivot"]
#[test]
fn browser_use_act_is_semantic_and_low_confidence_does_not_execute() {
    let pressed = call(
        "act",
        json!({
            "fixture": fixture("sign-in.browser-use.json"),
            "region": "n100",
            "action": "press",
            "executor": "browser-use",
            "confidence": 0.55
        }),
    )
    .unwrap();
    assert_eq!(pressed["executed"], true);
    assert_eq!(pressed["executor"], "browser-use");
    assert_eq!(pressed["mechanism"], "browser-use-semantic");
    assert_eq!(pressed["target"]["id"], "n100");
    assert_eq!(pressed["target"]["role"], "button");
    assert_eq!(pressed["target"]["label"], "Sign in");
    assert_eq!(pressed["action"], "click");
    assert!(pressed.get("x").is_none());
    assert!(pressed.get("goal").is_none());
    assert!(pressed["target"].get("x").is_none());

    let refused = call(
        "act",
        json!({
            "fixture": fixture("sign-in-reject.browser-use.json"),
            "region": "n100",
            "executor": "browser-use",
            "confidence": 0.49
        }),
    )
    .unwrap();
    assert_eq!(refused["executed"], false);
    assert_eq!(refused["fallback"], "low-confidence");
    assert_eq!(refused["executor"], Value::Null);
    assert!(refused["mechanism"].is_null());

    let err = call(
        "act",
        json!({
            "fixture": fixture("sign-in-reject.browser-use.json"),
            "region": "n100",
            "executor": "browser-use"
        }),
    )
    .unwrap_err();
    assert_eq!(
        err,
        ToolError::BrowserUseRejected {
            message: "control refused the semantic act".into(),
        }
    );
    assert_eq!(
        err.to_value(),
        json!({
            "variant": "BrowserUseRejected",
            "message": "control refused the semantic act"
        })
    );

    let err = call("act", json!({"region": "n100", "executor": "macos"})).unwrap_err();
    assert_eq!(
        err,
        ToolError::NotImplemented {
            executor: "macos".into(),
        }
    );
    assert_eq!(err.to_string(), "macos executor is not implemented");

    let err = call(
        "act",
        json!({
            "fixture": fixture("sign-in.browser-use.json"),
            "region": "n100",
            "executor": "browser-use",
            "cdp": "http://127.0.0.1:9222"
        }),
    )
    .unwrap_err();
    assert_eq!(err, ToolError::BrowserUseIsReplay);

    let err = call(
        "act",
        json!({
            "fixture": fixture("sign-in.browser-use.json"),
            "region": "n100",
            "executor": "browser-use",
            "goal": "sign the user in"
        }),
    )
    .unwrap_err();
    assert_eq!(err, ToolError::GoalNotAccepted);
}

#[ignore = "actuation removed in action-firewall pivot"]
#[test]
fn cua_act_is_semantic_and_low_confidence_does_not_execute() {
    let pressed = call(
        "act",
        json!({
            "fixture": fixture("sign-in.cua.json"),
            "region": "n100",
            "action": "press",
            "executor": "cua",
            "confidence": 0.55
        }),
    )
    .unwrap();
    assert_eq!(pressed["executed"], true);
    assert_eq!(pressed["executor"], "cua");
    assert_eq!(pressed["mechanism"], "cua-semantic");
    assert_eq!(pressed["target"]["id"], "n100");
    assert_eq!(pressed["target"]["role"], "button");
    assert_eq!(pressed["target"]["label"], "Sign in");
    assert_eq!(pressed["action"], "click");
    assert!(pressed.get("x").is_none());
    assert!(pressed.get("goal").is_none());
    assert!(pressed["target"].get("x").is_none());

    let refused = call(
        "act",
        json!({
            "fixture": fixture("sign-in-reject.cua.json"),
            "region": "n100",
            "executor": "cua",
            "confidence": 0.49
        }),
    )
    .unwrap();
    assert_eq!(refused["executed"], false);
    assert_eq!(refused["fallback"], "low-confidence");
    assert_eq!(refused["executor"], Value::Null);
    assert!(refused["mechanism"].is_null());

    let err = call(
        "act",
        json!({
            "fixture": fixture("sign-in-reject.cua.json"),
            "region": "n100",
            "executor": "cua"
        }),
    )
    .unwrap_err();
    assert_eq!(
        err,
        ToolError::CuaRejected {
            message: "control refused the semantic act".into(),
        }
    );
    assert_eq!(
        err.to_value(),
        json!({
            "variant": "CuaRejected",
            "message": "control refused the semantic act"
        })
    );
    assert_eq!(
        err.to_string(),
        "cua rejected the semantic act: control refused the semantic act"
    );

    let err = call(
        "act",
        json!({
            "fixture": fixture("sign-in.cua.json"),
            "region": "n100",
            "executor": "cua",
            "cdp": "http://127.0.0.1:9222"
        }),
    )
    .unwrap_err();
    assert_eq!(err, ToolError::CuaIsReplay);

    let err = call(
        "act",
        json!({
            "fixture": fixture("sign-in.cua.json"),
            "region": "n100",
            "executor": "cua",
            "goal": "sign the user in"
        }),
    )
    .unwrap_err();
    assert_eq!(err, ToolError::GoalNotAccepted);

    let default_act = call(
        "act",
        json!({
            "fixture": fixture("sign-in.cua.json"),
            "region": "n100"
        }),
    )
    .unwrap_err();
    assert_eq!(
        default_act,
        ToolError::Browser(
            "invalid CDP script: script must be an object with a calls array".into(),
        )
    );
    assert_eq!(
        default_act.to_string(),
        "browser: invalid CDP script: script must be an object with a calls array"
    );
}

#[ignore = "actuation removed in action-firewall pivot"]
#[test]
fn error_variants_are_exact() {
    let sign_in = fixture("sign-in.cdp.json");
    let err = call("observe", json!({})).unwrap_err();
    assert_eq!(err, ToolError::MissingFixture);
    assert_eq!(err.to_value(), json!({"variant": "MissingFixture"}));
    assert_eq!(err.to_string(), "tool requires fixture or cdp");

    let err = call(
        "locate",
        json!({"fixture": &sign_in, "cdp": "http://127.0.0.1:9222"}),
    )
    .unwrap_err();
    assert_eq!(err, ToolError::DuplicateSource);

    let err = call("fly", json!({})).unwrap_err();
    assert_eq!(err, ToolError::UnknownTool("fly".into()));
    assert_eq!(
        err.to_value(),
        json!({"variant": "UnknownTool", "name": "fly"})
    );

    let err = call("navigate", json!({})).unwrap_err();
    assert_eq!(err, ToolError::GoalNotAccepted);
    assert_eq!(
        err.to_string(),
        "ultra-instinct does not accept a goal or navigate"
    );

    let err = call(
        "locate",
        json!({"fixture": &sign_in, "goal": "sign the user in"}),
    )
    .unwrap_err();
    assert_eq!(err, ToolError::GoalNotAccepted);

    let err = call(
        "act",
        json!({"fixture": &sign_in, "region": "n100", "x": 12, "y": 8}),
    )
    .unwrap_err();
    assert_eq!(err, ToolError::CoordinatesNotAccepted);

    let err = call("locate", json!({"fixture": &sign_in, "matcher": "average"})).unwrap_err();
    assert_eq!(err, ToolError::UnknownMatcher("average".into()));

    let err = call("locate", json!({"fixture": &sign_in, "text": "..."})).unwrap_err();
    assert_eq!(err, ToolError::EmptyText);
    assert_eq!(
        err.to_string(),
        "text must contain at least one alphanumeric token"
    );

    let err = call("locate", json!({"fixture": &sign_in, "role": "spaceship"})).unwrap_err();
    assert_eq!(err, ToolError::UnknownRole("spaceship".into()));

    let err = call("locate", json!({"fixture": &sign_in, "position": "orbit"})).unwrap_err();
    assert_eq!(err, ToolError::UnknownPosition("orbit".into()));

    let err = call(
        "act",
        json!({"fixture": &sign_in, "region": "n100", "action": "fly"}),
    )
    .unwrap_err();
    assert_eq!(err, ToolError::UnknownAction("fly".into()));

    let err = call(
        "act",
        json!({"fixture": &sign_in, "region": "n100", "action": "type"}),
    )
    .unwrap_err();
    assert_eq!(err, ToolError::UnsupportedAction("type".into()));
    assert_eq!(err.to_string(), "browser session cannot perform `type`");

    let err = call(
        "locate",
        json!({"fixture": &sign_in, "matcher": "weighted", "dims": 512}),
    )
    .unwrap_err();
    assert_eq!(err, ToolError::DimsRequireHgra);

    let err = call(
        "locate",
        json!({"fixture": &sign_in, "matcher": "hgra", "dims": 100}),
    )
    .unwrap_err();
    assert_eq!(err, ToolError::BadDims("100".into()));

    let err = call(
        "act",
        json!({"fixture": "unused", "region": "n100", "confidence": "NaN"}),
    )
    .unwrap_err();
    assert_eq!(err, ToolError::NonFiniteConfidence);
    assert_eq!(err.to_string(), "confidence must be finite");

    let err = call(
        "act",
        json!({"fixture": "unused", "region": "n100", "confidence": "nope"}),
    )
    .unwrap_err();
    assert_eq!(err, ToolError::BadConfidence("nope".into()));

    let err = call("act", json!({"fixture": &sign_in})).unwrap_err();
    assert_eq!(err, ToolError::MissingRegion);

    let err = call("inspect", json!({"fixture": &sign_in, "region": "missing"})).unwrap_err();
    assert_eq!(err, ToolError::UnknownRegion("missing".into()));

    let err = call(
        "act",
        json!({"fixture": fixture("sidebar.manifold"), "region": "nav-settings"}),
    )
    .unwrap_err();
    assert_eq!(err, ToolError::ActNeedsCdp);

    let err = call(
        "verify",
        json!({"fixture": &sign_in, "expect_text": "Welcome"}),
    )
    .unwrap_err();
    assert_eq!(
        err,
        ToolError::ExpectedTextMissing {
            expected: "Welcome".into()
        }
    );
    assert_eq!(err.to_string(), "expected text `Welcome` did not appear");
    assert_eq!(
        err.to_value(),
        json!({"variant": "ExpectedTextMissing", "expected": "Welcome"})
    );

    let err = call(
        "verify",
        json!({"fixture": &sign_in, "expect_absent": "n100"}),
    )
    .unwrap_err();
    assert_eq!(err, ToolError::RegionStillPresent { id: "n100".into() });
    assert_eq!(err.to_string(), "region `n100` is still present");

    let err = call("verify", json!({"fixture": &sign_in})).unwrap_err();
    assert_eq!(err, ToolError::MissingExpect);

    let err = call(
        "verify",
        json!({
            "fixture": &sign_in,
            "expect_text": "Welcome",
            "expect_absent": "n100"
        }),
    )
    .unwrap_err();
    assert_eq!(err, ToolError::BothExpectations);

    let err = call("diff", json!({})).unwrap_err();
    assert_eq!(err, ToolError::MissingBefore);
    let err = call("diff", json!({"before": &sign_in})).unwrap_err();
    assert_eq!(err, ToolError::MissingAfter);

    let missing = "/workspace/ultra-instinct/fixtures/does-not-exist.cdp.json";
    let err = call("observe", json!({"fixture": missing})).unwrap_err();
    let ToolError::Io { path, message } = err.clone() else {
        panic!("expected Io, got {err:?}");
    };
    assert_eq!(path, missing);
    assert!(!message.is_empty());
    assert_eq!(err, ToolError::Io { path, message });

    let err = call("observe", json!({"fixture": 1})).unwrap_err();
    assert_eq!(
        err,
        ToolError::InvalidArguments("fixture must be a string".into())
    );

    let err = call("observe", json!([1])).unwrap_err();
    assert_eq!(
        err,
        ToolError::InvalidArguments("arguments must be an object".into())
    );
}

#[test]
fn json_rpc_reports_tool_errors_and_protocol_errors() {
    let missing = rpc(
        r#"{"jsonrpc":"2.0","id":9,"method":"tools/call","params":{"name":"observe","arguments":{}}}"#,
    );
    assert_eq!(missing["id"], 9);
    assert_eq!(missing["result"]["isError"], true);
    assert!(missing.get("error").is_none());
    assert_eq!(tool_text(&missing)["variant"], "MissingFixture");

    let navigate = rpc(
        r#"{"jsonrpc":"2.0","id":"nav","method":"tools/call","params":{"name":"navigate","arguments":{}}}"#,
    );
    assert_eq!(navigate["id"], "nav");
    assert_eq!(tool_text(&navigate)["variant"], "GoalNotAccepted");

    let bad = rpc("not-json");
    assert_eq!(bad["error"]["code"], -32700);
    assert_eq!(bad["error"]["data"]["variant"], "ParseError");
    assert!(bad["id"].is_null());

    let batch = rpc("[]");
    assert_eq!(batch["error"]["data"]["variant"], "InvalidRequest");

    let unknown = rpc(r#"{"jsonrpc":"2.0","id":3,"method":"resources/list"}"#);
    assert_eq!(unknown["error"]["code"], -32601);
    assert_eq!(unknown["error"]["data"]["variant"], "MethodNotFound");
    assert_eq!(unknown["error"]["data"]["method"], "resources/list");

    let unnamed = rpc(r#"{"jsonrpc":"2.0","id":4,"method":"tools/call","params":{}}"#);
    assert_eq!(unnamed["error"]["data"]["variant"], "MissingToolName");
    assert_eq!(unnamed["error"]["code"], -32602);
}

#[ignore = "actuation removed in action-firewall pivot"]
#[test]
fn above_threshold_press_still_uses_the_dom_click() {
    let pressed = call(
        "act",
        json!({
            "fixture": fixture("sign-in-press.cdp.json"),
            "region": "n100",
            "confidence": 0.55
        }),
    )
    .unwrap();
    assert_eq!(pressed["executed"], true);
    assert_eq!(pressed["mechanism"], "dom-semantic");
    assert!((pressed["confidence"].as_f64().unwrap() - 0.55).abs() < 1e-9);
}

#[ignore = "actuation removed in action-firewall pivot"]
#[test]
fn act_refuses_an_ambiguous_ranked_target() {
    // sign-in.cdp.json has no press steps: a press attempt would fail with
    // NoScriptedResponse, so a successful refusal proves nothing was sent.
    let refused = call(
        "act",
        json!({
            "fixture": fixture("sign-in.cdp.json"),
            "region": "n100",
            "confidence": 1.0,
            "runner_up": {"id": "n200", "confidence": 0.98}
        }),
    )
    .unwrap();
    assert_eq!(refused["executed"], false);
    assert_eq!(refused["fallback"], "ambiguous");
    assert_eq!(refused["mechanism"], Value::Null);
    assert_eq!(refused["margin_millis"], 20);
    assert_eq!(refused["runner_up"]["id"], "n200");
    assert_eq!(refused["runner_up"]["label"], "Cancel");
    assert_eq!(refused["target"]["id"], "n100");

    let pressed = call(
        "act",
        json!({
            "fixture": fixture("sign-in-press.cdp.json"),
            "region": "n100",
            "confidence": 1.0,
            "runner_up": {"id": "n200", "confidence": 0.5}
        }),
    )
    .unwrap();
    assert_eq!(pressed["executed"], true);
    assert_eq!(pressed["mechanism"], "dom-semantic");
}

#[ignore = "actuation removed in action-firewall pivot"]
#[test]
fn runner_up_without_confidence_is_exact() {
    let err = call(
        "act",
        json!({
            "fixture": fixture("sign-in-press.cdp.json"),
            "region": "n100",
            "runner_up": {"id": "n200", "confidence": 0.98}
        }),
    )
    .unwrap_err();
    assert_eq!(err, ToolError::RunnerUpNeedsConfidence);
    assert_eq!(
        err.to_value(),
        json!({"variant": "RunnerUpNeedsConfidence"})
    );
}

#[ignore = "actuation removed in action-firewall pivot"]
#[test]
fn runner_up_equal_to_region_is_exact() {
    let err = call(
        "act",
        json!({
            "fixture": fixture("sign-in-press.cdp.json"),
            "region": "n100",
            "confidence": 1.0,
            "runner_up": {"id": "n100", "confidence": 0.2}
        }),
    )
    .unwrap_err();
    assert_eq!(err, ToolError::RunnerUpIsTarget);
    let err = call(
        "act",
        json!({
            "fixture": fixture("sign-in-press.cdp.json"),
            "region": "n100",
            "confidence": 1.0,
            "runner_up": "n200"
        }),
    )
    .unwrap_err();
    assert_eq!(
        err,
        ToolError::BadRunnerUp("runner_up must be an object".into())
    );
}

#[ignore = "actuation removed in action-firewall pivot"]
#[test]
fn browser_use_and_cua_refuse_an_ambiguous_ranked_target() {
    for (executor, script) in [
        ("browser-use", "sign-in.browser-use.json"),
        ("cua", "sign-in.cua.json"),
    ] {
        let refused = call(
            "act",
            json!({
                "fixture": fixture(script),
                "region": "n100",
                "executor": executor,
                "confidence": 0.9,
                "runner_up": {"id": "n200", "confidence": 0.88}
            }),
        )
        .unwrap();
        assert_eq!(refused["executed"], false, "{executor}");
        assert_eq!(refused["fallback"], "ambiguous", "{executor}");
        assert_eq!(refused["margin_millis"], 20, "{executor}");
    }
}

#[ignore = "actuation removed in action-firewall pivot"]
#[test]
fn act_closed_loop_reports_delta_and_verifies_welcome() {
    let body = call(
        "act",
        json!({
            "fixture": fixture("sign-in-loop.cdp.json"),
            "region": "n100",
            "expect_text": "Welcome"
        }),
    )
    .unwrap();
    assert_eq!(body["executed"], true);
    assert_eq!(body["verified"], true);
    assert_eq!(body["fallback"], Value::Null);
    assert_eq!(body["mechanism"], "dom-semantic");
    assert_eq!(
        body["state_delta"],
        json!({
            "added": ["n300"],
            "removed": ["n100", "n200"],
            "changed": [],
            "moved": [],
            "text_changed": [],
            "focus_changed": false,
            "url_changed": true,
            "title_changed": true
        })
    );
    assert_eq!(body["before_snapshot"], 1);
    assert_eq!(body["after_snapshot"], 2);
    assert_eq!(body["signals"], json!([]));
}

#[ignore = "actuation removed in action-firewall pivot"]
#[test]
fn act_closed_loop_verify_failure_is_executed_true_verified_false() {
    let body = call(
        "act",
        json!({
            "fixture": fixture("sign-in-noop.cdp.json"),
            "region": "n100",
            "expect_absent": "n100"
        }),
    )
    .unwrap();
    assert_eq!(body["executed"], true);
    assert_eq!(body["verified"], false);
    assert_eq!(body["fallback"], "verify-failed");
    assert_eq!(
        body["verify_error"],
        json!({"variant": "RegionStillPresent", "id": "n100"})
    );
    assert_eq!(
        body["state_delta"],
        json!({
            "added": [],
            "removed": [],
            "changed": [],
            "moved": [],
            "text_changed": [],
            "focus_changed": false,
            "url_changed": false,
            "title_changed": false
        })
    );
    assert_eq!(body["signals"], json!([{"kind": "no-op"}]));
    let bare = call(
        "act",
        json!({
            "fixture": fixture("sign-in-noop.cdp.json"),
            "region": "n100",
            "observe_after": true
        }),
    )
    .unwrap();
    assert_eq!(bare["executed"], true);
    assert_eq!(bare["fallback"], "no-effect");
    assert_eq!(bare["signals"], json!([{"kind": "no-op"}]));
}

#[ignore = "actuation removed in action-firewall pivot"]
#[test]
fn act_without_expectation_on_a_fixture_does_not_observe_after() {
    let body = call(
        "act",
        json!({"fixture": fixture("sign-in-press.cdp.json"), "region": "n100"}),
    )
    .unwrap();
    assert_eq!(body["executed"], true);
    assert_eq!(body["before_snapshot"], 1);
    assert_eq!(body["after_snapshot"], Value::Null);
    assert_eq!(body["signals"], json!([]));
    let err = call(
        "act",
        json!({
            "fixture": fixture("sign-in-loop.cdp.json"),
            "region": "n100",
            "expect_text": "Welcome",
            "observe_after": false
        }),
    )
    .unwrap_err();
    assert_eq!(err, ToolError::ExpectNeedsObserveAfter);
}

#[ignore = "actuation removed in action-firewall pivot"]
#[test]
fn act_with_locate_fields_derives_the_ranked_gate() {
    let body = call(
        "act",
        json!({
            "fixture": fixture("sign-in-loop.cdp.json"),
            "region": "n100",
            "text": "Sign in",
            "expect_text": "Welcome"
        }),
    )
    .unwrap();
    assert_eq!(body["executed"], true);
    assert_eq!(body["verified"], true);
    assert_eq!(body["confidence"], 1.0);

    let err = call(
        "act",
        json!({
            "fixture": fixture("sign-in-loop.cdp.json"),
            "region": "n200",
            "text": "Sign in"
        }),
    )
    .unwrap_err();
    assert_eq!(
        err,
        ToolError::TargetNotTop {
            region: "n200".into(),
            top: "n100".into()
        }
    );
    let err = call(
        "act",
        json!({
            "fixture": fixture("sign-in-loop.cdp.json"),
            "region": "n100",
            "text": "Sign in",
            "confidence": 1.0
        }),
    )
    .unwrap_err();
    assert_eq!(err, ToolError::ConfidenceWithQuery);
}

#[test]
fn diff_by_snapshot_ids_matches_diff_by_paths() {
    let mut server = Server::new();
    let before = server
        .call_tool("observe", &json!({"fixture": fixture("sign-in.cdp.json")}))
        .unwrap();
    let after = server
        .call_tool("observe", &json!({"fixture": fixture("welcome.cdp.json")}))
        .unwrap();
    assert_eq!(before["snapshot"], 1);
    assert_eq!(after["snapshot"], 2);
    let by_id = server
        .call_tool("diff", &json!({"before_snapshot": 1, "after_snapshot": 2}))
        .unwrap();
    let by_path = server
        .call_tool(
            "diff",
            &json!({
                "before": fixture("sign-in.cdp.json"),
                "after": fixture("welcome.cdp.json")
            }),
        )
        .unwrap();
    assert_eq!(by_id["state_delta"], by_path["state_delta"]);
    assert_eq!(by_id["state_delta"]["added"], json!(["n300"]));

    let err = server
        .call_tool(
            "diff",
            &json!({"before_snapshot": 1, "after": fixture("welcome.cdp.json")}),
        )
        .unwrap_err();
    assert_eq!(err, ToolError::MixedDiffSources);
    let err = server
        .call_tool("diff", &json!({"before_snapshot": 1}))
        .unwrap_err();
    assert_eq!(err, ToolError::MissingAfter);
    let err = server
        .call_tool("diff", &json!({"before_snapshot": 1, "after_snapshot": 9}))
        .unwrap_err();
    assert_eq!(err, ToolError::UnknownSnapshot(9));
    assert_eq!(
        err.to_value(),
        json!({"variant": "UnknownSnapshot", "id": 9})
    );
}

#[test]
fn evicted_snapshot_is_exact() {
    let mut server = Server::new();
    for _ in 0..17 {
        server
            .call_tool("observe", &json!({"fixture": fixture("sign-in.cdp.json")}))
            .unwrap();
    }
    let err = server
        .call_tool("diff", &json!({"before_snapshot": 1, "after_snapshot": 17}))
        .unwrap_err();
    assert_eq!(err, ToolError::SnapshotEvicted { id: 1, oldest: 2 });
    assert_eq!(err.to_string(), "snapshot 1 was evicted; oldest kept is 2");
}

#[test]
fn stateless_call_tool_is_unchanged() {
    // The free function uses a fresh server, so every call is snapshot 1.
    let first = call("observe", json!({"fixture": fixture("sign-in.cdp.json")})).unwrap();
    let second = call("observe", json!({"fixture": fixture("sign-in.cdp.json")})).unwrap();
    assert_eq!(first["snapshot"], 1);
    assert_eq!(second["snapshot"], 1);
    let err = call("diff", json!({"before_snapshot": 1, "after_snapshot": 1})).unwrap_err();
    assert_eq!(err, ToolError::UnknownSnapshot(1));
}

#[ignore = "actuation removed in action-firewall pivot"]
#[test]
fn caller_confidence_outside_zero_to_one_is_exact() {
    let err = call(
        "act",
        json!({
            "fixture": fixture("sign-in-press.cdp.json"),
            "region": "n100",
            "confidence": 1e7,
            "runner_up": {"id": "n200", "confidence": 0.2}
        }),
    )
    .unwrap_err();
    assert_eq!(err, ToolError::ConfidenceOutOfRange("10000000.0".into()));
    assert_eq!(
        err.to_value(),
        json!({"variant": "ConfidenceOutOfRange", "value": "10000000.0"})
    );
    assert_eq!(
        err.to_string(),
        "confidence `10000000.0` must be between 0 and 1"
    );
    let err = call(
        "act",
        json!({
            "fixture": fixture("sign-in-press.cdp.json"),
            "region": "n100",
            "confidence": 0.9,
            "runner_up": {"id": "n200", "confidence": -1e7}
        }),
    )
    .unwrap_err();
    assert_eq!(err, ToolError::ConfidenceOutOfRange("-10000000.0".into()));
    let err = call(
        "act",
        json!({
            "fixture": fixture("sign-in-press.cdp.json"),
            "region": "n100",
            "confidence": "1.5"
        }),
    )
    .unwrap_err();
    assert_eq!(err, ToolError::ConfidenceOutOfRange("1.5".into()));
}

#[ignore = "actuation removed in action-firewall pivot"]
#[test]
fn raw_confidence_just_below_the_gate_does_not_press() {
    let body = call(
        "act",
        json!({
            "fixture": fixture("sign-in-press.cdp.json"),
            "region": "n100",
            "confidence": 0.5496
        }),
    )
    .unwrap();
    assert_eq!(body["executed"], false);
    assert_eq!(body["fallback"], "low-confidence");
    assert_eq!(body["mechanism"], Value::Null);
}

#[ignore = "actuation removed in action-firewall pivot"]
#[test]
fn remaining_tool_errors_are_exact() {
    let err = call(
        "act",
        json!({
            "fixture": fixture("sign-in.cdp.json"),
            "region": "n100",
            "executor": "nope"
        }),
    )
    .unwrap_err();
    assert_eq!(err, ToolError::UnknownExecutor("nope".into()));
    assert_eq!(
        err.to_value(),
        json!({"variant": "UnknownExecutor", "name": "nope"})
    );

    let err = call(
        "diff",
        json!({
            "before_snapshot": -1,
            "after_snapshot": 1
        }),
    )
    .unwrap_err();
    assert_eq!(err, ToolError::BadSnapshot("-1".into()));
    assert_eq!(
        err.to_value(),
        json!({"variant": "BadSnapshot", "value": "-1"})
    );

    let err = call(
        "act",
        json!({
            "fixture": fixture("sign-in.cdp.json"),
            "region": "n100",
            "executor": "browser-use"
        }),
    )
    .unwrap_err();
    assert!(matches!(err, ToolError::BrowserUseScript { .. }), "{err:?}");

    let err = call(
        "act",
        json!({
            "fixture": fixture("sign-in.cdp.json"),
            "region": "n100",
            "executor": "cua"
        }),
    )
    .unwrap_err();
    assert!(matches!(err, ToolError::CuaScript { .. }), "{err:?}");

    let bad = std::env::temp_dir().join("ultra-instinct-bad.manifold");
    std::fs::write(&bad, "not a manifold\n").unwrap();
    let err = call("observe", json!({"fixture": bad.to_str().unwrap()})).unwrap_err();
    assert!(matches!(err, ToolError::Fixture(_)), "{err:?}");
}

#[ignore = "actuation removed in action-firewall pivot"]
#[test]
fn stale_before_is_not_reused_after_a_press_without_observe_after() {
    use ultra_instinct_browser::script::{AxSpec, DomSpec, PageSpec, ScriptBuilder};

    let before = PageSpec::new(
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
    let after_press = PageSpec::new(
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
    // observe (first act, observe_after false) + DOM click + observe (second act) + DOM click
    // + observe (observe_after true on second) .
    let script = ScriptBuilder::new()
        .observe(&before)
        .dom_click(10)
        .observe(&after_press)
        .dom_click(20)
        .observe(&after_press)
        .to_json();
    let path = std::env::temp_dir().join("ultra-instinct-stale-before.cdp.json");
    std::fs::write(&path, &script).unwrap();
    let path = path.to_str().unwrap();

    let mut server = Server::new();
    let first = server
        .call_tool(
            "act",
            &json!({
                "fixture": path,
                "region": "n100",
                "observe_after": false
            }),
        )
        .unwrap();
    assert_eq!(first["executed"], true);
    assert_eq!(first["after_snapshot"], Value::Null);

    // A second act on the same fixture rebuilds a fresh session (fixture
    // origins do not keep a transport). Use a live-shaped path by observing
    // through one Server with a CDP fixture twice is not live. Exercise the
    // session API directly instead.
    use ultra_instinct_browser::{BrowserSession, ReplayTransport};
    let mut session = BrowserSession::new(ReplayTransport::parse(&script).unwrap());
    session.observe().unwrap();
    assert!(!session.is_stale());
    assert!(session.fresh_manifold().is_some());
    session
        .press(
            &ultra_instinct_core::RegionId::try_new("n100").unwrap(),
            ultra_instinct_core::Action::Click,
        )
        .unwrap();
    assert!(session.is_stale());
    assert!(session.fresh_manifold().is_none());
    // A new observe clears the flag.
    session.observe().unwrap();
    assert!(!session.is_stale());
    assert_eq!(
        session.manifold().unwrap().get_str("n200").unwrap().label(),
        "Welcome"
    );
}

#[test]
fn observe_alone_tells_the_disabled_save_from_the_enabled_one() {
    let observed = call(
        "observe",
        json!({"fixture": fixture("settings-saves.manifold")}),
    )
    .unwrap();
    let rows: Vec<(&str, &str, &str, &str, &str)> = observed["regions"]
        .as_array()
        .unwrap()
        .iter()
        .map(|region| {
            (
                region["id"].as_str().unwrap(),
                region["role"].as_str().unwrap(),
                region["label"].as_str().unwrap(),
                region["state"]["availability"].as_str().unwrap(),
                region["state"]["visibility"].as_str().unwrap(),
            )
        })
        .collect();
    assert_eq!(
        rows,
        [
            ("a-save-vacation", "button", "Save", "disabled", "visible"),
            ("b-save-footer", "button", "Save", "enabled", "offscreen"),
            ("z-save-signature", "button", "Save", "enabled", "visible"),
        ]
    );

    let inspected = call(
        "inspect",
        json!({"fixture": fixture("settings-saves.manifold"), "region": "a-save-vacation"}),
    )
    .unwrap();
    assert_eq!(
        inspected["target"]["state"],
        json!({"availability": "disabled", "visibility": "visible"})
    );

    let located = call(
        "locate",
        json!({"fixture": fixture("settings-saves.manifold"), "text": "Save", "role": "button"}),
    )
    .unwrap();
    let candidates = located["candidates"].as_array().unwrap();
    assert_eq!(candidates[0]["id"], "z-save-signature");
    assert_eq!(
        candidates[0]["state"],
        json!({"availability": "enabled", "visibility": "visible"})
    );
    let disabled = candidates
        .iter()
        .find(|row| row["id"] == "a-save-vacation")
        .unwrap();
    assert_eq!(disabled["state"]["availability"], "disabled");
}

#[test]
fn a_repeated_identical_locate_carries_repeated_query_and_still_ranks() {
    let mut server = Server::new();
    let ask =
        json!({"fixture": fixture("settings-saves.manifold"), "text": "Save", "role": "button"});
    let first = server.call_tool("locate", &ask).unwrap();
    assert_eq!(first["signals"], json!([]));

    // Same query, different spelling of the text: still the same query.
    let again =
        json!({"fixture": fixture("settings-saves.manifold"), "text": " save! ", "role": "BUTTON"});
    let second = server.call_tool("locate", &again).unwrap();
    assert_eq!(second["candidates"], first["candidates"]);
    assert_eq!(second["target"], first["target"]);
    assert_eq!(
        second["signals"],
        json!([{
            "kind": "repeated_query",
            "count": 2,
            "top": {"id": "z-save-signature", "suggested_position": null},
            "runner_up": {"id": "a-save-vacation", "suggested_position": null},
        }])
    );
    assert!(second.get("executed").is_none(), "{second}");

    let third = server.call_tool("locate", &ask).unwrap();
    assert_eq!(third["signals"][0]["count"], 3);

    // A different query on the same page is a first call.
    let positioned = json!({"fixture": fixture("settings-saves.manifold"), "text": "Save", "role": "button", "position": "left"});
    assert_eq!(
        server.call_tool("locate", &positioned).unwrap()["signals"],
        json!([])
    );

    // The stateless entry point keeps nothing, so it never signals.
    assert_eq!(call("locate", ask.clone()).unwrap()["signals"], json!([]));
    assert_eq!(call("locate", ask).unwrap()["signals"], json!([]));
    assert_eq!(ultra_instinct_mcp::REPEAT_THRESHOLD, 2);
}
