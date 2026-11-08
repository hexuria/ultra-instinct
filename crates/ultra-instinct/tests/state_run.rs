//! `run --load-state` / `--save-state` over a CDP replay: the state loads
//! before the first observe and saves when the run ends, done or not.

use serde_json::json;
use ultra_instinct_browser::script::{Control, PageSpec, ScriptBuilder};
use ultra_instinct_browser::storage_state::{read_state_file, write_state_file, StorageState};

const TOKEN: &str = "s3cret-token";

fn dir(name: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("ui-state-run-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn page() -> PageSpec {
    PageSpec::of(
        &[Control::text_field(
            10,
            100,
            "Search",
            (20.0, 20.0, 300.0, 28.0),
        )],
        "http://127.0.0.1/",
        "Search",
    )
}

fn save_calls(script: ScriptBuilder) -> ScriptBuilder {
    script
        .call(
            "Storage.getCookies",
            json!({"cookies": [{"name": "sid", "value": TOKEN, "domain": "127.0.0.1", "path": "/",
                "expires": -1, "httpOnly": true, "secure": false, "session": true}]}),
        )
        .call(
            "Runtime.evaluate",
            json!({"result": {"value": {"origin": "http://127.0.0.1", "host": "127.0.0.1",
                "local": {"auth": "x"}, "session": {}}}}),
        )
}

fn run(args: &[&str]) -> Result<String, ultra_instinct::CliError> {
    let mut all = vec!["run".to_owned()];
    all.extend(args.iter().map(|s| (*s).to_owned()));
    ultra_instinct::execute(&all)
}

#[test]
fn load_before_observe_and_save_after_the_run() {
    let dir = dir("done");
    let load = dir.join("in.json");
    let save = dir.join("out.json");
    let state = StorageState::parse(&format!(
        r#"{{"version":1,"cookies":[{{"name":"sid","value":"{TOKEN}","domain":"127.0.0.1","path":"/"}}],
            "origins":[{{"origin":"http://127.0.0.1","local":{{"auth":"x"}}}}]}}"#
    ))
    .unwrap();
    write_state_file(&load, &state).unwrap();
    let script = ScriptBuilder::new()
        .call("Network.setCookies", json!({}))
        .call(
            "Runtime.evaluate",
            json!({"result": {"value": "http://127.0.0.1"}}),
        )
        .call("Runtime.evaluate", json!({"result": {"objectId": "g"}}))
        .call("Runtime.callFunctionOn", json!({"result": {"value": true}}))
        .observe(&page())
        .observe(&page())
        .dom_input(10)
        .observe(&page())
        .dom_read_value(10, "rust", "")
        .observe(&page());
    let fixture = dir.join("search.cdp.json");
    std::fs::write(&fixture, save_calls(script).to_json()).unwrap();

    let out = run(&[
        "--goal",
        r#"Type "rust" into Search"#,
        "--policy",
        "instinct",
        "--fixture",
        fixture.to_str().unwrap(),
        "--load-state",
        load.to_str().unwrap(),
        "--save-state",
        save.to_str().unwrap(),
    ])
    .unwrap_or_else(|e| panic!("{e}"));
    assert!(
        out.starts_with("state loaded ") && out.contains("1 cookies, storage http://127.0.0.1"),
        "{out}"
    );
    assert!(out.contains("outcome done"), "{out}");
    assert!(
        out.contains("state saved ") && out.contains("1 cookies, 1 origins"),
        "{out}"
    );
    assert!(!out.contains(TOKEN), "values never printed: {out}");
    let saved = read_state_file(&save).unwrap();
    assert_eq!(saved.cookies[0].value, TOKEN);
    assert_eq!(saved.origins[0].local["auth"], "x");
}

#[test]
fn save_runs_when_the_agent_does_not_finish() {
    let dir = dir("abstain");
    let save = dir.join("out.json");
    let script = ScriptBuilder::new().observe(&page());
    let fixture = dir.join("abstain.cdp.json");
    std::fs::write(&fixture, save_calls(script).to_json()).unwrap();
    let out = run(&[
        "--goal",
        "Click Purchase",
        "--policy",
        "instinct",
        "--fixture",
        fixture.to_str().unwrap(),
        "--save-state",
        save.to_str().unwrap(),
    ]);
    let text = match out {
        Ok(text) | Err(ultra_instinct::CliError::Agent(text)) => text,
        Err(other) => panic!("{other}"),
    };
    assert!(!text.contains("outcome done"), "{text}");
    assert!(text.contains("state saved "), "{text}");
    assert!(save.exists());
}

#[test]
fn state_flags_refuse_a_static_manifold_and_a_loose_file() {
    let manifold = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../fixtures")
        .read_dir()
        .unwrap()
        .filter_map(Result::ok)
        .map(|e| e.path())
        .find(|p| p.extension().is_some_and(|x| x == "manifold"));
    if let Some(manifold) = manifold {
        let err = run(&[
            "--goal",
            "Click Go",
            "--policy",
            "instinct",
            "--fixture",
            manifold.to_str().unwrap(),
            "--save-state",
            "/nonexistent/out.json",
        ])
        .unwrap_err();
        assert!(err.to_string().contains("--save-state"), "{err}");
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let dir = dir("loose");
        let load = dir.join("in.json");
        std::fs::write(&load, r#"{"version":1}"#).unwrap();
        std::fs::set_permissions(&load, std::fs::Permissions::from_mode(0o640)).unwrap();
        let fixture = dir.join("page.cdp.json");
        std::fs::write(&fixture, ScriptBuilder::new().observe(&page()).to_json()).unwrap();
        let err = run(&[
            "--goal",
            "Click Go",
            "--policy",
            "instinct",
            "--fixture",
            fixture.to_str().unwrap(),
            "--load-state",
            load.to_str().unwrap(),
        ])
        .unwrap_err();
        assert!(err.to_string().contains("chmod 600"), "{err}");
    }
}
