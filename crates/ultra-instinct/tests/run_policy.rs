//! `run --confirm-press` / `--confirm` over a CDP replay: a matching press
//! ends BLOCKED with the rerun hint; confirming the label presses it.

fn fixture(name: &str) -> String {
    std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../fixtures")
        .join(name)
        .to_str()
        .unwrap()
        .to_owned()
}

fn run(extra: &[&str]) -> Result<String, ultra_instinct::CliError> {
    let mut args: Vec<String> = [
        "run",
        "--goal",
        "Click Go",
        "--policy",
        "instinct",
        "--fixture",
    ]
    .iter()
    .map(|s| (*s).to_owned())
    .collect();
    args.push(fixture("agent-click-go.cdp.json"));
    args.extend(extra.iter().map(|s| (*s).to_owned()));
    ultra_instinct::execute(&args)
}

#[test]
fn confirm_press_blocks_with_a_rerun_hint() {
    let out = run(&["--confirm-press", "g*"]).unwrap_or_else(|e| panic!("{e}"));
    assert!(
        out.contains("outcome blocked: needs-confirm: \"Go\""),
        "{out}"
    );
    assert!(out.contains("rerun with --confirm \"Go\"\n"), "{out}");
    assert!(!out.contains("step 1"), "nothing pressed: {out}");
}

#[test]
fn confirm_lets_that_press_run() {
    let out = run(&["--confirm-press", "g*", "--confirm", "Go"]).unwrap_or_else(|e| panic!("{e}"));
    assert!(out.contains("step 1"), "{out}");
    assert!(!out.contains("needs-confirm"), "{out}");
}
