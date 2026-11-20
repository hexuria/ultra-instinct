//! P5.6: the action policy file allows / denies tools by name.

use serde_json::json;
use ultra_instinct_mcp::{Server, ToolPolicy, TOOLS};

#[test]
fn deny_takes_tools_off_and_allow_is_the_whole_list() {
    let deny = ToolPolicy::parse(r#"{"deny": ["browser_exec", "run_goal"]}"#).unwrap();
    assert!(!deny.allows("browser_exec") && !deny.allows("run_goal"));
    assert!(deny.allows("browser_click"));
    assert_eq!(deny.allowed().count(), TOOLS.len() - 1);

    let both = ToolPolicy::parse(
        r#"{"allow": ["browser_get_text", "browser_exec"], "deny": ["browser_exec"]}"#,
    )
    .unwrap();
    assert_eq!(both.allowed().collect::<Vec<_>>(), ["browser_get_text"]);
    assert_eq!(
        ToolPolicy::parse("{}").unwrap().allowed().count(),
        TOOLS.len() + 1
    );
}

#[test]
fn a_typo_or_a_wrong_shape_is_an_error_not_an_open_tool() {
    for (bad, says) in [
        (
            r#"{"deny": ["browser_exce"]}"#,
            "`browser_exce` in `deny` is not a tool",
        ),
        (r#"{"denied": []}"#, "unknown key `denied`"),
        (r#"{"deny": "browser_exec"}"#, "`deny` is not a list"),
        (r#"{"allow": [1]}"#, "not a tool name"),
        (
            r#"{"deny": ["browser_exec"], "deny": []}"#,
            "`deny` is given twice",
        ),
        ("[]", "not a JSON object"),
        ("{", "EOF"),
    ] {
        let err = ToolPolicy::parse(bad).unwrap_err();
        assert!(err.contains(says), "{bad}: {err}");
    }
}

#[test]
fn a_denied_tool_refuses_with_the_stable_code_and_is_not_listed() {
    let policy = ToolPolicy::parse(r#"{"deny": ["browser_exec"]}"#).unwrap();
    let mut server = Server::with_connector(|_| panic!("no CDP call for a denied tool"))
        .with_tool_policy(&policy);
    let err = server
        .call_tool("browser_exec", &json!({"script": "1"}))
        .unwrap_err();
    assert_eq!(err.code(), "tool_not_allowed");
    assert!(!server.allows("browser_exec") && server.allows("browser_click"));
    let listed = server
        .handle_line(r#"{"jsonrpc":"2.0","id":1,"method":"tools/list"}"#)
        .unwrap();
    assert!(!listed.contains("\"browser_exec\"") && listed.contains("\"browser_click\""));
}

#[test]
fn a_policy_narrows_an_allow_list_and_never_widens_it() {
    let policy = ToolPolicy::parse(r#"{"deny": ["browser_click"]}"#).unwrap();
    let server = Server::with_connector(|_| panic!("unused"))
        .with_tool_allowlist(["browser_click", "browser_get_text"])
        .with_tool_policy(&policy);
    assert!(server.allows("browser_get_text"));
    assert!(!server.allows("browser_click") && !server.allows("browser_exec"));
}

#[test]
fn a_policy_link_to_a_missing_file_is_found_and_fails_to_read() {
    let dir = std::env::temp_dir().join(format!("aui-policy-link-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    assert_eq!(ultra_instinct_mcp::find_policy_file(&dir), None);
    std::os::unix::fs::symlink(dir.join("gone.json"), dir.join("aui-policy.json")).unwrap();
    let found = ultra_instinct_mcp::find_policy_file(&dir).expect("the link is found");
    assert!(ToolPolicy::read(&found).is_err());
    std::fs::remove_dir_all(&dir).unwrap();
}
