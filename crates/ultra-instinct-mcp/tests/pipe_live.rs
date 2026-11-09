//! `Server::with_pipe` against a real Chrome on `--remote-debugging-pipe`.
//! Ignored by default:
//! `cargo test -p ultra-instinct-mcp --test pipe_live -- --ignored --nocapture`.
//!
//! As in ultra-instinct-cdp's pipe_live: `/bin/sh` places the child's stdin
//! pipe at fd 3 and its stdout pipe at fd 4 (no `pre_exec`, no unsafe).

#![cfg(unix)]

use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::{json, Value};
use ultra_instinct_cdp::PipeBrowser;
use ultra_instinct_mcp::{Server, ToolError};

const SECRET: &str = "pipe-live-S3cret";

fn chrome() -> Option<PathBuf> {
    if let Some(path) = std::env::var_os("ULTRA_INSTINCT_CHROME") {
        return Some(PathBuf::from(path));
    }
    [
        "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome",
        "/usr/bin/google-chrome",
        "/usr/bin/chromium",
        "/usr/bin/chromium-browser",
    ]
    .into_iter()
    .map(PathBuf::from)
    .find(|path| path.is_file())
}

/// A login form; "Sign in" puts a banner button on top of the page (every
/// control moves down) and says so.
const PAGE: &str = "data:text/html,<title>pipe live</title><div id=top></div>\
<label>Email <input id=e type=email></label><br>\
<label>Password <input id=p type=password></label><br>\
<button onclick=\"document.getElementById('top').innerHTML='<button>Dismiss banner</button>';\
document.title='signed in';this.textContent='Signed in'\">Sign in</button>";

fn call(server: &mut Server, tool: &str, arguments: Value) -> Value {
    server
        .call_tool(tool, &arguments)
        .unwrap_or_else(|err| panic!("{tool} {arguments}: {}", err.to_json_string()))
}

fn index_of(state: &Value, label: &str) -> u64 {
    state["elements"]
        .as_array()
        .unwrap()
        .iter()
        .find(|e| e["label"] == label)
        .unwrap_or_else(|| panic!("no {label} in {state}"))["index"]
        .as_u64()
        .unwrap()
}

/// Chrome on a pipe: the child, its profile dir, and the browser handle.
fn launch() -> (std::process::Child, PathBuf, PipeBrowser) {
    let binary = chrome().expect("set ULTRA_INSTINCT_CHROME or install Chrome");
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock")
        .as_nanos();
    let profile = std::env::temp_dir().join(format!("ui-mcp-pipe-live-{nanos}"));
    let mut child = Command::new("/bin/sh")
        .arg("-c")
        .arg(r#"exec "$0" "$@" 3<&0 4>&1 </dev/null 1>&2"#)
        .arg(&binary)
        .arg("--headless=new")
        .arg("--remote-debugging-pipe")
        .arg("--no-first-run")
        .arg("--no-default-browser-check")
        .arg(format!("--user-data-dir={}", profile.display()))
        .arg("about:blank")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn chrome");
    let writer = child.stdin.take().expect("stdin");
    let reader = child.stdout.take().expect("stdout");
    (child, profile, PipeBrowser::new(reader, writer))
}

#[test]
#[ignore = "needs a local Chrome"]
fn the_server_drives_a_real_chrome_over_the_pipe() {
    let (mut child, profile, browser) = launch();
    let mut server = Server::with_pipe(browser)
        .require_snapshot(true)
        .with_tool_allowlist([
            "browser_navigate",
            "browser_get_state",
            "browser_get_text",
            "browser_click",
            "browser_type",
            "browser_new_tab",
            "browser_list_tabs",
            "browser_switch_tab",
            "browser_close_tab",
            "browser_screenshot",
        ]);

    call(&mut server, "browser_navigate", json!({"url": PAGE}));
    let state = call(&mut server, "browser_get_state", json!({"format": "json"}));
    let snapshot = state["snapshot"].as_u64().unwrap();
    let password = &state["elements"][index_of(&state, "Password") as usize - 1];
    assert_eq!(password["sensitive"], true, "{state}");
    assert_eq!(password["input_type"], "password", "{state}");
    let email = &state["elements"][index_of(&state, "Email") as usize - 1];
    assert_eq!(email["sensitive"], false, "{state}");

    call(
        &mut server,
        "browser_type",
        json!({"index": index_of(&state, "Password"), "text": SECRET, "snapshot": snapshot}),
    );
    let text_state = call(&mut server, "browser_get_state", json!({}));
    assert!(!text_state.to_string().contains(SECRET), "{text_state}");
    let snapshot = text_state["snapshot"].as_u64().unwrap();
    let json_state = call(&mut server, "browser_get_state", json!({"format": "json"}));
    assert!(!json_state.to_string().contains(SECRET), "{json_state}");

    // A bare index refuses: this server binds presses to snapshots.
    let err = server
        .call_tool("browser_click", &json!({"index": 3}))
        .expect_err("snapshot required");
    assert_eq!(err.code(), "snapshot_required");

    let sign_in = index_of(&json_state, "Sign in");
    let clicked = call(
        &mut server,
        "browser_click",
        json!({"index": sign_in, "snapshot": snapshot, "mouse": true}),
    );
    assert_eq!(clicked["acted"], "click", "{clicked}");
    let page_text = call(&mut server, "browser_get_text", json!({}));
    assert!(
        page_text["text"].as_str().unwrap().contains("Signed in"),
        "{page_text}"
    );
    // The banner moved every control: the old snapshot refuses, no press.
    let err = server
        .call_tool(
            "browser_click",
            &json!({"index": sign_in, "snapshot": snapshot}),
        )
        .expect_err("stale");
    assert_eq!(err.code(), "stale_ref", "{}", err.to_json_string());

    let tab = call(
        &mut server,
        "browser_new_tab",
        json!({"url": "data:text/html,<title>second</title>two"}),
    );
    let tab_id = tab["tab_id"].as_str().unwrap().to_owned();
    let tabs = call(&mut server, "browser_list_tabs", json!({}));
    assert_eq!(tabs["tabs"].as_array().unwrap().len(), 2, "{tabs}");
    let closed = call(&mut server, "browser_close_tab", json!({"tab_id": tab_id}));
    assert_eq!(closed["closed"], true);
    let tabs = call(&mut server, "browser_list_tabs", json!({}));
    assert_eq!(tabs["tabs"].as_array().unwrap().len(), 1, "{tabs}");

    let shot = call(&mut server, "browser_screenshot", json!({}));
    assert!(shot["base64"].as_str().unwrap().len() > 1000, "a real PNG");
    assert_eq!(shot["format"], "png");

    let err = server
        .call_tool("browser_exec", &json!({"script": "1"}))
        .expect_err("off the allow-list");
    assert_eq!(
        err,
        ToolError::ToolNotAllowed {
            tool: "browser_exec".into()
        }
    );
    assert_eq!(err.to_value()["code"], "tool_not_allowed");
    println!(
        "pipe live: sensitive password, snapshot click, stale refusal, tabs, \
         screenshot ({} base64 chars), allow-list refusal: ok",
        shot["base64"].as_str().unwrap().len()
    );

    drop(server);
    let _ = child.wait();
    let _ = std::fs::remove_dir_all(&profile);
}

/// A CVV in a plain `type=text` box: masking cannot reach the screenshot's
/// pixels, so the capture draws it as dots and puts the style back. The page
/// records the field's computed `-webkit-text-security` on every style
/// change it sees.
const CARD: &str = "data:text/html,<title>card</title>\
<label>CVV <input id=c type=text style='width: 80px'></label>\
<script>window.seen=[];var c=document.getElementById('c');\
new MutationObserver(function(){seen.push(getComputedStyle(c).webkitTextSecurity)})\
.observe(c,{attributes:true,attributeFilter:['style']});</script>";

#[test]
#[ignore = "needs a local Chrome"]
fn a_screenshot_hides_a_typed_cvv_and_restores_the_field() {
    let (mut child, profile, browser) = launch();
    let mut server = Server::with_pipe(browser);
    call(&mut server, "browser_navigate", json!({"url": CARD}));
    let state = call(&mut server, "browser_get_state", json!({"format": "json"}));
    let cvv = index_of(&state, "CVV");
    assert_eq!(
        state["elements"][cvv as usize - 1]["sensitive"],
        true,
        "{state}"
    );
    call(
        &mut server,
        "browser_type",
        json!({"index": cvv, "text": "4321", "snapshot": state["snapshot"]}),
    );
    let before = call(
        &mut server,
        "browser_exec",
        json!({"script": "document.getElementById('c').getAttribute('style')"}),
    );
    let shot = call(&mut server, "browser_screenshot", json!({}));
    assert!(shot["base64"].as_str().unwrap().len() > 1000, "a real PNG");
    assert!(shot.get("restore_error").is_none(), "{shot}");
    let after = call(
        &mut server,
        "browser_exec",
        json!({"script": "JSON.stringify({style: document.getElementById('c').getAttribute('style'), \
                value: document.getElementById('c').value === '4321', seen: window.seen})"}),
    );
    let after: Value = serde_json::from_str(after["result"].as_str().unwrap()).unwrap();
    assert_eq!(
        after["style"], before["result"],
        "the style is back: {after}"
    );
    assert_eq!(after["value"], true, "the value never changed: {after}");
    assert_eq!(
        after["seen"],
        json!(["disc", "none"]),
        "hidden, then shown: {after}"
    );
    println!("pipe live: screenshot hid the CVV and restored it: {after}");
    drop(server);
    let _ = child.wait();
    let _ = std::fs::remove_dir_all(&profile);
}

/// A download begun on a second tab reaches that tab's `browser_download`.
/// (Chrome 156 sends the `Browser.download*` events on the session that set
/// the download behaviour; ones without a `sessionId` are routed by frame,
/// pinned by the cdp crate's `tests/pipe.rs`.)
#[test]
#[ignore = "needs a local Chrome"]
fn a_download_on_a_second_tab_reaches_that_tab() {
    let (mut child, profile, browser) = launch();
    let mut server = Server::with_pipe(browser);
    call(&mut server, "browser_get_state", json!({}));
    call(
        &mut server,
        "browser_new_tab",
        json!({"url": "data:text/html,<a href='data:application/octet-stream,hello' download='r.bin'>Get report</a>"}),
    );
    let state = call(&mut server, "browser_get_state", json!({"format": "json"}));
    let link = index_of(&state, "Get report");
    let dest = profile.with_extension("bin");
    let got = call(
        &mut server,
        "browser_download",
        json!({"index": link, "snapshot": state["snapshot"], "path": dest.display().to_string()}),
    );
    assert_eq!(std::fs::read(&dest).unwrap(), b"hello", "{got}");
    println!("pipe live: download on tab two: {got}");
    let _ = std::fs::remove_file(&dest);
    drop(server);
    let _ = child.wait();
    let _ = std::fs::remove_dir_all(&profile);
}
