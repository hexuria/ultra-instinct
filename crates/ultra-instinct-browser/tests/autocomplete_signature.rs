use serde_json::{json, Value};
use ultra_instinct_browser::script::{Control, PageSpec, ScriptBuilder};
use ultra_instinct_browser::{BrowserSession, ReplayTransport};
use ultra_instinct_core::RegionId;

fn city_page() -> PageSpec {
    PageSpec::of(
        &[Control::combobox(
            10,
            100,
            "City",
            (10.0, 10.0, 200.0, 28.0),
        )],
        "http://127.0.0.1/auto",
        "Auto",
    )
}

fn session(script: &str) -> BrowserSession<ReplayTransport> {
    BrowserSession::new(ReplayTransport::parse(script).unwrap())
}

fn append_step(script: String, step: Value) -> String {
    let mut value: Value = serde_json::from_str(&script).unwrap();
    value["calls"].as_array_mut().unwrap().push(step);
    serde_json::to_string(&value).unwrap()
}

fn city_id() -> RegionId {
    RegionId::try_new("n100").unwrap()
}

#[test]
fn autocomplete_signature_reads_a_string() {
    let script = ScriptBuilder::new()
        .observe(&city_page())
        .dom_read_autocomplete_signature(10, json!("o2:Manila\u{001f}Makati"))
        .to_json();
    let mut session = session(&script);
    session.observe().unwrap();

    assert_eq!(
        session.autocomplete_options_signature(&city_id()).unwrap(),
        Some("o2:Manila\u{001f}Makati".to_owned())
    );
}

#[test]
fn autocomplete_signature_ignores_non_string_values() {
    let script = ScriptBuilder::new()
        .observe(&city_page())
        .dom_read_autocomplete_signature(10, json!(7))
        .to_json();
    let mut session = session(&script);
    session.observe().unwrap();

    assert_eq!(
        session.autocomplete_options_signature(&city_id()).unwrap(),
        None
    );
}

#[test]
fn autocomplete_signature_ignores_thrown_functions() {
    let script = ScriptBuilder::new().observe(&city_page()).to_json();
    let script = append_step(
        script,
        json!({
            "method": "DOM.resolveNode",
            "result": {"object": {"type": "object", "objectId": "obj-10"}}
        }),
    );
    let script = append_step(
        script,
        json!({
            "method": "Runtime.callFunctionOn",
            "result": {
                "result": {"type": "object", "subtype": "error"},
                "exceptionDetails": {
                    "text": "Uncaught",
                    "exception": {"description": "Error: unavailable"}
                }
            }
        }),
    );
    let mut session = session(&script);
    session.observe().unwrap();

    assert_eq!(
        session.autocomplete_options_signature(&city_id()).unwrap(),
        None
    );
}

#[test]
fn autocomplete_signature_ignores_resolve_protocol_errors() {
    let script = append_step(
        ScriptBuilder::new().observe(&city_page()).to_json(),
        json!({
            "method": "DOM.resolveNode",
            "error": "execution context was destroyed"
        }),
    );
    let mut session = session(&script);
    session.observe().unwrap();

    assert_eq!(
        session.autocomplete_options_signature(&city_id()).unwrap(),
        None
    );
}

#[test]
fn autocomplete_signature_ignores_call_protocol_errors() {
    let script = ScriptBuilder::new().observe(&city_page()).to_json();
    let script = append_step(
        script,
        json!({
            "method": "DOM.resolveNode",
            "result": {"object": {"type": "object", "objectId": "obj-10"}}
        }),
    );
    let script = append_step(
        script,
        json!({
            "method": "Runtime.callFunctionOn",
            "error": "execution context was destroyed"
        }),
    );
    let mut session = session(&script);
    session.observe().unwrap();

    assert_eq!(
        session.autocomplete_options_signature(&city_id()).unwrap(),
        None
    );
}

#[test]
fn autocomplete_signature_returns_none_for_an_unknown_region() {
    let script = ScriptBuilder::new().observe(&city_page()).to_json();
    let mut session = session(&script);
    session.observe().unwrap();

    let unknown = RegionId::try_new("unknown").unwrap();
    assert_eq!(
        session.autocomplete_options_signature(&unknown).unwrap(),
        None
    );
}
