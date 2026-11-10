use serde_json::json;
use ultra_instinct_browser::script::{AxSpec, DomSpec, PageSpec, ScriptBuilder};
use ultra_instinct_browser::{BrowserSession, ReplayTransport};
use ultra_instinct_core::{ElementState, FormSubmit, Role};

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
            form: Some(FormSubmit::NotInForm),
            ..ElementState::default()
        }
    );
    assert_eq!(
        manifold.get_str("n200").unwrap().state(),
        &ElementState {
            selected: Some("UTC".to_owned()),
            options: vec!["UTC".to_owned(), "Asia/Manila".to_owned()],
            form: Some(FormSubmit::NotInForm),
            ..ElementState::default()
        }
    );
}

#[test]
fn a_zero_width_label_reads_plain_and_a_selected_tab_is_selected() {
    // Facebook's "S͏p͏o͏n͏s͏o͏r͏e͏d": U+034F between the letters, the AX name
    // plain. One region, labelled without the hidden characters.
    let sponsored = DomSpec::button(
        10,
        100,
        "S\u{34F}p\u{34F}o\u{34F}nsored",
        (20.0, 20.0, 120.0, 28.0),
    );
    let tab = DomSpec::button(20, 200, "Inbox", (20.0, 60.0, 80.0, 28.0))
        .with_tag("DIV")
        .with_attr("role", "tab")
        .with_attr("aria-selected", "true")
        .with_compact_state(json!({"as": true}));
    let other = DomSpec::button(30, 300, "Sent\u{200B}", (110.0, 60.0, 80.0, 28.0))
        .with_tag("DIV")
        .with_attr("role", "tab")
        .with_attr("aria-selected", "false")
        .with_attr("aria-current", "step")
        .with_compact_state(json!({"as": false, "ac": "step"}));
    let page = PageSpec::new(
        vec![sponsored, tab, other],
        vec![AxSpec::new(
            100,
            "button",
            "Sponsored",
            (20.0, 20.0, 120.0, 28.0),
        )],
        "https://example.test/feed",
        "Feed",
    );
    let script = ScriptBuilder::new().observe_compact(&page).to_json();
    let mut session = BrowserSession::new(ReplayTransport::parse(&script).unwrap());

    let manifold = session.observe().unwrap();
    let region = |id: &str| manifold.get_str(id).unwrap();
    assert_eq!(region("n100").label(), "Sponsored");
    assert_eq!(
        (region("n200").role(), region("n200").state().selected_item),
        (Role::Tab, Some(true))
    );
    assert_eq!(
        (region("n300").label(), region("n300").state().selected_item),
        ("Sent", Some(false))
    );
    assert_eq!(region("n300").state().aria_current.as_deref(), Some("step"));
    assert_eq!(region("n200").state().aria_current, None);
    assert_eq!(region("n100").state().selected_item, None);
}
