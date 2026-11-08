//! Storage state (save and reuse a login) over the replay transport, plus the
//! state-file format: it is untrusted input on load.

use std::path::PathBuf;

use proptest::prelude::*;
use serde_json::{json, Value};
use ultra_instinct_browser::script::ScriptBuilder;
use ultra_instinct_browser::storage_state::{
    load_state, read_state_file, save_state, write_state_file, Cookie, OriginStorage, StateError,
    StorageState, WRITE_STORAGE_FUNCTION,
};
use ultra_instinct_browser::{BrowserSession, ReplayTransport};

const ORIGIN: &str = "https://app.example.test";

fn session(script: ScriptBuilder) -> BrowserSession<ReplayTransport> {
    BrowserSession::new(ReplayTransport::parse(&script.to_json()).unwrap())
}

fn scratch(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "ultra-instinct-state-{}-{name}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir.join("state.json")
}

fn cdp_cookie() -> Value {
    // Network.Cookie as Chrome sends it: extra fields are ignored.
    json!({
        "name": "sid", "value": "s3cret-token", "domain": "app.example.test", "path": "/",
        "expires": 1_900_000_000.0, "size": 15, "httpOnly": true, "secure": true,
        "session": false, "sameSite": "Lax", "priority": "Medium", "sourcePort": 443
    })
}

fn session_cookie() -> Value {
    json!({
        "name": "pref", "value": "dark", "domain": ".example.test", "path": "/",
        "expires": -1, "size": 8, "httpOnly": false, "secure": false, "session": true
    })
}

fn page_storage() -> Value {
    json!({"result": {"type": "object", "value": {
        "origin": ORIGIN, "host": "app.example.test",
        "local": {"auth": "bearer xyz", "theme": "dark"},
        "session": {"tab": "inbox"}
    }}})
}

fn saved_script() -> ScriptBuilder {
    ScriptBuilder::new()
        .call(
            "Storage.getCookies",
            json!({"cookies": [cdp_cookie(), session_cookie()]}),
        )
        .call("Runtime.evaluate", page_storage())
}

fn params(session: &BrowserSession<ReplayTransport>, method: &str) -> Vec<Value> {
    session
        .transport()
        .logged_calls()
        .iter()
        .filter(|(m, _)| m == method)
        .map(|(_, p)| serde_json::from_str(p).unwrap())
        .collect()
}

#[test]
fn save_reads_cookies_and_current_origin_storage() {
    let mut s = session(saved_script());
    let state = save_state(&mut s).unwrap();
    assert_eq!(state.cookies.len(), 2);
    let sid = &state.cookies[0];
    assert_eq!(
        (sid.name.as_str(), sid.value.as_str()),
        ("sid", "s3cret-token")
    );
    assert_eq!(sid.expires, Some(1_900_000_000.0));
    assert!(sid.http_only && sid.secure);
    assert_eq!(sid.same_site.as_deref(), Some("Lax"));
    assert_eq!(
        state.cookies[1].expires, None,
        "session cookie has no expiry"
    );
    assert_eq!(state.origins.len(), 1);
    assert_eq!(state.origins[0].origin, ORIGIN);
    assert_eq!(state.origins[0].local["auth"], "bearer xyz");
    assert_eq!(state.origins[0].session["tab"], "inbox");
    // The read is one fixed script: no value is ever spliced into it.
    let eval = &params(&s, "Runtime.evaluate")[0];
    assert_eq!(eval["returnByValue"], true);
    assert!(!eval["expression"].as_str().unwrap().contains(ORIGIN));
}

#[test]
fn save_falls_back_to_network_get_all_cookies() {
    let mut s = session(
        ScriptBuilder::new()
            .call_error("Storage.getCookies", "'Storage.getCookies' wasn't found")
            .call("Network.getAllCookies", json!({"cookies": [cdp_cookie()]}))
            .call(
                "Runtime.evaluate",
                json!({"result": {"value": {"origin": "null", "host": "", "local": null, "session": null}}}),
            ),
    );
    let state = save_state(&mut s).unwrap();
    assert_eq!(state.cookies.len(), 1);
    assert!(state.origins.is_empty(), "opaque origin keeps no storage");
}

fn load_script(current: &str) -> ScriptBuilder {
    ScriptBuilder::new()
        .call("Network.setCookies", json!({}))
        .call(
            "Runtime.evaluate",
            json!({"result": {"type": "string", "value": current}}),
        )
        .call(
            "Runtime.evaluate",
            json!({"result": {"type": "object", "objectId": "global-1"}}),
        )
        .call(
            "Runtime.callFunctionOn",
            json!({"result": {"type": "boolean", "value": true}}),
        )
        .call(
            "Runtime.evaluate",
            json!({"result": {"type": "object", "objectId": "global-1"}}),
        )
        .call(
            "Runtime.callFunctionOn",
            json!({"result": {"type": "boolean", "value": true}}),
        )
}

#[test]
fn round_trip_save_file_load_through_the_transport() {
    let path = scratch("round-trip");
    let mut saver = session(saved_script());
    let state = save_state(&mut saver).unwrap();
    write_state_file(&path, &state).unwrap();
    let loaded = read_state_file(&path).unwrap();
    assert_eq!(loaded, state);

    let mut loader = session(load_script(ORIGIN));
    let report = load_state(&mut loader, &loaded).unwrap();
    assert_eq!(report.cookies, 2);
    assert_eq!(report.applied.as_deref(), Some(ORIGIN));
    assert!(report.pending.is_empty());
    assert_eq!(loader.transport().remaining(), 0);

    let set = &params(&loader, "Network.setCookies")[0]["cookies"];
    assert_eq!(set[0]["name"], "sid");
    assert_eq!(set[0]["value"], "s3cret-token");
    assert_eq!(set[0]["httpOnly"], true);
    assert_eq!(set[0]["sameSite"], "Lax");
    assert!(
        set[1].get("expires").is_none(),
        "session cookie stays a session cookie"
    );

    // Storage values travel as CDP arguments to one fixed function.
    let writes = params(&loader, "Runtime.callFunctionOn");
    assert_eq!(writes.len(), 2);
    for write in &writes {
        assert_eq!(write["functionDeclaration"], WRITE_STORAGE_FUNCTION);
        assert_eq!(write["objectId"], "global-1");
        assert_eq!(write["arguments"][0]["value"], ORIGIN);
        assert_eq!(write["arguments"][2]["value"], "set");
    }
    assert_eq!(writes[0]["arguments"][1]["value"], "local");
    assert_eq!(writes[0]["arguments"][3]["value"]["auth"], "bearer xyz");
    assert_eq!(writes[1]["arguments"][1]["value"], "session");
    assert_eq!(writes[1]["arguments"][3]["value"]["tab"], "inbox");
    assert!(!WRITE_STORAGE_FUNCTION.contains("bearer"));
}

#[test]
fn storage_for_another_origin_is_pending() {
    let state = StorageState {
        cookies: Vec::new(),
        origins: vec![OriginStorage {
            origin: ORIGIN.into(),
            local: [("k".to_owned(), "v".to_owned())].into(),
            ..OriginStorage::default()
        }],
    };
    let mut s = session(ScriptBuilder::new().call(
        "Runtime.evaluate",
        json!({"result": {"type": "string", "value": "null"}}),
    ));
    let report = load_state(&mut s, &state).unwrap();
    assert_eq!(report.cookies, 0);
    assert_eq!(report.applied, None);
    assert_eq!(report.pending, vec![ORIGIN.to_owned()]);
    assert!(
        params(&s, "Network.setCookies").is_empty(),
        "no cookies, no call"
    );
}

#[test]
fn a_page_that_throws_on_write_is_an_error() {
    let state = StorageState::parse(
        r#"{"version":1,"cookies":[],"origins":[{"origin":"https://app.example.test","local":{"a":"b"}}]}"#,
    )
    .unwrap();
    let mut s = session(
        ScriptBuilder::new()
            .call("Runtime.evaluate", json!({"result": {"value": ORIGIN}}))
            .call("Runtime.evaluate", json!({"result": {"objectId": "g"}}))
            .call(
                "Runtime.callFunctionOn",
                json!({"result": {}, "exceptionDetails": {"text": "Uncaught", "exception": {"description": "Error: page left https://app.example.test\n at x"}}}),
            ),
    );
    let err = load_state(&mut s, &state).unwrap_err();
    assert!(err.to_string().contains("page left"), "{err}");
}

#[cfg(unix)]
#[test]
fn files_are_written_0600_and_loose_files_are_refused() {
    use std::os::unix::fs::PermissionsExt;
    let path = scratch("modes");
    // Overwriting a loose file still leaves a 0600 file.
    std::fs::write(&path, "{}").unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();
    write_state_file(&path, &StorageState::default()).unwrap();
    let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
    assert_eq!(mode, 0o600);
    assert_eq!(read_state_file(&path).unwrap(), StorageState::default());
    let dir = path.parent().unwrap();
    let leftovers: Vec<_> = std::fs::read_dir(dir).unwrap().collect();
    assert_eq!(leftovers.len(), 1, "no temp file left behind");

    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();
    let err = read_state_file(&path).unwrap_err();
    assert!(
        matches!(err, StateError::Permissions { mode: 0o644, .. }),
        "{err:?}"
    );
    assert!(err.to_string().contains("chmod 600"), "{err}");
}

#[test]
fn versioned_format_errors_are_exact() {
    let cases = [
        ("[]", "top level must be an object"),
        ("not json", "not JSON"),
        (r#"{"cookies":[]}"#, "missing integer `version`"),
        (
            r#"{"version":2,"cookies":[]}"#,
            "unsupported version 2; this build reads 1",
        ),
        (r#"{"version":"1"}"#, "missing integer `version`"),
        (r#"{"version":1,"extra":0}"#, "unknown field `extra`"),
        (
            r#"{"version":1,"cookies":{}}"#,
            "`cookies` must be an array",
        ),
        (
            r#"{"version":1,"cookies":[{"name":"a","value":"b","domain":"d","path":"/","sameSite":"lax"}]}"#,
            "sameSite",
        ),
        (
            r#"{"version":1,"cookies":[{"name":"a","value":"b","domain":"d","path":"/","size":3}]}"#,
            "unknown cookie field `size`",
        ),
        (
            r#"{"version":1,"cookies":[{"name":"","value":"b","domain":"d","path":"/"}]}"#,
            "cookie needs a name",
        ),
        (
            r#"{"version":1,"origins":[{"origin":"https://a.test/path"}]}"#,
            "`origin` must be",
        ),
        (
            r#"{"version":1,"origins":[{"origin":"javascript:alert(1)"}]}"#,
            "`origin` must be",
        ),
        (
            r#"{"version":1,"origins":[{"origin":"https://a.test"},{"origin":"https://a.test"}]}"#,
            "duplicate origin https://a.test",
        ),
        (
            r#"{"version":1,"origins":[{"origin":"https://a.test","local":{"k":1}}]}"#,
            "local `k` must be a string",
        ),
    ];
    for (text, needle) in cases {
        let err = StorageState::parse(text).unwrap_err();
        assert!(matches!(err, StateError::Format(_)), "{text}: {err:?}");
        assert!(err.to_string().contains(needle), "{text}: {err}");
    }
    assert_eq!(
        StorageState::parse(r#"{"version":1}"#).unwrap(),
        StorageState::default()
    );
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(64))]

    #[test]
    fn garbage_never_panics(raw in "\\PC{0,200}") {
        let _ = StorageState::parse(&raw);
    }

    #[test]
    fn json_shaped_garbage_never_panics(
        version in prop_oneof![Just(json!(1)), Just(json!(2)), Just(json!(-1)), Just(json!(null)), Just(json!(1.5))],
        cookie in prop::collection::btree_map("[a-zA-Z]{1,8}", prop_oneof![
            Just(json!(null)), Just(json!(true)), Just(json!(3)), "\\PC{0,8}".prop_map(Value::from)
        ], 0..6),
        origin in "\\PC{0,24}",
    ) {
        let doc = json!({"version": version, "cookies": [cookie], "origins": [{"origin": origin}]});
        let _ = StorageState::parse(&doc.to_string());
    }

    #[test]
    fn structured_states_round_trip(
        cookies in prop::collection::vec((
            "[A-Za-z0-9_]{1,8}", "\\PC{0,16}", "[a-z]{1,8}\\.test", "/[a-z]{0,6}",
            prop::option::of(1u32..2_000_000_000u32), any::<bool>(), any::<bool>(),
            prop::option::of(prop_oneof![Just("Strict"), Just("Lax"), Just("None")]),
        ), 0..4),
        local in prop::collection::btree_map("\\PC{0,8}", "\\PC{0,16}", 0..4),
        session in prop::collection::btree_map("\\PC{0,8}", "\\PC{0,16}", 0..4),
        host in "[a-z]{1,10}(:[0-9]{2,4})?",
    ) {
        let state = StorageState {
            cookies: cookies.into_iter().map(|(name, value, domain, path, expires, http_only, secure, same_site)| Cookie {
                name, value, domain, path,
                expires: expires.map(f64::from),
                http_only, secure,
                same_site: same_site.map(str::to_owned),
            }).collect(),
            origins: vec![OriginStorage { origin: format!("https://{host}"), local, session }],
        };
        let text = state.to_json().to_string();
        prop_assert_eq!(StorageState::parse(&text).unwrap(), state);
    }
}
