//! Newline-delimited JSON-RPC 2.0.
//!
//! stdout carries only protocol messages. This module does not speak
//! `Content-Length` framing and does not accept JSON-RPC batches. Those are
//! documented absences, not silent successes.

use std::io::{self, BufRead, Write};

use serde_json::{json, Value};

use crate::server::Server;
use crate::ToolError;
use crate::TOOLS;

pub const PROTOCOL_VERSION: &str = "2024-11-05";

/// Read stdin until EOF. Write one JSON line per response. Notifications and
/// blank lines produce nothing.
pub fn serve_stdio() -> io::Result<()> {
    let stdin = io::stdin();
    let mut stdout = io::stdout();
    let mut server = Server::new();
    for line in stdin.lock().lines() {
        let line = line?;
        if let Some(response) = server.handle_line(&line) {
            stdout.write_all(response.as_bytes())?;
            stdout.write_all(b"\n")?;
            stdout.flush()?;
        }
    }
    Ok(())
}

/// One inbound line against a fresh [`Server`]. `None` means the client must
/// not be answered. Use [`Server::handle_line`] to keep state between lines.
pub fn handle_line(line: &str) -> Option<String> {
    Server::new().handle_line(line)
}

pub(crate) fn handle_line_with(server: &mut Server, line: &str) -> Option<String> {
    let line = line.trim();
    if line.is_empty() {
        return None;
    }
    let value: Value = match serde_json::from_str(line) {
        Ok(value) => value,
        Err(_) => {
            return Some(rpc_error(
                Value::Null,
                -32700,
                "parse error",
                json!({"variant": "ParseError"}),
            ))
        }
    };
    if !value.is_object() {
        return Some(rpc_error(
            Value::Null,
            -32600,
            "invalid request",
            json!({"variant": "InvalidRequest"}),
        ));
    }
    dispatch_object(server, &value)
}

fn dispatch_object(server: &mut Server, value: &Value) -> Option<String> {
    let id = value.get("id").cloned();
    if value.get("jsonrpc").and_then(Value::as_str) != Some("2.0") {
        return Some(rpc_error(
            id.unwrap_or(Value::Null),
            -32600,
            "invalid request",
            json!({"variant": "InvalidRequest"}),
        ));
    }
    let Some(method) = value.get("method").and_then(Value::as_str) else {
        return Some(rpc_error(
            id.unwrap_or(Value::Null),
            -32600,
            "invalid request",
            json!({"variant": "InvalidRequest"}),
        ));
    };
    let id = id?;
    let params = value.get("params").cloned().unwrap_or(Value::Null);
    Some(match method {
        "initialize" => rpc_ok(id, initialize_result()),
        "tools/list" => rpc_ok(id, tools_list(server)),
        "tools/call" => match tools_call(server, &params) {
            Ok(result) => rpc_ok(id, result),
            Err(err) => rpc_error(id, -32602, &err.to_string(), err.to_value()),
        },
        "ping" => rpc_ok(id, json!({})),
        other => rpc_error(
            id,
            -32601,
            "method not found",
            json!({"variant": "MethodNotFound", "method": other}),
        ),
    })
}

fn tools_call(server: &mut Server, params: &Value) -> Result<Value, ToolError> {
    if !params.is_object() && !params.is_null() {
        return Err(ToolError::InvalidArguments(
            "tools/call params must be an object".into(),
        ));
    }
    let name = params
        .get("name")
        .and_then(Value::as_str)
        .ok_or(ToolError::MissingToolName)?;
    let arguments = params.get("arguments").cloned().unwrap_or(Value::Null);
    match server.call_tool(name, &arguments) {
        Ok(body) if name == "browser_screenshot" => Ok(image_result(body)),
        // The text state goes back as plain text, not a JSON-escaped string.
        Ok(body)
            if name == "browser_get_state"
                && body.get("format").and_then(Value::as_str) == Some("text") =>
        {
            match body.get("text").and_then(Value::as_str) {
                Some(text) => Ok(tool_result(text.to_owned(), false)),
                None => Ok(tool_result(body.to_string(), false)),
            }
        }
        Ok(body) => Ok(tool_result(body.to_string(), false)),
        Err(err) => Ok(tool_result(err.to_json_string(), true)),
    }
}

/// A screenshot goes back as an MCP `image` item the client can show
/// (browser-use parity), then the rest of the body as text, without the
/// base64 it would otherwise repeat.
fn image_result(mut body: Value) -> Value {
    let mime = match body.get("format").and_then(Value::as_str) {
        Some("jpeg") => "image/jpeg",
        _ => "image/png",
    };
    match body.as_object_mut().and_then(|map| map.remove("base64")) {
        Some(Value::String(data)) => json!({
            "content": [
                {"type": "image", "data": data, "mimeType": mime},
                {"type": "text", "text": body.to_string()},
            ],
            "isError": false,
        }),
        _ => tool_result(body.to_string(), false),
    }
}

fn tool_result(text: String, is_error: bool) -> Value {
    json!({
        "content": [{"type": "text", "text": text}],
        "isError": is_error,
    })
}

fn initialize_result() -> Value {
    json!({
        "protocolVersion": PROTOCOL_VERSION,
        "capabilities": {"tools": {"listChanged": false}},
        "serverInfo": {"name": "ultra-instinct", "version": env!("CARGO_PKG_VERSION")},
        "instructions": "ultra-instinct is an action firewall plus a browser-use-compatible tool surface. Firewall tools: observe, guard (Allow returns an ActionTicket the host must revalidate before clicking the exact target), verify — these never click. browser_* tools drive a live CDP endpoint like browser-use's MCP server (browser_navigate, browser_get_state, browser_click, browser_type, browser_scroll, browser_screenshot, tabs): element indexes come from browser_get_state in reading order, and click/type go through the ticketed executor — they re-observe and refuse on drift instead of clicking the wrong element. Never guess coordinates."
    })
}

fn tools_list(server: &Server) -> Value {
    json!({
        "tools": TOOLS
            .iter()
            .copied()
            .filter(|name| server.allows(name))
            .map(tool_spec)
            .collect::<Vec<_>>(),
    })
}

fn tool_spec(name: &str) -> Value {
    if name.starts_with("browser_") {
        let (description, properties, required) = ultra_instinct_browser_tools::spec(name);
        return json!({
            "name": name,
            "description": description,
            "inputSchema": {"type": "object", "properties": properties, "required": required},
        });
    }
    let (description, properties, required) = match name {
        "observe" => (
            "Read a CDP fixture or an optional live CDP endpoint into regions. Returns id, role, label, parent, state (availability enabled or disabled; visibility visible, occluded, offscreen, or hidden; a region behind an open dialog reads occluded), focused (region id or null), front_layer (open dialogs), and a snapshot id for guard seen_snapshot and verify before/after. Does not click and does not choose the next capability.",
            source_props(),
            Vec::<&str>::new(),
        ),
        "guard" => (
            "Decide allow / refuse / escalate for a proposed click against the page as observed now. Allow includes an ActionTicket the host must revalidate before clicking. Never clicks. Pass target (or text): the region whose label equals it (trimmed, whitespace collapsed, case ignored) is the target, unique or refuse. Optional role, optional within (container id), optional proposed region id (picks one of several matches), optional seen_snapshot (the observation the host decided on). No match refuses missing-target; several visible matches escalate ambiguous. Refuses front-layer when the target is behind an open dialog; escalates world-changed when focus, open dialogs, clickable ids, or occluded regions differ from seen_snapshot. Returns decision, evidence, candidates, focused, front_layer.",
            {
                let mut props = guard_props();
                props.insert(
                    "target".into(),
                    json!({
                        "type": "string",
                        "description": "Visible label to resolve (alias of text)."
                    }),
                );
                props.insert(
                    "proposed".into(),
                    json!({
                        "type": "string",
                        "description": "Optional region id that must be one of the label matches."
                    }),
                );
                props.insert(
                    "region".into(),
                    json!({
                        "type": "string",
                        "description": "Deprecated alias of proposed."
                    }),
                );
                props.insert(
                    "seen_snapshot".into(),
                    json!({
                        "type": "integer",
                        "description": "Snapshot id the host decided on. A different set of open dialogs now escalates world-changed."
                    }),
                );
                props
            },
            Vec::new(),
        ),
        "verify" => (
            "Check a postcondition. Snapshot mode: expect_text / expect_absent on a fresh observe. Ticketed delta mode: before_snapshot + after_snapshot (+ optional ticket from Allow) with expect_appeared / expect_disappeared / expect_url_changed / expect_text / expect_absent. Does not click.",
            {
                let mut props = source_props();
                props.insert("expect_text".into(), json!({"type": "string"}));
                props.insert("expect_absent".into(), json!({"type": "string"}));
                props.insert("expect_appeared".into(), json!({"type": "string"}));
                props.insert("expect_disappeared".into(), json!({"type": "string"}));
                props.insert("expect_url_changed".into(), json!({"type": "boolean"}));
                props.insert("before_snapshot".into(), json!({"type": "integer"}));
                props.insert("after_snapshot".into(), json!({"type": "integer"}));
                props.insert(
                    "ticket".into(),
                    json!({
                        "type": "object",
                        "description": "ActionTicket from GuardDecision::Allow; revalidated against before_snapshot"
                    }),
                );
                props
            },
            Vec::new(),
        ),
        _ => ("unsupported", serde_json::Map::new(), Vec::new()),
    };
    json!({
        "name": name,
        "description": description,
        "inputSchema": {
            "type": "object",
            "properties": properties,
            "required": required,
            "additionalProperties": false,
        },
    })
}

fn source_props() -> serde_json::Map<String, Value> {
    let mut props = serde_json::Map::new();
    props.insert(
        "fixture".into(),
        json!({"type": "string", "description": "Path to a CDP replay script or a manifold fixture"}),
    );
    props.insert(
        "cdp".into(),
        json!({"type": "string", "description": "Optional live CDP HTTP endpoint. Not required for tests."}),
    );
    props
}

fn guard_props() -> serde_json::Map<String, Value> {
    let mut props = source_props();
    props.insert("text".into(), json!({"type": "string"}));
    props.insert("role".into(), json!({"type": "string"}));
    props.insert("action".into(), json!({"type": "string"}));
    props.insert(
        "within".into(),
        json!({
            "type": "string",
            "description": "Region id of a container. The target must be under it in the parent chain (observe returns each region's parent)."
        }),
    );
    props
}

fn rpc_ok(id: Value, result: Value) -> String {
    json!({"jsonrpc": "2.0", "id": id, "result": result}).to_string()
}

fn rpc_error(id: Value, code: i64, message: &str, data: Value) -> String {
    json!({
        "jsonrpc": "2.0",
        "id": id,
        "error": {"code": code, "message": message, "data": data}
    })
    .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use ultra_instinct_browser::script::ScriptBuilder;
    use ultra_instinct_browser::{CdpTransport, ReplayTransport};

    fn server_replaying(script: ScriptBuilder) -> Server {
        let mut script = Some(script);
        Server::with_connector(move |_url| {
            let script = script.take().expect("one connect per test");
            Ok(Box::new(ReplayTransport::parse(&script.to_json())?) as Box<dyn CdpTransport>)
        })
    }

    fn tools_call_line(tool: &str) -> String {
        json!({"jsonrpc": "2.0", "id": 1, "method": "tools/call",
               "params": {"name": tool, "arguments": {"cdp": "http://127.0.0.1:9222"}}})
        .to_string()
    }

    /// The screenshot reaches an MCP client as an image it can show; the text
    /// item keeps the metadata but not the base64. Other tools stay text-only.
    #[test]
    fn screenshot_is_an_mcp_image_item() {
        let mut server = server_replaying(
            ScriptBuilder::new().call("Page.captureScreenshot", json!({"data": "iVBORw0KGgo="})),
        );
        let reply: Value = serde_json::from_str(
            &handle_line_with(&mut server, &tools_call_line("browser_screenshot")).unwrap(),
        )
        .unwrap();
        let content = &reply["result"]["content"];
        assert_eq!(
            content[0],
            json!({"type": "image", "data": "iVBORw0KGgo=", "mimeType": "image/png"}),
            "{reply}"
        );
        let meta: Value = serde_json::from_str(content[1]["text"].as_str().unwrap()).unwrap();
        assert_eq!(meta, json!({"format": "png"}), "{reply}");
        assert_eq!(reply["result"]["isError"], false);

        let mut server = server_replaying(
            ScriptBuilder::new().call("Page.printToPDF", json!({"data": "JVBERi0="})),
        );
        let reply: Value = serde_json::from_str(
            &handle_line_with(&mut server, &tools_call_line("browser_save_as_pdf")).unwrap(),
        )
        .unwrap();
        assert_eq!(reply["result"]["content"][0]["type"], "text", "{reply}");
    }

    /// The default text state reaches an MCP client as plain text (not a
    /// JSON-escaped string); `format: "json"` keeps the JSON body.
    #[test]
    fn text_state_is_a_plain_text_item() {
        use ultra_instinct_browser::script::{Control, PageSpec};
        let page = PageSpec::of(
            &[Control::button(11, 101, "Go", (20.0, 20.0, 80.0, 28.0))],
            "http://127.0.0.1/a",
            "A",
        );
        let mut server = server_replaying(ScriptBuilder::new().observe_compact(&page));
        let reply: Value = serde_json::from_str(
            &handle_line_with(&mut server, &tools_call_line("browser_get_state")).unwrap(),
        )
        .unwrap();
        let text = reply["result"]["content"][0]["text"].as_str().unwrap();
        assert!(
            text.starts_with("A  http://127.0.0.1/a\n@e1 button \"Go\"\n"),
            "{reply}"
        );
        assert!(
            serde_json::from_str::<Value>(text).is_err(),
            "plain text: {text}"
        );

        let mut server = server_replaying(ScriptBuilder::new().observe_compact(&page));
        let line = json!({"jsonrpc": "2.0", "id": 1, "method": "tools/call",
            "params": {"name": "browser_get_state",
                       "arguments": {"cdp": "http://127.0.0.1:9222", "format": "json"}}})
        .to_string();
        let reply: Value =
            serde_json::from_str(&handle_line_with(&mut server, &line).unwrap()).unwrap();
        let body: Value =
            serde_json::from_str(reply["result"]["content"][0]["text"].as_str().unwrap()).unwrap();
        assert_eq!(body["elements"][0]["label"], "Go", "{reply}");
    }
}
