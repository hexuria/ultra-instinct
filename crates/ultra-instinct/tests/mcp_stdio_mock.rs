//! Mock environment over the real stdio process.
//!
//! One `ultra-instinct mcp` child, driven line by line the way an MCP client would:
//! initialize, tools/list, then observe, guard, and verify, where each call
//! is built from the previous reply. The pages are CDP replay scripts
//! written by `ScriptBuilder` into a temp dir. Fixture origins open a fresh
//! replay per call; the live-session paths are covered in
//! `ultra-instinct-mcp/tests/mock_env.rs`. No Chrome, no network, no API key.

use std::io::{BufRead, BufReader, Write};
use std::path::PathBuf;
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};

use serde_json::{json, Value};
use ultra_instinct_browser::script::{Control, PageSpec, ScriptBuilder};

struct Client {
    child: Child,
    stdin: Option<ChildStdin>,
    stdout: BufReader<ChildStdout>,
    next_id: u64,
}

impl Client {
    fn spawn() -> Self {
        let mut child = Command::new(env!("CARGO_BIN_EXE_ultra-instinct"))
            .arg("mcp")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("spawn");
        let stdin = child.stdin.take();
        let stdout = BufReader::new(child.stdout.take().expect("stdout"));
        Self {
            child,
            stdin,
            stdout,
            next_id: 1,
        }
    }

    fn send(&mut self, message: &Value) {
        let stdin = self.stdin.as_mut().expect("open stdin");
        writeln!(stdin, "{message}").unwrap();
        stdin.flush().unwrap();
    }

    fn request(&mut self, method: &str, params: Value) -> Value {
        let id = self.next_id;
        self.next_id += 1;
        self.send(&json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params}));
        let mut line = String::new();
        assert!(
            self.stdout.read_line(&mut line).unwrap() > 0,
            "no reply to {method}"
        );
        let reply: Value = serde_json::from_str(&line).unwrap();
        assert_eq!(reply["id"], id, "{reply}");
        reply
    }

    /// A tools/call. Returns (is_error, parsed text body).
    fn tool(&mut self, name: &str, arguments: Value) -> (bool, Value) {
        let reply = self.request("tools/call", json!({"name": name, "arguments": arguments}));
        let result = &reply["result"];
        let text = result["content"][0]["text"].as_str().expect("text content");
        (
            result["isError"] == true,
            serde_json::from_str(text).unwrap(),
        )
    }

    fn ok(&mut self, name: &str, arguments: Value) -> Value {
        let (is_error, body) = self.tool(name, arguments.clone());
        assert!(!is_error, "{name} {arguments}: {body}");
        body
    }

    fn finish(mut self) {
        drop(self.stdin.take());
        let mut rest = String::new();
        assert_eq!(
            self.stdout.read_line(&mut rest).unwrap(),
            0,
            "extra output: {rest}"
        );
        assert!(self.child.wait().unwrap().success());
    }
}

fn rect(x: f64, y: f64) -> (f64, f64, f64, f64) {
    (x, y, 80.0, 32.0)
}

fn sign_in() -> PageSpec {
    PageSpec::of(
        &[
            Control::button(10, 100, "Sign in", rect(600.0, 340.0)),
            Control::link(20, 200, "Forgot password", rect(600.0, 400.0)),
        ],
        "https://example.test/sign-in",
        "Sign in",
    )
}

fn account() -> PageSpec {
    PageSpec::of(
        &[Control::button(30, 300, "Welcome back", rect(600.0, 340.0))],
        "https://example.test/account",
        "Account",
    )
}

fn twins() -> PageSpec {
    PageSpec::of(
        &[
            Control::button(10, 100, "Send", rect(40.0, 600.0)),
            Control::button(20, 200, "Send", rect(1160.0, 600.0)),
        ],
        "https://example.test/compose",
        "Compose",
    )
}

struct Pages {
    dir: PathBuf,
}

impl Pages {
    fn new() -> Self {
        let dir =
            std::env::temp_dir().join(format!("ultra-instinct-stdio-mock-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        Self { dir }
    }

    fn write(&self, name: &str, script: ScriptBuilder) -> String {
        let path = self.dir.join(name);
        std::fs::write(&path, script.to_json()).unwrap();
        let path = path.to_str().unwrap().to_owned();
        assert!(!path.contains('"'), "{path}");
        path
    }
}

impl Drop for Pages {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

#[test]
fn an_mcp_client_drives_observe_guard_verify_over_stdio() {
    let pages = Pages::new();
    let sign_in_page = pages.write("sign-in.cdp.json", ScriptBuilder::new().observe(&sign_in()));
    let twins_page = pages.write("twins.cdp.json", ScriptBuilder::new().observe(&twins()));
    let account_page = pages.write("account.cdp.json", ScriptBuilder::new().observe(&account()));

    let mut client = Client::spawn();
    let init = client.request(
        "initialize",
        json!({"protocolVersion": "2024-11-05", "capabilities": {}, "clientInfo": {"name": "mock", "version": "0"}}),
    );
    assert_eq!(init["result"]["serverInfo"]["name"], "ultra-instinct");
    client.send(&json!({"jsonrpc": "2.0", "method": "notifications/initialized"}));
    let tools = client.request("tools/list", json!({}));
    let names: Vec<&str> = tools["result"]["tools"]
        .as_array()
        .unwrap()
        .iter()
        .map(|tool| tool["name"].as_str().unwrap())
        .collect();
    assert_eq!(names[..3], ["observe", "guard", "verify"]);
    for removed in ["locate", "inspect", "act", "diff"] {
        assert!(!names.contains(&removed), "{removed}");
    }

    let observed = client.ok("observe", json!({"fixture": sign_in_page}));
    assert_eq!(observed["snapshot"], 1);

    // Twins: the same label twice escalates; nothing is pressed.
    let twins = client.ok(
        "guard",
        json!({"fixture": twins_page, "target": "Send", "role": "button"}),
    );
    assert_eq!(twins["decision"], "escalate", "{twins}");
    assert_eq!(twins["reason"], "ambiguous", "{twins}");
    assert_eq!(twins["executed"], false);

    // One Sign in: allowed with a ticket, still never clicked.
    let allowed = client.ok(
        "guard",
        json!({"fixture": sign_in_page, "target": "Sign in", "role": "button"}),
    );
    assert_eq!(allowed["decision"], "allow", "{allowed}");
    assert_eq!(allowed["target"]["id"], "n100");
    assert_eq!(allowed["ticket"]["target_id"], "n100");
    assert_eq!(allowed["executed"], false);

    // The host pressed it itself; verify compares the two observations.
    let after = client.ok("observe", json!({"fixture": account_page}));
    let verified = client.ok(
        "verify",
        json!({
            "before_snapshot": observed["snapshot"],
            "after_snapshot": after["snapshot"],
            "expect_url_changed": true,
        }),
    );
    assert_eq!(verified["verified"], true, "{verified}");

    let (is_error, removed) = client.tool("locate", json!({"fixture": sign_in_page}));
    assert!(is_error);
    assert_eq!(removed["variant"], "UnknownTool");

    let (is_error, navigate) = client.tool("navigate", json!({"url": "https://example.test"}));
    assert!(is_error);
    assert_eq!(navigate["variant"], "GoalNotAccepted");

    client.finish();
}
