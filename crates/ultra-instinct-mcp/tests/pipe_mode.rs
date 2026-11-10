//! `Server::with_pipe` against a fake Chrome on an in-process pipe pair.
//!
//! The fake answers `Target.*` itself (targets, create, attach, activate,
//! close with `Target.targetDestroyed`) and each page session from a replay
//! script, so the same `browser_*` paths run with no HTTP endpoint. Each test
//! pins the CDP calls the server sends (method, and the session it rode).

#![cfg(unix)]

use std::collections::{BTreeMap, VecDeque};
use std::io::{pipe, BufRead, BufReader, PipeReader, PipeWriter, Write};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};

use serde_json::{json, Value};
use ultra_instinct_browser::script::{Control, PageSpec, ScriptBuilder};
use ultra_instinct_browser::{CdpError, CdpTransport, ReplayTransport};
use ultra_instinct_cdp::PipeBrowser;
use ultra_instinct_mcp::{Server, ToolError};

/// `(target, method)` for page calls, `("", method)` for browser calls.
type Log = Arc<Mutex<Vec<(String, String)>>>;

struct Target {
    id: String,
    url: String,
    replay: ReplayTransport,
}

/// Chrome's side: one thread, scripted pages, built-in `Target.*`.
struct Fake {
    requests: BufReader<PipeReader>,
    out: PipeWriter,
    targets: Vec<Target>,
    /// Scripts for pages `Target.createTarget` opens, in order.
    next_pages: VecDeque<ScriptBuilder>,
    sessions: BTreeMap<String, String>,
    log: Log,
    /// A page method that destroys its target instead of answering.
    destroy_on: Option<&'static str>,
    /// A page method that makes Chrome exit (EOF on the pipe).
    exit_on: Option<&'static str>,
    /// A page method that opens an `alert("Saved")` instead of answering;
    /// `method:type` matches only a call whose `type` param is `type`.
    alert_on: Option<&'static str>,
}

fn hits(trigger: &str, method: &str, params: &Value) -> bool {
    match trigger.split_once(':') {
        Some((m, kind)) => m == method && params["type"] == kind,
        None => trigger == method,
    }
}

impl Fake {
    fn send(&mut self, value: Value) {
        self.out
            .write_all(format!("{value}\0").as_bytes())
            .expect("write frame");
        self.out.flush().expect("flush");
    }

    fn run(mut self) {
        loop {
            let mut frame = Vec::new();
            if self.requests.read_until(0, &mut frame).expect("read") == 0 {
                return;
            }
            frame.pop();
            let request: Value = serde_json::from_slice(&frame).expect("json");
            let id = request["id"].clone();
            let method = request["method"].as_str().unwrap_or_default().to_owned();
            let params = request["params"].clone();
            match request["sessionId"].as_str() {
                None => {
                    self.log
                        .lock()
                        .unwrap()
                        .push((String::new(), method.clone()));
                    let result = self.browser_call(&method, &params);
                    self.send(json!({"id": id, "result": result}));
                    if method == "Target.closeTarget" {
                        let target = params["targetId"].clone();
                        self.send(json!({"method": "Target.targetDestroyed",
                                         "params": {"targetId": target}}));
                    }
                }
                Some(session) => {
                    let session = session.to_owned();
                    let target = self.sessions[&session].clone();
                    self.log
                        .lock()
                        .unwrap()
                        .push((target.clone(), method.clone()));
                    if self.exit_on == Some(method.as_str()) {
                        return;
                    }
                    if self.alert_on.is_some_and(|m| hits(m, &method, &params)) {
                        self.alert_on = None;
                        self.send(json!({"sessionId": session,
                                         "method": "Page.javascriptDialogOpening",
                                         "params": {"type": "alert", "message": "Saved",
                                                    "url": "https://shop.example/login",
                                                    "hasBrowserHandler": false,
                                                    "defaultPrompt": ""}}));
                        continue;
                    }
                    if self.destroy_on == Some(method.as_str()) {
                        self.send(json!({"method": "Target.targetDestroyed",
                                         "params": {"targetId": target}}));
                        continue;
                    }
                    let reply = self.page_call(&target, &method, &params);
                    let mut envelope = match reply {
                        Ok(result) => json!({"id": id, "result": result}),
                        Err(message) => {
                            json!({"id": id, "error": {"code": -32000, "message": message}})
                        }
                    };
                    envelope["sessionId"] = json!(session);
                    self.send(envelope);
                }
            }
        }
    }

    fn browser_call(&mut self, method: &str, params: &Value) -> Value {
        match method {
            "Target.getTargets" => json!({"targetInfos": self.targets.iter().map(|t| json!({
                "targetId": t.id, "type": "page", "url": t.url, "title": ""
            })).chain([json!({"targetId": "W1", "type": "service_worker", "url": "", "title": ""})])
              .collect::<Vec<_>>()}),
            "Target.createTarget" => {
                let id = format!("T{}", self.targets.len() + 1);
                let script = self.next_pages.pop_front().unwrap_or_default();
                self.targets.push(Target {
                    id: id.clone(),
                    url: params["url"].as_str().unwrap_or_default().to_owned(),
                    replay: ReplayTransport::parse(&script.to_json()).expect("script"),
                });
                json!({"targetId": id})
            }
            "Target.attachToTarget" => {
                let target = params["targetId"].as_str().unwrap_or_default().to_owned();
                let session = format!("S-{target}");
                self.sessions.insert(session.clone(), target);
                json!({"sessionId": session})
            }
            "Target.closeTarget" => {
                let target = params["targetId"].as_str().unwrap_or_default();
                self.targets.retain(|t| t.id != target);
                json!({"success": true})
            }
            _ => json!({}),
        }
    }

    fn page_call(&mut self, target: &str, method: &str, params: &Value) -> Result<Value, String> {
        if matches!(method, "Emulation.setFocusEmulationEnabled" | "Page.enable") {
            return Ok(json!({}));
        }
        let page = self
            .targets
            .iter_mut()
            .find(|t| t.id == target)
            .ok_or("no target")?;
        match page.replay.call(method, &params.to_string()) {
            Ok(raw) => Ok(serde_json::from_str(&raw).expect("result json")),
            Err(err) => Err(err.to_string()),
        }
    }
}

/// The fake's log. Its thread ends at EOF, once the server (every pipe
/// handle) is dropped; nextest runs each test in its own process.
struct Chrome {
    log: Log,
    _thread: JoinHandle<()>,
}

impl Chrome {
    fn methods(&self) -> Vec<(String, String)> {
        self.log.lock().unwrap().clone()
    }

    fn page_methods(&self, target: &str) -> Vec<String> {
        self.methods()
            .into_iter()
            .filter(|(t, _)| t == target)
            .map(|(_, m)| m)
            .collect()
    }

    fn browser_methods(&self) -> Vec<String> {
        self.page_methods("")
    }
}

struct Setup {
    first: ScriptBuilder,
    next_pages: Vec<ScriptBuilder>,
    destroy_on: Option<&'static str>,
    exit_on: Option<&'static str>,
    alert_on: Option<&'static str>,
    /// `Server::with_attach` instead of `with_pipe`: T1 is the user's tab.
    attach: bool,
}

impl Setup {
    fn page(first: ScriptBuilder) -> Self {
        Self {
            first,
            next_pages: Vec::new(),
            destroy_on: None,
            exit_on: None,
            alert_on: None,
            attach: false,
        }
    }

    fn start(self) -> (Server, Chrome) {
        let (requests, browser_out) = pipe().expect("pipe");
        let (browser_in, out) = pipe().expect("pipe");
        let browser = PipeBrowser::new(browser_in, browser_out);
        let log: Log = Arc::default();
        let fake = Fake {
            requests: BufReader::new(requests),
            out,
            targets: vec![Target {
                id: "T1".into(),
                url: "about:blank".into(),
                replay: ReplayTransport::parse(&self.first.to_json()).expect("script"),
            }],
            next_pages: self.next_pages.into(),
            sessions: BTreeMap::new(),
            log: Arc::clone(&log),
            destroy_on: self.destroy_on,
            exit_on: self.exit_on,
            alert_on: self.alert_on,
        };
        let thread = thread::spawn(move || fake.run());
        (
            match self.attach {
                true => Server::with_attach(browser),
                false => Server::with_pipe(browser),
            },
            Chrome {
                log,
                _thread: thread,
            },
        )
    }
}

fn call(server: &mut Server, tool: &str, arguments: Value) -> Value {
    server
        .call_tool(tool, &arguments)
        .unwrap_or_else(|err| panic!("{tool} {arguments}: {err:?}"))
}

/// Email @e1, Sign in @e2.
fn page() -> PageSpec {
    PageSpec::of(
        &[
            Control::text_field(10, 100, "Email", (20.0, 20.0, 300.0, 28.0)),
            Control::button(11, 101, "Sign in", (20.0, 80.0, 120.0, 28.0)),
        ],
        "https://shop.example/login",
        "Log in",
    )
}

#[test]
fn navigate_state_and_a_snapshot_click_ride_the_bound_tab_with_no_http() {
    let (mut server, chrome) = Setup::page(
        ScriptBuilder::new()
            .call("Page.navigate", json!({"frameId": "F1"}))
            .ready_state("complete")
            .observe_compact(&page())
            .observe_compact(&page())
            .observe_compact(&page())
            .pointer_click(),
    )
    .start();
    // `cdp` is ignored in pipe mode: no HTTP endpoint is ever read.
    let out = call(
        &mut server,
        "browser_navigate",
        json!({"cdp": "http://127.0.0.1:1", "url": "https://shop.example/login"}),
    );
    assert_eq!(out["navigated"], "https://shop.example/login");
    let state = call(&mut server, "browser_get_state", json!({}));
    let snapshot = state["snapshot"].as_u64().unwrap();
    let out = call(
        &mut server,
        "browser_click",
        json!({"index": 2, "snapshot": snapshot}),
    );
    assert_eq!(out["region"], "n101", "{out}");
    assert_eq!(server.live_sessions(), ["pipe:T1"]);
    assert_eq!(
        chrome.browser_methods()[..3],
        [
            "Target.setDiscoverTargets",
            "Target.getTargets",
            "Target.attachToTarget"
        ],
        "{:?}",
        chrome.methods()
    );
    let page = chrome.page_methods("T1");
    assert_eq!(page[0], "Emulation.setFocusEmulationEnabled");
    assert!(page.contains(&"Page.navigate".to_owned()), "{page:?}");
    assert_eq!(
        page.iter()
            .filter(|m| *m == "Input.dispatchMouseEvent")
            .count(),
        3,
        "one pointer press: {page:?}"
    );
    assert!(
        !page.iter().any(|m| m == "Runtime.callFunctionOn"),
        "no script click: {page:?}"
    );
    drop(server);
}

#[test]
fn tabs_are_target_calls_and_a_closed_tab_falls_back() {
    let mut setup = Setup::page(ScriptBuilder::new());
    setup
        .next_pages
        .push(ScriptBuilder::new().ready_state("complete"));
    let (mut server, chrome) = setup.start();
    let opened = call(
        &mut server,
        "browser_new_tab",
        json!({"url": "https://shop.example/cart"}),
    );
    assert_eq!(opened["tab_id"], "T2", "{opened}");
    let tabs = call(&mut server, "browser_list_tabs", json!({}));
    let ids: Vec<&str> = tabs["tabs"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["tab_id"].as_str().unwrap())
        .collect();
    assert_eq!(ids, ["T1", "T2"], "pages only: {tabs}");
    let closed = call(&mut server, "browser_close_tab", json!({"tab_id": "T2"}));
    assert_eq!(closed["closed"], true);
    assert!(server.live_sessions().iter().all(|k| k != "pipe:T2"));
    let switched = call(&mut server, "browser_switch_tab", json!({"tab_id": "T1"}));
    assert_eq!(switched["active_tab"], "T1");
    let browser = chrome.browser_methods();
    for method in [
        "Target.createTarget",
        "Target.activateTarget",
        "Target.closeTarget",
    ] {
        assert!(browser.contains(&method.to_owned()), "{browser:?}");
    }
    drop(server);
}

/// Attached, the user's tab (T1) is out of reach: not listed, not switched
/// to or closed, never bound; only the tab the server opened, in a window
/// of its own. Once that closes, a page call refuses: no tab is bound.
#[test]
fn an_attached_server_sees_only_the_tabs_it_opened() {
    let mut setup = Setup::page(ScriptBuilder::new());
    setup.attach = true;
    setup
        .next_pages
        .push(ScriptBuilder::new().ready_state("complete"));
    let (mut server, chrome) = setup.start();
    let ids = |server: &mut Server| -> Vec<String> {
        call(server, "browser_list_tabs", json!({}))["tabs"]
            .as_array()
            .unwrap()
            .iter()
            .map(|t| t["tab_id"].as_str().unwrap().to_owned())
            .collect()
    };
    assert!(ids(&mut server).is_empty());
    let refused = |server: &mut Server, tool: &str, arguments: Value| {
        let err = server.call_tool(tool, &arguments).expect_err(tool);
        err.to_string()
    };
    let unbound = refused(&mut server, "browser_get_state", json!({}));
    assert!(unbound.contains("browser_new_tab"), "{unbound}");
    for tool in ["browser_switch_tab", "browser_close_tab"] {
        let err = refused(&mut server, tool, json!({"tab_id": "T1"}));
        assert!(err.contains("no tab `T1`"), "{tool}: {err}");
    }
    let opened = call(&mut server, "browser_new_tab", json!({}));
    assert_eq!(opened["tab_id"], "T2", "{opened}");
    assert_eq!(ids(&mut server), ["T2"]);
    let switched = call(&mut server, "browser_switch_tab", json!({"tab_id": "T2"}));
    assert_eq!(switched["active_tab"], "T2");
    let closed = call(&mut server, "browser_close_tab", json!({"tab_id": "T2"}));
    assert_eq!(closed["closed"], true);
    let unbound = refused(&mut server, "browser_get_state", json!({}));
    assert!(unbound.contains("browser_new_tab"), "{unbound}");
    drop(server);
    assert!(
        chrome.page_methods("T1").is_empty(),
        "{:?}",
        chrome.methods()
    );
    let browser = chrome.browser_methods();
    let count = |m: &str| browser.iter().filter(|b| *b == m).count();
    assert_eq!(count("Target.closeTarget"), 1, "{browser:?}");
    assert_eq!(
        count("Target.activateTarget"),
        1,
        "the switch only: {browser:?}"
    );
}

/// `Target.targetDestroyed` reaches the page (discovery is on): its next
/// call ends `target_gone` instead of waiting for the deadline.
#[test]
fn a_destroyed_tab_answers_target_gone() {
    let mut setup = Setup::page(ScriptBuilder::new());
    setup.destroy_on = Some("Page.reload");
    let (mut server, _chrome) = setup.start();
    let err = server
        .call_tool("browser_reload", &json!({}))
        .expect_err("destroyed");
    assert_eq!(
        err,
        ToolError::TargetGone {
            target_id: "T1".into()
        }
    );
    assert_eq!(err.to_value()["code"], "target_gone");
    assert_eq!(err.to_value()["variant"], "Browser");
}

/// Chrome exiting closes the pipe: this call and every later one answer
/// `pipe_closed`.
#[test]
fn chrome_exiting_answers_pipe_closed() {
    let mut setup = Setup::page(ScriptBuilder::new());
    setup.exit_on = Some("Runtime.evaluate");
    let (mut server, _chrome) = setup.start();
    for _ in 0..2 {
        let err = server
            .call_tool("browser_exec", &json!({"script": "1"}))
            .expect_err("closed");
        assert!(matches!(err, ToolError::PipeClosed { .. }), "{err:?}");
        assert_eq!(err.code(), "pipe_closed");
    }
}

/// observe / guard read the bound tab too; a fixture still reads the file.
#[test]
fn observe_reads_the_bound_tab() {
    let (mut server, chrome) = Setup::page(ScriptBuilder::new().observe_compact(&page())).start();
    let out = call(&mut server, "observe", json!({}));
    assert_eq!(out["regions"].as_array().unwrap().len(), 2, "{out}");
    assert!(chrome
        .page_methods("T1")
        .contains(&"Page.getLayoutMetrics".to_owned()));
    drop(server);
}

/// A transport error type check: `CdpError` stays the transport's.
#[test]
fn replay_errors_are_not_typed_failures() {
    assert!(ToolError::from_cdp(&CdpError::NoScriptedResponse { method: "X".into() }).is_none());
}

/// A click whose handler opens an `alert()` over the pipe: the press ends
/// `dialog_open` carrying the dialog's type and message, not empty fields.
#[test]
fn a_click_that_opens_an_alert_carries_its_type_and_message() {
    let mut setup = Setup::page(
        ScriptBuilder::new()
            .observe_compact(&page())
            .observe_compact(&page())
            .pointer_click(),
    );
    setup.alert_on = Some("Input.dispatchMouseEvent:mouseReleased");
    let (mut server, _chrome) = setup.start();
    let err = server
        .call_tool("browser_click", &json!({"index": 2}))
        .expect_err("the alert interrupts the press");
    let ToolError::DialogOpen { kind, message, .. } = &err else {
        panic!("{err:?}");
    };
    assert_eq!(
        (kind.as_str(), message.as_str()),
        ("alert", "Saved"),
        "{err:?}"
    );
    assert_eq!(err.to_value()["type"], "alert");
    assert_eq!(err.to_value()["message"], "Saved");
    drop(server);
}

/// `browser_download` presses through the `browser_tools` path directly:
/// an `alert()` its press opens still answers `dialog_open` with the
/// dialog's type and message, not empty fields.
#[test]
fn a_download_press_that_opens_an_alert_carries_its_type_and_message() {
    let mut setup = Setup::page(
        ScriptBuilder::new()
            .call("Browser.setDownloadBehavior", json!({}))
            .observe_compact(&page())
            .observe_compact(&page())
            .pointer_click()
            .call("Browser.setDownloadBehavior", json!({})),
    );
    setup.alert_on = Some("Input.dispatchMouseEvent:mouseReleased");
    let (mut server, _chrome) = setup.start();
    let path = std::env::temp_dir().join(format!("aui-pipe-alert-{}.bin", std::process::id()));
    let err = server
        .call_tool(
            "browser_download",
            &json!({"index": 2, "path": path.display().to_string(), "timeout_ms": 50}),
        )
        .expect_err("the alert interrupts the press");
    let ToolError::DialogOpen { kind, message, .. } = &err else {
        panic!("{err:?}");
    };
    assert_eq!(
        (kind.as_str(), message.as_str()),
        ("alert", "Saved"),
        "{err:?}"
    );
    drop(server);
}

/// observe is not a `browser_*` tool: a dialog that ends its observation
/// still answers `dialog_open` with the dialog's type and message.
#[test]
fn an_observe_cut_short_by_an_alert_carries_its_type_and_message() {
    let mut setup = Setup::page(ScriptBuilder::new().observe_compact(&page()));
    setup.alert_on = Some("Page.getLayoutMetrics");
    let (mut server, _chrome) = setup.start();
    let err = server
        .call_tool("observe", &json!({}))
        .expect_err("the alert interrupts observe");
    let ToolError::DialogOpen { kind, message, .. } = &err else {
        panic!("{err:?}");
    };
    assert_eq!(
        (kind.as_str(), message.as_str()),
        ("alert", "Saved"),
        "{err:?}"
    );
    drop(server);
}

/// guard takes a `browser_get_state` snapshot of the bound tab as
/// `seen_snapshot`, and refuses one taken on another tab.
#[test]
fn guard_takes_a_get_state_snapshot_of_its_tab_only() {
    let mut setup = Setup::page(
        ScriptBuilder::new()
            .observe_compact(&page())
            .observe_compact(&page()),
    );
    setup
        .next_pages
        .push(ScriptBuilder::new().ready_state("complete"));
    let (mut server, _chrome) = setup.start();
    let guard = |snapshot: &Value| json!({"target": "Sign in", "role": "button", "seen_snapshot": snapshot});
    let seen = call(&mut server, "browser_get_state", json!({}));
    let decision = call(&mut server, "guard", guard(&seen["snapshot"]));
    assert_eq!(decision["decision"], "allow", "{decision}");
    call(
        &mut server,
        "browser_new_tab",
        json!({"url": "https://shop.example/cart"}),
    );
    let err = server
        .call_tool("guard", &guard(&seen["snapshot"]))
        .expect_err("a snapshot of another tab");
    assert!(
        matches!(&err, ToolError::InvalidArguments(m) if m.contains("browser:pipe:T1")),
        "{err:?}"
    );
    drop(server);
}
