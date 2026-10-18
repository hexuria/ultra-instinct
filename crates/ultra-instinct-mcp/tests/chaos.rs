//! Untrusted and structured JSON-RPC lines must not panic. Not a proof.

use std::path::Path;

use proptest::prelude::*;
use serde_json::{json, Value};
use ultra_instinct_mcp::{handle_line, Server, ToolError};

fn fixture(name: &str) -> String {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../fixtures")
        .join(name)
        .to_str()
        .unwrap()
        .to_owned()
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(16))]
    #[test]
    fn random_lines_do_not_panic(raw in "\\PC{0,160}") {
        let _ = handle_line(&raw);
    }
}

fn tool_body(value: &Value) -> Value {
    let text = value["result"]["content"][0]["text"].as_str().unwrap();
    serde_json::from_str(text).unwrap()
}

// Structurally valid `tools/call` envelopes with typed outcomes. 32 cases.
proptest! {
    #![proptest_config(ProptestConfig::with_cases(32))]
    #[test]
    fn structured_tool_calls_are_typed(
        tool in prop_oneof![
            Just("observe"),
            Just("locate"),
            Just("inspect"),
            Just("act"),
            Just("guard"),
            Just("diff"),
            Just("verify"),
            Just("navigate"),
            Just("nope"),
        ],
        with_goal in any::<bool>(),
        with_coords in any::<bool>(),
        confidence in prop_oneof![
            Just(None),
            Just(Some(0.4f64)),
            Just(Some(0.5496f64)),
            Just(Some(0.9f64)),
            Just(Some(1.5f64)),
            Just(Some(-0.1f64)),
        ],
    ) {
        let mut arguments = serde_json::Map::new();
        arguments.insert("fixture".into(), json!(fixture("sign-in-press.cdp.json")));
        if with_goal {
            arguments.insert("goal".into(), json!("do the thing"));
        }
        if with_coords {
            arguments.insert("x".into(), json!(10));
        }
        match tool {
            "locate" => {
                arguments.insert("text".into(), json!("Sign in"));
            }
            "inspect" => {
                arguments.insert("region".into(), json!("n100"));
            }
            "act" | "guard" => {
                arguments.insert("target".into(), json!("Sign in"));
                arguments.insert("role".into(), json!("button"));
            }
            "diff" => {
                arguments.remove("fixture");
                arguments.insert("before".into(), json!(fixture("sign-in.cdp.json")));
                arguments.insert("after".into(), json!(fixture("welcome.cdp.json")));
            }
            "verify" => {
                arguments.insert("expect_text".into(), json!("Sign in"));
            }
            _ => {}
        }
        if let Some(score) = confidence {
            arguments.insert("confidence".into(), json!(score));
        }
        let line = json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "tools/call",
            "params": {"name": tool, "arguments": Value::Object(arguments)}
        })
        .to_string();
        let response = handle_line(&line);
        prop_assert!(response.is_some(), "a request with an id must get a reply");
        let value: Value = serde_json::from_str(&response.unwrap()).unwrap();
        prop_assert_eq!(&value["jsonrpc"], "2.0");
        prop_assert_eq!(&value["id"], 1);
        let body = tool_body(&value);
        let is_error = value["result"]["isError"] == true;
        let variant = if is_error {
            body["variant"].clone()
        } else {
            Value::Null
        };
        // Argument checks run before the tool name: goal, then coordinates.
        if with_goal {
            prop_assert_eq!(variant, "GoalNotAccepted");
        } else if with_coords {
            prop_assert_eq!(variant, "CoordinatesNotAccepted");
        } else if tool == "navigate" {
            prop_assert_eq!(variant, "GoalNotAccepted");
        } else if tool == "nope" {
            prop_assert_eq!(variant, "UnknownTool");
        } else if tool == "act" || tool == "guard" {
            // Guard never clicks. Confidence args are ignored; ranking decides.
            prop_assert!(!is_error, "{tool}: {body}");
            prop_assert_eq!(&body["executed"], false);
            prop_assert!(body.get("decision").is_some(), "{body}");
        } else {
            // Every other verb on a valid fixture succeeds.
            prop_assert!(!is_error, "{tool}: {body}");
        }

        // The typed Server path with empty arguments never panics and fails
        // with a named variant.
        let mut server = Server::new();
        let result = server.call_tool(tool, &Value::Object(serde_json::Map::new()));
        match result {
            Err(ToolError::MissingFixture)
            | Err(ToolError::GoalNotAccepted)
            | Err(ToolError::UnknownTool(_))
            | Err(ToolError::MissingBefore) => {}
            other => prop_assert!(false, "unexpected {other:?}"),
        }
    }
}
