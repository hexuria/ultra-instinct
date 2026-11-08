//! `browser_state_*`, `browser_cookies`, `browser_storage` over a scripted
//! Chrome. Values stay masked unless the call reveals them.

use std::cell::RefCell;
use std::path::PathBuf;
use std::rc::Rc;

use serde_json::{json, Value};
use ultra_instinct_browser::script::ScriptBuilder;
use ultra_instinct_browser::{CdpError, CdpTransport, ReplayTransport};
use ultra_instinct_mcp::{Server, ToolError, STATE_TOOLS, TOOLS};

type Calls = Rc<RefCell<Vec<(String, Value)>>>;

struct Logged {
    inner: ReplayTransport,
    calls: Calls,
}

impl CdpTransport for Logged {
    fn call(&mut self, method: &str, params_json: &str) -> Result<String, CdpError> {
        let out = self.inner.call(method, params_json)?;
        self.calls.borrow_mut().push((
            method.to_owned(),
            serde_json::from_str(params_json).unwrap(),
        ));
        Ok(out)
    }
}

fn server(script: ScriptBuilder) -> (Server, Calls) {
    let calls: Calls = Rc::default();
    let shared = Rc::clone(&calls);
    let mut script = Some(script);
    let server = Server::with_connector(move |_url| {
        let script = script.take().ok_or_else(|| CdpError::Transport {
            message: "one connect per test".into(),
        })?;
        Ok(Box::new(Logged {
            inner: ReplayTransport::parse(&script.to_json())?,
            calls: Rc::clone(&shared),
        }) as Box<dyn CdpTransport>)
    });
    (server, calls)
}

fn methods(calls: &Calls) -> Vec<String> {
    calls.borrow().iter().map(|(m, _)| m.clone()).collect()
}

fn scratch(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "ultra-instinct-mcp-state-{}-{name}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir.join("login.json")
}

const TOKEN: &str = "s3cret-session-token";

fn cookies_result() -> Value {
    json!({"cookies": [
        {"name": "sid", "value": TOKEN, "domain": "app.example.test", "path": "/",
         "expires": 1_900_000_000.0, "httpOnly": true, "secure": true, "session": false, "sameSite": "Lax"},
        {"name": "pref", "value": "dark", "domain": ".example.test", "path": "/",
         "expires": -1, "httpOnly": false, "secure": false, "session": true}
    ]})
}

fn storage_result() -> Value {
    json!({"result": {"type": "object", "value": {
        "origin": "https://app.example.test", "host": "app.example.test",
        "local": {"auth": "bearer xyz"}, "session": {"step": "2"}
    }}})
}

#[test]
fn state_tools_are_listed() {
    for name in STATE_TOOLS {
        assert!(TOOLS.contains(&name), "{name}");
    }
}

#[test]
fn cookies_list_masks_values_by_default_and_reveals_on_request() {
    let (mut server, _) = server(
        ScriptBuilder::new()
            .call("Storage.getCookies", cookies_result())
            .call("Storage.getCookies", cookies_result()),
    );
    let listed = server
        .call_tool("browser_cookies", &json!({"action": "list"}))
        .unwrap();
    assert_eq!(listed["count"], 2);
    let sid = &listed["cookies"][0];
    assert_eq!(sid["name"], "sid");
    assert_eq!(sid["value"], "•••");
    assert_eq!(sid["value_length"], TOKEN.len());
    assert_eq!(sid["http_only"], true);
    assert!(!listed.to_string().contains(TOKEN), "{listed}");

    let revealed = server
        .call_tool(
            "browser_cookies",
            &json!({"action": "list", "name": "sid", "reveal": true}),
        )
        .unwrap();
    assert_eq!(revealed["count"], 1);
    assert_eq!(revealed["cookies"][0]["value"], TOKEN);
}

#[test]
fn cookies_set_defaults_to_the_page_host_and_clear_filters() {
    let (mut server, calls) = server(
        ScriptBuilder::new()
            .call("Runtime.evaluate", storage_result())
            .call("Network.setCookies", json!({}))
            .call("Storage.getCookies", cookies_result())
            .call("Network.deleteCookies", json!({}))
            .call("Network.clearBrowserCookies", json!({})),
    );
    let set = server
        .call_tool(
            "browser_cookies",
            &json!({"action": "set", "name": "sid", "value": TOKEN, "secure": true, "same_site": "strict"}),
        )
        .unwrap();
    assert_eq!(set["set"]["domain"], "app.example.test");
    assert_eq!(set["set"]["value"], "•••", "set echoes no value: {set}");
    let sent = calls.borrow()[1].1.clone();
    assert_eq!(
        sent,
        json!({"cookies": [{"name": "sid", "value": TOKEN, "domain": "app.example.test",
            "path": "/", "httpOnly": false, "secure": true, "sameSite": "Strict"}]})
    );

    let cleared = server
        .call_tool(
            "browser_cookies",
            &json!({"action": "clear", "name": "sid"}),
        )
        .unwrap();
    assert_eq!(cleared["cleared"], 1);
    assert_eq!(
        calls.borrow()[3].1,
        json!({"name": "sid", "domain": "app.example.test", "path": "/"})
    );

    let all = server
        .call_tool("browser_cookies", &json!({"action": "clear"}))
        .unwrap();
    assert_eq!(all["cleared"], "all");
    assert_eq!(
        methods(&calls),
        [
            "Runtime.evaluate",
            "Network.setCookies",
            "Storage.getCookies",
            "Network.deleteCookies",
            "Network.clearBrowserCookies"
        ]
    );
}

#[test]
fn cookie_arguments_are_checked() {
    let (mut server, calls) = server(ScriptBuilder::new());
    for args in [
        json!({"action": "set", "value": "v", "domain": "a.test"}),
        json!({"action": "set", "name": "n", "domain": "a.test"}),
        json!({"action": "set", "name": "n", "value": "v", "domain": "a.test", "same_site": "maybe"}),
        json!({"action": "set", "name": "n", "value": "v", "domain": "a.test", "path": "x"}),
        json!({"action": "eat"}),
        json!({"action": "list", "reveal": "yes"}),
    ] {
        let err = server.call_tool("browser_cookies", &args).unwrap_err();
        assert!(
            matches!(err, ToolError::InvalidArguments(_)),
            "{args}: {err:?}"
        );
    }
    assert!(calls.borrow().is_empty());
}

#[test]
fn storage_list_masks_get_reads_set_and_clear_write_through_arguments() {
    let global = json!({"result": {"type": "object", "objectId": "g1"}});
    let ok = json!({"result": {"type": "boolean", "value": true}});
    let (mut server, calls) = server(
        ScriptBuilder::new()
            .call("Runtime.evaluate", storage_result())
            .call("Runtime.evaluate", storage_result())
            .call("Runtime.evaluate", storage_result())
            .call("Runtime.evaluate", global.clone())
            .call("Runtime.callFunctionOn", ok.clone())
            .call("Runtime.evaluate", global.clone())
            .call("Runtime.callFunctionOn", ok.clone())
            .call("Runtime.evaluate", global)
            .call("Runtime.callFunctionOn", ok),
    );
    let listed = server
        .call_tool("browser_storage", &json!({"area": "local"}))
        .unwrap();
    assert_eq!(listed["origin"], "https://app.example.test");
    assert_eq!(
        listed["entries"]["auth"],
        json!({"value": "•••", "value_length": 10})
    );
    let revealed = server
        .call_tool(
            "browser_storage",
            &json!({"area": "session", "action": "list", "reveal": true}),
        )
        .unwrap();
    assert_eq!(revealed["entries"]["step"]["value"], "2");
    let got = server
        .call_tool(
            "browser_storage",
            &json!({"area": "local", "action": "get", "key": "auth"}),
        )
        .unwrap();
    assert_eq!(got["value"], "bearer xyz");

    server
        .call_tool(
            "browser_storage",
            &json!({"area": "local", "action": "set", "key": "k'); alert(1); ('", "value": "v"}),
        )
        .unwrap();
    server
        .call_tool(
            "browser_storage",
            &json!({"area": "session", "action": "clear", "key": "step"}),
        )
        .unwrap();
    let all = server
        .call_tool(
            "browser_storage",
            &json!({"area": "session", "action": "clear"}),
        )
        .unwrap();
    assert_eq!(all["cleared"], "all");

    let writes: Vec<Value> = calls
        .borrow()
        .iter()
        .filter(|(m, _)| m == "Runtime.callFunctionOn")
        .map(|(_, p)| p["arguments"].clone())
        .collect();
    assert_eq!(
        writes[0],
        json!([{"value": null}, {"value": "local"}, {"value": "set"}, {"value": {"k'); alert(1); ('": "v"}}])
    );
    assert_eq!(writes[1][2]["value"], "remove");
    assert_eq!(writes[1][3]["value"], json!(["step"]));
    assert_eq!(writes[2][2]["value"], "clear");
    let err = server
        .call_tool("browser_storage", &json!({"area": "cookie"}))
        .unwrap_err();
    assert!(matches!(err, ToolError::InvalidArguments(_)));
}

#[test]
fn state_save_prints_counts_only_and_load_round_trips() {
    let path = scratch("round-trip");
    let global = json!({"result": {"type": "object", "objectId": "g1"}});
    let ok = json!({"result": {"type": "boolean", "value": true}});
    let (mut server, calls) = server(
        ScriptBuilder::new()
            .call("Storage.getCookies", cookies_result())
            .call("Runtime.evaluate", storage_result())
            .call("Network.setCookies", json!({}))
            .call(
                "Runtime.evaluate",
                json!({"result": {"value": "https://app.example.test"}}),
            )
            .call("Runtime.evaluate", global.clone())
            .call("Runtime.callFunctionOn", ok.clone())
            .call("Runtime.evaluate", global)
            .call("Runtime.callFunctionOn", ok),
    );
    let path_str = path.to_str().unwrap();
    let saved = server
        .call_tool("browser_state_save", &json!({"path": path_str}))
        .unwrap();
    assert_eq!(
        saved,
        json!({"path": path_str, "cookies": 2,
            "origins": [{"origin": "https://app.example.test", "local": 1, "session": 1}]})
    );
    assert!(!saved.to_string().contains(TOKEN));
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600);
    }
    let file = std::fs::read_to_string(&path).unwrap();
    assert!(file.contains("\"version\": 1"), "{file}");

    let loaded = server
        .call_tool("browser_state_load", &json!({"path": path_str}))
        .unwrap();
    assert_eq!(loaded["cookies"], 2);
    assert_eq!(loaded["applied_origin"], "https://app.example.test");
    assert_eq!(loaded["pending_origins"], json!([]));
    assert!(!loaded.to_string().contains(TOKEN));
    let calls = calls.borrow();
    let set = &calls[2];
    assert_eq!(set.0, "Network.setCookies");
    assert_eq!(set.1["cookies"][0]["value"], TOKEN);
}

#[test]
fn state_load_reports_pending_origins() {
    let path = scratch("pending");
    let state = ultra_instinct_browser::storage_state::StorageState::parse(
        r#"{"version":1,"origins":[{"origin":"https://other.test","local":{"a":"b"}}]}"#,
    )
    .unwrap();
    ultra_instinct_browser::storage_state::write_state_file(&path, &state).unwrap();
    let (mut server, _) = server(ScriptBuilder::new().call(
        "Runtime.evaluate",
        json!({"result": {"value": "https://app.example.test"}}),
    ));
    let loaded = server
        .call_tool(
            "browser_state_load",
            &json!({"path": path.to_str().unwrap()}),
        )
        .unwrap();
    assert_eq!(loaded["applied_origin"], Value::Null);
    assert_eq!(loaded["pending_origins"], json!(["https://other.test"]));
    assert!(loaded["hint"]
        .as_str()
        .unwrap()
        .contains("browser_navigate"));
}

#[cfg(unix)]
#[test]
fn state_load_refuses_a_world_readable_file() {
    use std::os::unix::fs::PermissionsExt;
    let path = scratch("loose");
    std::fs::write(&path, r#"{"version":1}"#).unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();
    let (mut server, calls) = server(ScriptBuilder::new());
    let err = server
        .call_tool(
            "browser_state_load",
            &json!({"path": path.to_str().unwrap()}),
        )
        .unwrap_err();
    match err {
        ToolError::InvalidArguments(message) => {
            assert!(message.contains("chmod 600"), "{message}");
            assert!(message.contains("0644"), "{message}");
        }
        other => panic!("{other:?}"),
    }
    assert!(calls.borrow().is_empty(), "nothing reached the browser");
}

#[test]
fn state_load_rejects_another_version() {
    let path = scratch("v2");
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);
    use std::io::Write;
    options
        .open(&path)
        .unwrap()
        .write_all(br#"{"version":2,"cookies":[]}"#)
        .unwrap();
    let (mut server, _) = server(ScriptBuilder::new());
    let err = server
        .call_tool(
            "browser_state_load",
            &json!({"path": path.to_str().unwrap()}),
        )
        .unwrap_err();
    assert!(
        matches!(&err, ToolError::InvalidArguments(m) if m.contains("unsupported version 2")),
        "{err:?}"
    );
}

#[test]
fn tools_list_carries_state_schemas() {
    let line = r#"{"jsonrpc":"2.0","id":1,"method":"tools/list"}"#;
    let mut server = Server::new();
    let reply: Value = serde_json::from_str(&server.handle_line(line).unwrap()).unwrap();
    let tools = reply["result"]["tools"].as_array().unwrap();
    let storage = tools
        .iter()
        .find(|t| t["name"] == "browser_storage")
        .unwrap();
    assert_eq!(storage["inputSchema"]["required"], json!(["area"]));
    let cookies = tools
        .iter()
        .find(|t| t["name"] == "browser_cookies")
        .unwrap();
    assert!(cookies["inputSchema"]["properties"]["reveal"].is_object());
}
