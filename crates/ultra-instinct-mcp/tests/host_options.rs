//! Host options on `Server`: runtime policy, per-call confirm, output
//! limits, caller-chosen absolute paths, downloads, and the tool allow-list.

use std::cell::RefCell;
use std::rc::Rc;
use std::time::{Duration, Instant};

use serde_json::{json, Value};
use ultra_instinct_browser::script::{Control, PageSpec, ScriptBuilder};
use ultra_instinct_browser::{CdpError, CdpTransport, ReplayTransport};
use ultra_instinct_guard::Policy;
use ultra_instinct_mcp::{Downloads, Server, ToolError, ToolLimits};

const CDP: &str = "http://127.0.0.1:9222";

type Log = Rc<RefCell<Vec<String>>>;

struct Logged {
    inner: ReplayTransport,
    log: Log,
}

impl CdpTransport for Logged {
    fn call(&mut self, method: &str, params_json: &str) -> Result<String, CdpError> {
        self.log.borrow_mut().push(method.to_owned());
        self.inner.call(method, params_json)
    }
}

/// One connect only: a second connect fails, so a test that passes kept
/// its session open.
fn replay(script: ScriptBuilder) -> (Server, Log) {
    let log: Log = Rc::new(RefCell::new(Vec::new()));
    let shared = Rc::clone(&log);
    let mut script = Some(script);
    let server = Server::with_connector(move |_url| {
        let script = script.take().ok_or_else(|| CdpError::Transport {
            message: "one connect per test".into(),
        })?;
        Ok(Box::new(Logged {
            inner: ReplayTransport::parse(&script.to_json())?,
            log: Rc::clone(&shared),
        }) as Box<dyn CdpTransport>)
    });
    (server, log)
}

fn call(server: &mut Server, tool: &str, arguments: Value) -> Value {
    server
        .call_tool(tool, &arguments)
        .unwrap_or_else(|err| panic!("{tool} {arguments}: {err:?}"))
}

fn clicks(log: &Log) -> usize {
    log.borrow()
        .iter()
        .filter(|m| *m == "Runtime.callFunctionOn")
        .count()
}

/// Email @e1, Sign in @e2.
fn page() -> PageSpec {
    PageSpec::of(
        &[
            Control::text_field(10, 100, "Email", (20.0, 20.0, 300.0, 28.0)),
            Control::button(11, 101, "Sign in", (20.0, 80.0, 120.0, 28.0)),
        ],
        "http://127.0.0.1/form",
        "Form",
    )
}

fn confirm_sign() -> Policy {
    Policy::from_lists("", "sign*").unwrap()
}

#[test]
fn set_policy_applies_to_the_next_call_with_the_session_still_open() {
    let (mut server, log) = replay(
        ScriptBuilder::new()
            .observe_compact(&page())
            .observe_compact(&page())
            .observe_compact(&page())
            .dom_click(11),
    );
    server.set_policy(confirm_sign());
    let err = server
        .call_tool("browser_click", &json!({"cdp": CDP, "index": 2}))
        .expect_err("needs confirm");
    assert_eq!(err.code(), "needs_confirm");
    assert_eq!(clicks(&log), 0);
    server.set_policy(Policy::default());
    let out = call(
        &mut server,
        "browser_click",
        json!({"cdp": CDP, "index": 2}),
    );
    assert_eq!(out["region"], "n101", "{out}");
    assert_eq!(clicks(&log), 1);
    assert_eq!(server.live_sessions().len(), 1, "same session throughout");
}

#[test]
fn a_confirm_lifts_its_own_call_only() {
    let (mut server, log) = replay(
        ScriptBuilder::new()
            .observe_compact(&page())
            .observe_compact(&page())
            .dom_click(11)
            .observe_compact(&page()),
    );
    server.set_policy(confirm_sign());
    let out = call(
        &mut server,
        "browser_click",
        json!({"cdp": CDP, "index": 2, "confirm": "Sign in"}),
    );
    assert_eq!(out["acted"], "click", "{out}");
    assert_eq!(clicks(&log), 1);
    let err = server
        .call_tool("browser_click", &json!({"cdp": CDP, "index": 2}))
        .expect_err("the confirm was for the call before");
    assert_eq!(
        err,
        ToolError::NeedsConfirm {
            label: "Sign in".into()
        }
    );
    assert_eq!(clicks(&log), 1, "no second press");
}

#[test]
fn get_state_caps_elements_and_text_with_a_marker() {
    let (server, _log) = replay(
        ScriptBuilder::new()
            .observe_compact(&page())
            .observe_compact(&page()),
    );
    let mut server = server.with_limits(ToolLimits {
        max_elements: 1,
        max_state_chars: 30,
        ..ToolLimits::default()
    });
    let json_state = call(
        &mut server,
        "browser_get_state",
        json!({"cdp": CDP, "format": "json"}),
    );
    assert_eq!(json_state["elements"].as_array().unwrap().len(), 1);
    assert_eq!(json_state["dropped_elements"], 1);
    assert_eq!(json_state["truncated"], true);
    let text = call(&mut server, "browser_get_state", json!({"cdp": CDP}));
    let body = text["text"].as_str().unwrap();
    assert_eq!(text["truncated"], true, "{text}");
    assert!(body.contains("… [truncated "), "{body}");
    assert!(body.chars().count() < 60, "{body}");
}

#[test]
fn defaults_are_documented_values() {
    let limits = ToolLimits::default();
    assert_eq!(limits.max_state_chars, 100_000);
    assert_eq!(limits.max_elements, 1_000);
    assert_eq!(limits.max_screenshot_bytes, 16 << 20);
    assert_eq!(limits.max_wait, Duration::from_secs(60));
    assert_eq!(limits.max_text_chars, 1_000_000);
    assert_eq!(Downloads::default(), Downloads::Anywhere);
}

fn shot(base64: &str) -> Value {
    json!({"data": base64})
}

#[test]
fn a_screenshot_over_the_cap_retries_as_jpeg_then_refuses() {
    let big = "A".repeat(400);
    let small = "B".repeat(40);
    let (server, log) = replay(
        ScriptBuilder::new()
            .call("Page.captureScreenshot", shot(&big))
            .call("Page.captureScreenshot", shot(&small))
            .call("Page.captureScreenshot", shot(&big))
            .call("Page.captureScreenshot", shot(&big)),
    );
    let mut server = server.with_limits(ToolLimits {
        max_screenshot_bytes: 100,
        ..ToolLimits::default()
    });
    let out = call(&mut server, "browser_screenshot", json!({"cdp": CDP}));
    assert_eq!(out["format"], "jpeg", "{out}");
    assert_eq!(out["base64"], small, "base64 stays without a path");
    let err = server
        .call_tool("browser_screenshot", &json!({"cdp": CDP}))
        .expect_err("still over the cap");
    assert_eq!(
        err,
        ToolError::OutputTooLarge {
            limit: 100,
            bytes: 300
        }
    );
    assert_eq!(err.to_value()["code"], "output_too_large");
    assert_eq!(
        log.borrow()
            .iter()
            .filter(|m| *m == "Page.captureScreenshot")
            .count(),
        4
    );
}

#[test]
fn browser_wait_is_capped_at_max_wait() {
    let (server, _log) = replay(ScriptBuilder::new());
    let mut server = server.with_limits(ToolLimits {
        max_wait: Duration::from_millis(20),
        ..ToolLimits::default()
    });
    let started = Instant::now();
    let out = call(&mut server, "browser_wait", json!({"seconds": 30}));
    assert!(started.elapsed() < Duration::from_secs(2));
    assert_eq!(out["waited"], 0.02, "{out}");
}

#[test]
fn file_paths_must_be_absolute_and_refuse_before_any_cdp_call() {
    for (tool, arguments) in [
        (
            "browser_screenshot",
            json!({"cdp": CDP, "path": "shot.png"}),
        ),
        (
            "browser_save_as_pdf",
            json!({"cdp": CDP, "path": "out/r.pdf"}),
        ),
        (
            "browser_state_save",
            json!({"cdp": CDP, "path": "login.json"}),
        ),
        (
            "browser_state_load",
            json!({"cdp": CDP, "path": "./login.json"}),
        ),
        (
            "browser_download",
            json!({"cdp": CDP, "index": 1, "path": "r.pdf"}),
        ),
        (
            "browser_network",
            json!({"cdp": CDP, "action": "har_stop", "path": "r.har"}),
        ),
    ] {
        let (mut server, log) = replay(ScriptBuilder::new());
        let err = server.call_tool(tool, &arguments).expect_err(tool);
        assert!(
            matches!(&err, ToolError::PathNotAbsolute { path } if Some(path.as_str()) == arguments["path"].as_str()),
            "{tool}: {err:?}"
        );
        assert_eq!(err.to_value()["code"], "path_not_absolute");
        assert!(
            log.borrow().iter().all(|m| m == "Page.enable"),
            "{tool}: {:?}",
            log.borrow()
        );
    }
}

#[test]
fn an_absolute_screenshot_path_is_written_and_base64_stays() {
    let png = "iVBORw0KGgo="; // 8 bytes
    let (mut server, _log) = replay(ScriptBuilder::new().call("Page.captureScreenshot", shot(png)));
    let path = std::env::temp_dir().join(format!("ui-shot-{}.png", std::process::id()));
    let out = call(
        &mut server,
        "browser_screenshot",
        json!({"cdp": CDP, "path": path.to_str().unwrap()}),
    );
    assert_eq!(out["base64"], png);
    assert_eq!(out["bytes"], 8);
    assert_eq!(std::fs::read(&path).unwrap().len(), 8);
    let _ = std::fs::remove_file(&path);
}

#[test]
fn downloads_off_or_another_folder_refuse_before_any_press() {
    let (server, log) = replay(ScriptBuilder::new());
    let mut off = server.with_downloads(Downloads::Off);
    let err = off
        .call_tool(
            "browser_download",
            &json!({"cdp": CDP, "index": 1, "path": "/tmp/r.pdf"}),
        )
        .expect_err("off");
    assert_eq!(err, ToolError::DownloadsOff);
    assert_eq!(err.to_value()["code"], "downloads_off");

    let (server, _) = replay(ScriptBuilder::new());
    let dir = std::env::temp_dir().join("ui-downloads-only");
    let mut boxed = server.with_downloads(Downloads::Dir(dir.clone()));
    for elsewhere in [
        "/etc/r.pdf".to_owned(),
        dir.join("sub/r.pdf").display().to_string(),
        dir.join("../r.pdf").display().to_string(),
    ] {
        let err = boxed
            .call_tool(
                "browser_download",
                &json!({"cdp": CDP, "index": 1, "path": elsewhere}),
            )
            .expect_err("outside the folder");
        assert_eq!(err.code(), "path_not_allowed", "{elsewhere}: {err:?}");
    }
    assert!(log.borrow().iter().all(|m| m == "Page.enable"));
}

#[test]
fn the_allow_list_refuses_other_tools_and_lists_only_its_own() {
    let (server, log) = replay(ScriptBuilder::new());
    let mut server = server.with_tool_allowlist(["browser_get_state", "browser_click"]);
    let err = server
        .call_tool("browser_exec", &json!({"cdp": CDP, "script": "1"}))
        .expect_err("not allowed");
    assert_eq!(
        err,
        ToolError::ToolNotAllowed {
            tool: "browser_exec".into()
        }
    );
    assert_eq!(err.to_value()["code"], "tool_not_allowed");
    assert_eq!(err.to_value()["tool"], "browser_exec");
    assert!(log.borrow().is_empty(), "no CDP call at all");
    let listed: Value = serde_json::from_str(
        &server
            .handle_line(r#"{"jsonrpc":"2.0","id":1,"method":"tools/list"}"#)
            .unwrap(),
    )
    .unwrap();
    let names: Vec<&str> = listed["result"]["tools"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["name"].as_str().unwrap())
        .collect();
    assert_eq!(names, ["browser_get_state", "browser_click"]);
    // Without an allow-list every tool is listed, as before.
    let all: Value = serde_json::from_str(
        &replay(ScriptBuilder::new())
            .0
            .handle_line(r#"{"jsonrpc":"2.0","id":1,"method":"tools/list"}"#)
            .unwrap(),
    )
    .unwrap();
    assert_eq!(
        all["result"]["tools"].as_array().unwrap().len(),
        ultra_instinct_mcp::TOOLS.len()
    );
}

/// `browser_search` is a navigation: with an allow-list it also needs
/// `browser_navigate`, else `tool_not_allowed` before any CDP call.
#[test]
fn search_needs_navigate_on_the_allow_list() {
    let (server, log) = replay(ScriptBuilder::new());
    let mut server = server.with_tool_allowlist(["browser_search"]);
    let err = server
        .call_tool("browser_search", &json!({"cdp": CDP, "query": "rust"}))
        .unwrap_err();
    assert_eq!(
        err,
        ToolError::ToolNotAllowed {
            tool: "browser_navigate".into()
        }
    );
    assert!(log.borrow().is_empty(), "{:?}", log.borrow());
}

/// Logs each `Runtime.evaluate` expression.
struct Expressions {
    inner: ReplayTransport,
    seen: Log,
}

impl CdpTransport for Expressions {
    fn call(&mut self, method: &str, params_json: &str) -> Result<String, CdpError> {
        if method == "Runtime.evaluate" {
            let params: Value = serde_json::from_str(params_json).unwrap();
            self.seen
                .borrow_mut()
                .push(params["expression"].as_str().unwrap_or_default().to_owned());
        }
        self.inner.call(method, params_json)
    }
}

/// A huge page cannot make a huge frame from get_text / get_html: the page
/// script cuts the text at `max_text_chars` and the tool marks the cut.
/// observe lists at most `max_elements` regions.
#[test]
fn text_and_html_are_cut_in_the_page_and_observe_caps_its_regions() {
    let cut =
        |text: &str, n: u64| json!({"result": {"type": "object", "value": {"t": text, "cut": n}}});
    let seen: Log = Rc::default();
    let shared = Rc::clone(&seen);
    let mut script = Some(
        ScriptBuilder::new()
            .call("Runtime.evaluate", cut("abc", 5))
            .call("Runtime.evaluate", cut("<ht", 7))
            .call("Runtime.evaluate", cut("ok", 0))
            .observe_compact(&page()),
    );
    let server = Server::with_connector(move |_url| {
        let script = script.take().ok_or_else(|| CdpError::Transport {
            message: "one connect per test".into(),
        })?;
        Ok(Box::new(Expressions {
            inner: ReplayTransport::parse(&script.to_json())?,
            seen: Rc::clone(&shared),
        }) as Box<dyn CdpTransport>)
    });
    let mut server = server.with_limits(ToolLimits {
        max_text_chars: 3,
        max_elements: 1,
        ..ToolLimits::default()
    });
    let text = call(&mut server, "browser_get_text", json!({"cdp": CDP}));
    assert_eq!(
        text,
        json!({"text": "abc… [truncated 5 chars]", "truncated": true})
    );
    let html = call(&mut server, "browser_get_html", json!({"cdp": CDP}));
    assert_eq!(
        html,
        json!({"html": "<ht… [truncated 7 chars]", "truncated": true})
    );
    let whole = call(&mut server, "browser_get_text", json!({"cdp": CDP}));
    assert_eq!(whole, json!({"text": "ok"}));
    for expression in seen.borrow().iter() {
        assert!(
            expression.ends_with("})(3)"),
            "the cap rides in the script: {expression}"
        );
    }
    let observed = call(&mut server, "observe", json!({"cdp": CDP}));
    assert_eq!(
        observed["regions"].as_array().unwrap().len(),
        1,
        "{observed}"
    );
    assert_eq!(observed["truncated"], true);
    assert_eq!(observed["dropped_regions"], 1);
}

/// `Server::run_goal` is gated by the allow-list as the name `run_goal`:
/// with an allow-list that leaves it out, the run refuses `tool_not_allowed`
/// before any CDP call, even when every press tool is allowed.
#[test]
fn run_goal_runs_only_when_the_allow_list_names_it() {
    let (server, log) = replay(ScriptBuilder::new());
    let mut server = server.with_tool_allowlist([
        "browser_get_state",
        "browser_click",
        "browser_type",
        "browser_select_dropdown",
        "browser_navigate",
    ]);
    let report = server.run_goal("click Sign in", ultra_instinct_mcp::RunLimits::default());
    assert_eq!(
        report.outcome,
        ultra_instinct_mcp::RunOutcome::Failed {
            error: "tool_not_allowed: tool `run_goal` is not allowed on this server".into(),
            code: Some("tool_not_allowed"),
        }
    );
    assert!(report.steps.is_empty());
    assert!(log.borrow().is_empty(), "no CDP call at all: {log:?}");

    // Named on the list, the run goes ahead (here it reaches the page).
    let (server, log) = replay(ScriptBuilder::new());
    let mut server = server.with_tool_allowlist(["run_goal"]);
    let report = server.run_goal(
        "click Sign in",
        ultra_instinct_mcp::RunLimits {
            max_steps: 1,
            deadline: None,
        },
    );
    assert!(
        matches!(&report.outcome, ultra_instinct_mcp::RunOutcome::Failed { error, .. } if !error.contains("tool_not_allowed")),
        "{report:?}"
    );
    assert!(!log.borrow().is_empty(), "the run observed the page");
}
