use ultra_instinct_browser::{
    script::{AxSpec, Control, DomSpec, PageSpec, ScriptBuilder},
    BrowserSession, ReplayTransport,
};
use ultra_instinct_core::Role;

fn session(script: ScriptBuilder) -> BrowserSession<ReplayTransport> {
    BrowserSession::new(ReplayTransport::parse(&script.to_json()).unwrap())
}

#[test]
fn observe_keeps_button_inside_open_shadow_root() {
    let host = DomSpec::button(5, 50, "", (0.0, 0.0, 400.0, 300.0))
        .with_tag("DIV")
        .with_shadow_roots(vec![DomSpec::shadow_root(
            6,
            60,
            vec![DomSpec::button(
                10,
                100,
                "Shadow Save",
                (40.0, 40.0, 100.0, 28.0),
            )],
        )]);
    // Empty label + DIV without role is not kept; only the shadow button is.
    let host = DomSpec {
        label: String::new(),
        attributes: Vec::new(),
        ..host
    };
    let page = PageSpec::new(
        vec![host],
        vec![AxSpec::new(
            100,
            "button",
            "Shadow Save",
            (40.0, 40.0, 100.0, 28.0),
        )],
        "http://127.0.0.1/shadow",
        "Shadow",
    );
    let mut s = session(ScriptBuilder::new().observe(&page));
    let m = s.observe().unwrap();
    let region = m.get_str("n100").expect("shadow button");
    assert_eq!(region.label(), "Shadow Save");
    assert_eq!(region.role(), Role::Button);
}

#[test]
fn observe_keeps_button_inside_same_origin_iframe() {
    let frame = DomSpec::button(5, 50, "", (0.0, 0.0, 400.0, 300.0))
        .with_tag("IFRAME")
        .with_content_document(DomSpec::document(
            6,
            60,
            vec![DomSpec::button(
                20,
                200,
                "Frame Confirm",
                (20.0, 20.0, 120.0, 28.0),
            )],
        ));
    let frame = DomSpec {
        label: String::new(),
        attributes: Vec::new(),
        ..frame
    };
    let page = PageSpec::new(
        vec![frame],
        vec![AxSpec::new(
            200,
            "button",
            "Frame Confirm",
            (20.0, 20.0, 120.0, 28.0),
        )],
        "http://127.0.0.1/frame",
        "Frame",
    );
    let mut s = session(ScriptBuilder::new().observe(&page));
    let m = s.observe().unwrap();
    assert_eq!(m.get_str("n200").unwrap().label(), "Frame Confirm");
}

#[test]
fn observe_offers_type_on_combobox_and_click_on_option() {
    let page = PageSpec::of(
        &[
            Control::combobox(10, 100, "City", (10.0, 10.0, 200.0, 28.0)),
            Control::option(11, 110, "Manila", (10.0, 40.0, 200.0, 28.0)),
        ],
        "http://127.0.0.1/auto",
        "Auto",
    );
    let mut s = session(ScriptBuilder::new().observe(&page));
    let m = s.observe().unwrap();
    let city = m.get_str("n100").unwrap();
    assert_eq!(city.role(), Role::ComboBox);
    assert!(city.actions().contains(&ultra_instinct_core::Action::Type));
    let opt = m.get_str("n110").unwrap();
    assert_eq!(opt.role(), Role::Option);
    assert!(opt.actions().contains(&ultra_instinct_core::Action::Click));
}
