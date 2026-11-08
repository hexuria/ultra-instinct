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
fn stdio_server_lists_tools_and_observes() {
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
    let observe = format!(
        r#"{{"jsonrpc":"2.0","id":3,"method":"tools/call","params":{{"name":"observe","arguments":{{"fixture":"{path}"}}}}}}"#
    );
    let script = format!(
        "{}\n{}\n{}\n{}\n",
        r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2024-11-05","capabilities":{},"clientInfo":{"name":"stdio","version":"0"}}}"#,
        r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#,
        r#"{"jsonrpc":"2.0","id":2,"method":"tools/list"}"#,
        observe
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
    for removed in ["locate", "inspect", "act", "diff"] {
        let name = format!(r#""name":"{removed}""#);
        assert!(!lines[1].contains(&name), "{removed}: {}", lines[1]);
    }
    assert!(lines[2].contains("n100"), "{}", lines[2]);
    assert!(lines[2].contains("Sign in"), "{}", lines[2]);
    assert!(lines[2].contains(r#"\"executed\":false"#), "{}", lines[2]);
    assert!(!lines[2].contains("mechanism"), "{}", lines[2]);
}
