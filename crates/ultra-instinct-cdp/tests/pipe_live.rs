//! Live `--remote-debugging-pipe` round trip against a local Chrome.
//! Ignored by default: `cargo test -p ultra-instinct-cdp --test pipe_live -- --ignored`.
//!
//! Chrome reads CDP on fd 3 and writes on fd 4. std cannot place a child's
//! fds at 3 / 4 without a `pre_exec` hook, so `/bin/sh` does the remap:
//! the child's stdin pipe becomes fd 3, its stdout pipe fd 4.

#![cfg(unix)]

use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::Value;
use ultra_instinct_cdp::{CdpTransport, PipeBrowser};

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

#[test]
#[ignore = "needs a local Chrome"]
fn a_real_chrome_answers_over_the_pipe() {
    let binary = chrome().expect("set ULTRA_INSTINCT_CHROME or install Chrome");
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock")
        .as_nanos();
    let profile = std::env::temp_dir().join(format!("ui-pipe-live-{nanos}"));
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
    let browser = PipeBrowser::new(reader, writer);

    let version: Value =
        serde_json::from_str(&browser.call("Browser.getVersion", "{}").expect("version"))
            .expect("json");
    assert!(version["product"].as_str().is_some(), "{version}");

    let target = browser
        .create_target("data:text/html,<title>pipe-live</title>")
        .expect("create target");
    assert!(browser
        .targets()
        .expect("targets")
        .iter()
        .any(|t| t.target_id == target));
    let mut page = browser.page(&target).expect("attach");
    let body = page
        .call(
            "Runtime.evaluate",
            r#"{"expression":"document.title","returnByValue":true}"#,
        )
        .expect("evaluate");
    let result: Value = serde_json::from_str(&body).expect("json");
    assert_eq!(result["result"]["value"], "pipe-live");
    drop(page);
    assert_eq!(browser.close_target(&target), Ok(true));

    let _ = browser.call("Browser.close", "{}");
    drop(browser);
    let _ = child.wait();
    let _ = std::fs::remove_dir_all(&profile);
}
