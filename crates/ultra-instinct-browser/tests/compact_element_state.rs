use serde_json::json;
use ultra_instinct_browser::script::{DomSpec, PageSpec, ScriptBuilder};
use ultra_instinct_browser::{BrowserSession, ReplayTransport};
use ultra_instinct_core::ElementState;

#[test]
fn compact_observe_attaches_input_and_select_state_to_regions() {
    let input = DomSpec::button(10, 100, "", (20.0, 20.0, 120.0, 28.0))
        .with_tag("INPUT")
        .with_attr("aria-label", "Name")
        .with_compact_state(json!({"v":"Ana"}));
    let select = DomSpec::button(20, 200, "", (20.0, 60.0, 160.0, 28.0))
        .with_tag("SELECT")
        .with_attr("aria-label", "Time zone")
        .with_compact_state(json!({
            "sel":"UTC",
            "o":["UTC", "Asia/Manila"]
        }));
    let page = PageSpec::new(
        vec![input, select],
        Vec::new(),
        "https://example.test/form",
        "Form",
    );
    let script = ScriptBuilder::new().observe_compact(&page).to_json();
    let mut session = BrowserSession::new(ReplayTransport::parse(&script).unwrap());

    let manifold = session.observe().unwrap();
    assert_eq!(
        manifold.get_str("n100").unwrap().state(),
        &ElementState {
            value: Some("Ana".to_owned()),
            ..ElementState::default()
        }
    );
    assert_eq!(
        manifold.get_str("n200").unwrap().state(),
        &ElementState {
            selected: Some("UTC".to_owned()),
            options: vec!["UTC".to_owned(), "Asia/Manila".to_owned()],
            ..ElementState::default()
        }
    );
}
