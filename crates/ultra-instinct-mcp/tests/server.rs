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
fn tools_list_has_the_product_tools_and_no_removed_helpers() {
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
    for removed in ["locate", "inspect", "act", "diff"] {
        assert!(!names.contains(&removed), "{removed}");
    }
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

#[test]
fn observe_and_verify_round_trip() {
    let observed = call("observe", json!({"fixture": fixture("sign-in.cdp.json")})).unwrap();
    assert_eq!(observed["tool"], "observe");
    assert_eq!(observed["executed"], false);
    assert_eq!(observed["verified"], false);
    let n100 = observed["regions"]
        .as_array()
        .unwrap()
        .iter()
        .find(|region| region["id"] == "n100")
        .expect("n100 observed");
    assert_eq!(n100["label"], "Sign in");

    let verified = call(
        "verify",
        json!({"fixture": fixture("welcome.cdp.json"), "expect_text": "Welcome"}),
    )
    .unwrap();
    assert_eq!(verified["verified"], true);
    assert_eq!(verified["executed"], false);
}

#[test]
fn removed_helpers_are_unknown_tools() {
    for name in ["locate", "inspect", "act", "diff"] {
        let err = call(name, json!({"fixture": fixture("sign-in.cdp.json")})).unwrap_err();
        assert_eq!(err, ToolError::UnknownTool(name.into()), "{name}");
    }
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

#[test]
fn verify_by_snapshot_ids_sees_the_page_change() {
    let mut server = Server::new();
    let before = server
        .call_tool("observe", &json!({"fixture": fixture("sign-in.cdp.json")}))
        .unwrap();
    let after = server
        .call_tool("observe", &json!({"fixture": fixture("welcome.cdp.json")}))
        .unwrap();
    assert_eq!(before["snapshot"], 1);
    assert_eq!(after["snapshot"], 2);
    let appeared = server
        .call_tool(
            "verify",
            &json!({"before_snapshot": 1, "after_snapshot": 2, "expect_appeared": "n300"}),
        )
        .unwrap();
    assert_eq!(appeared["verified"], true);
    let gone = server
        .call_tool(
            "verify",
            &json!({"before_snapshot": 1, "after_snapshot": 2, "expect_disappeared": "n100"}),
        )
        .unwrap();
    assert_eq!(gone["verified"], true);

    let err = server
        .call_tool(
            "verify",
            &json!({"before_snapshot": 1, "expect_appeared": "n300"}),
        )
        .unwrap_err();
    assert_eq!(err, ToolError::MissingAfter);
    let err = server
        .call_tool(
            "verify",
            &json!({"before_snapshot": 1, "after_snapshot": 9, "expect_appeared": "n300"}),
        )
        .unwrap_err();
    assert_eq!(err, ToolError::UnknownSnapshot(9));
    assert_eq!(
        err.to_value(),
        json!({"variant": "UnknownSnapshot", "id": 9})
    );
    let err = server
        .call_tool(
            "verify",
            &json!({"before_snapshot": -1, "after_snapshot": 1, "expect_appeared": "n300"}),
        )
        .unwrap_err();
    assert_eq!(err, ToolError::BadSnapshot("-1".into()));
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
        .call_tool(
            "verify",
            &json!({"before_snapshot": 1, "after_snapshot": 17, "expect_appeared": "n300"}),
        )
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
    let err = call(
        "verify",
        json!({"before_snapshot": 1, "after_snapshot": 1, "expect_appeared": "n300"}),
    )
    .unwrap_err();
    assert_eq!(err, ToolError::UnknownSnapshot(1));
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
}

#[test]
fn guard_refuses_the_matcher_arguments_by_name() {
    for key in ["near", "position", "matcher", "dims"] {
        let mut args = json!({"fixture": fixture("settings-saves.manifold"), "target": "Save"});
        args[key] = json!("left");
        match call("guard", args).unwrap_err() {
            ToolError::InvalidArguments(message) => {
                assert!(message.contains(key), "{message}");
                assert!(
                    message.contains("proposed") && message.contains("within"),
                    "{message}"
                );
            }
            other => panic!("{key}: expected InvalidArguments, got {other:?}"),
        }
    }
}
