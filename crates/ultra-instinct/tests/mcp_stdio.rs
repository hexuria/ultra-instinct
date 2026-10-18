//! The `ultra-instinct mcp` binary speaks JSON-RPC. This is not a name-only stub.

use std::io::{BufRead, BufReader, Write};
use std::path::PathBuf;
use std::process::{Command, Stdio};

fn fixture(name: &str) -> String {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../fixtures")
        .join(name)
        .to_str()
        .expect("utf-8 path")
        .to_owned()
}

#[test]
fn stdio_server_lists_tools_and_locates() {
    let mut child = Command::new(env!("CARGO_BIN_EXE_ultra-instinct"))
        .arg("mcp")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn");
    let mut stdin = child.stdin.take().expect("stdin");
    let stdout = child.stdout.take().expect("stdout");
    let mut reader = BufReader::new(stdout);
    let path = fixture("sign-in.cdp.json");
    assert!(!path.contains('"'), "{path}");
    let locate = format!(
        r#"{{"jsonrpc":"2.0","id":3,"method":"tools/call","params":{{"name":"locate","arguments":{{"fixture":"{path}","text":"Sign in"}}}}}}"#
    );
    let script = format!(
        "{}\n{}\n{}\n{}\n",
        r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2024-11-05","capabilities":{},"clientInfo":{"name":"stdio","version":"0"}}}"#,
        r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#,
        r#"{"jsonrpc":"2.0","id":2,"method":"tools/list"}"#,
        locate
    );
    stdin.write_all(script.as_bytes()).unwrap();
    drop(stdin);
    let mut lines = Vec::new();
    let mut buf = String::new();
    while reader.read_line(&mut buf).unwrap() > 0 {
        lines.push(buf.trim().to_owned());
        buf.clear();
    }
    let status = child.wait().unwrap();
    assert!(status.success(), "{status:?} lines={lines:?}");
    assert_eq!(lines.len(), 3, "{lines:?}");
    assert!(
        lines[0].contains(r#""name":"ultra-instinct""#),
        "{}",
        lines[0]
    );
    assert!(lines[1].contains(r#""name":"observe""#), "{}", lines[1]);
    assert!(lines[1].contains(r#""name":"verify""#), "{}", lines[1]);
    // There is no bare `navigate` tool; `browser_navigate` is the host surface.
    assert!(!lines[1].contains(r#""name":"navigate""#), "{}", lines[1]);
    assert!(
        lines[1].contains(r#""name":"browser_navigate""#),
        "{}",
        lines[1]
    );
    assert!(lines[2].contains("n100"), "{}", lines[2]);
    assert!(lines[2].contains("weighted"), "{}", lines[2]);
    assert!(lines[2].contains("benchmark"), "{}", lines[2]);
    assert!(!lines[2].contains("\"executed\""), "{}", lines[2]);
    assert!(!lines[2].contains("\"verified\""), "{}", lines[2]);
    assert!(!lines[2].contains("mechanism"), "{}", lines[2]);
    assert!(lines[2].contains(r#"\"action\":null"#), "{}", lines[2]);
    assert!(lines[2].contains("Sign in"), "{}", lines[2]);
}

/// One process: observe, then act with expect_text Welcome, then diff by snapshot id.
#[ignore = "actuation removed in action-firewall pivot"]
#[test]
fn observe_then_diff_by_snapshot_in_one_process() {
    let mut child = Command::new(env!("CARGO_BIN_EXE_ultra-instinct"))
        .arg("mcp")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn");
    let mut stdin = child.stdin.take().expect("stdin");
    let stdout = child.stdout.take().expect("stdout");
    let mut reader = BufReader::new(stdout);
    let sign_in = fixture("sign-in.cdp.json");
    let looped = fixture("sign-in-loop.cdp.json");
    let script = format!(
        "{}\n{}\n{}\n",
        format_args!(
            r#"{{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{{"name":"observe","arguments":{{"fixture":"{sign_in}"}}}}}}"#
        ),
        format_args!(
            r#"{{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{{"name":"act","arguments":{{"fixture":"{looped}","region":"n100","expect_text":"Welcome"}}}}}}"#
        ),
        r#"{"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"diff","arguments":{"before_snapshot":1,"after_snapshot":3}}}"#,
    );
    stdin.write_all(script.as_bytes()).unwrap();
    drop(stdin);
    let mut lines = Vec::new();
    let mut buf = String::new();
    while reader.read_line(&mut buf).unwrap() > 0 {
        lines.push(buf.trim().to_owned());
        buf.clear();
    }
    assert!(child.wait().unwrap().success(), "{lines:?}");
    assert_eq!(lines.len(), 3, "{lines:?}");
    assert!(lines[0].contains(r#"\"snapshot\":1"#), "{}", lines[0]);
    assert!(lines[1].contains(r#"\"verified\":true"#), "{}", lines[1]);
    assert!(lines[1].contains(r#"\"after_snapshot\":3"#), "{}", lines[1]);
    assert!(!lines[2].contains(r#""isError":true"#), "{}", lines[2]);
    assert!(lines[2].contains(r#"\"added\":[\"n300\"]"#), "{}", lines[2]);
    assert!(
        lines[2].contains(r#"\"removed\":[\"n100\",\"n200\"]"#),
        "{}",
        lines[2]
    );
}
