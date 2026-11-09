//! Label folding (ADR 0014): a label and the control it names are one
//! region, observed through the real session over a compact replay.

use serde_json::json;
use ultra_instinct_browser::script::{AxSpec, DomSpec, PageSpec, ScriptBuilder};
use ultra_instinct_browser::{BrowserSession, ReplayTransport};
use ultra_instinct_core::{Action, ActionKind, ActionSpace, InteractionManifold, Role};

fn observe(pages: &[PageSpec]) -> Vec<InteractionManifold> {
    let script = pages.iter().fold(ScriptBuilder::new(), |script, page| {
        script.observe_compact(page)
    });
    let mut session = BrowserSession::new(ReplayTransport::parse(&script.to_json()).unwrap());
    pages
        .iter()
        .map(|_| session.observe().unwrap().clone())
        .collect()
}

fn ids(manifold: &InteractionManifold) -> Vec<&str> {
    manifold.ids().map(|id| id.as_str()).collect()
}

#[test]
fn each_label_form_folds_into_the_control_it_names() {
    let manifold = observe(&[PageSpec::labelled_form(false)]).remove(0);
    // Gone: `for` labels (100, 102), the wrapping label (104), the label
    // holding a link (106), the aria-labelledby text (109), and the native
    // input with no box inside its styled label (112).
    assert_eq!(
        ids(&manifold),
        ["n101", "n103", "n105", "n107", "n108", "n110", "n111", "n113", "n114", "n115", "n116"]
    );
    let field = |id: &str| manifold.get_str(id).unwrap();
    assert_eq!(
        (field("n101").role(), field("n101").label()),
        (Role::TextField, "Email")
    );
    assert_eq!(field("n110").label(), "Nickname");
    // The link inside a folded label stays its own target.
    assert_eq!(field("n107").role(), Role::Link);
    assert_eq!(field("n108").label(), "I agree to the terms");
    // A label for a disabled field and a label for nothing stay as they were.
    assert_eq!(field("n113").role(), Role::Generic);
    assert!(field("n114").flags().disabled());
    assert_eq!(field("n115").label(), "Orphan label");
}

#[test]
fn a_custom_checkbox_is_its_label_box_with_the_checkbox_role_and_state() {
    let [before, after] = observe(&[
        PageSpec::labelled_form(false),
        PageSpec::labelled_form(true),
    ])
    .try_into()
    .unwrap();
    let news = before.get_str("n111").unwrap();
    assert_eq!(
        (news.role(), news.label()),
        (Role::Checkbox, "Send me news")
    );
    assert_eq!(
        news.rect().width(),
        120.0,
        "the label's box is the click target"
    );
    assert!(news.actions().contains(&Action::Toggle));
    assert_eq!(news.state().checked, Some(false));
    assert_eq!(after.get_str("n111").unwrap().state().checked, Some(true));
    // One CLICK each for the custom checkbox and the plain one, none for a label.
    let space = ActionSpace::from_manifold(&before);
    let clicks: Vec<&str> = space
        .targets_of(ActionKind::Click)
        .map(|a| a.target().unwrap().as_str())
        .collect();
    assert_eq!(
        clicks,
        ["n101", "n103", "n105", "n107", "n108", "n110", "n111", "n116"]
    );
    // Region identity is stable across observations.
    assert_eq!(ids(&before), ids(&after));
}

#[test]
fn a_transparent_or_covered_checkbox_folds_into_its_label() {
    // opacity: 0 on the plain wrapped checkbox, then the label painted over it.
    let transparent = PageSpec::labelled_form(false).style(55, vec![("opacity", "0")]);
    let covered = PageSpec::labelled_form(false).cover(105, 104);
    for page in [transparent, covered] {
        let manifold = observe(&[page]).remove(0);
        assert!(manifold.get_str("n105").is_none(), "{:?}", ids(&manifold));
        let remember = manifold.get_str("n104").unwrap();
        assert_eq!(
            (remember.role(), remember.label()),
            (Role::Checkbox, "Remember me")
        );
        assert_eq!(remember.rect().width(), 120.0);
    }
}

#[test]
fn a_covered_text_field_keeps_its_label() {
    // Only a checkbox or radio folds the other way; a covered text field
    // keeps its label and is marked occluded on its own.
    let manifold = observe(&[PageSpec::labelled_form(false).cover(101, 116)]).remove(0);
    assert!(manifold.get_str("n100").is_some());
    assert!(manifold.get_str("n101").is_some());
}

fn el(node: i64, tag: &'static str, text: &str, rect: (f64, f64, f64, f64)) -> DomSpec {
    DomSpec::button(node, node + 100, text, rect).with_tag(tag)
}

fn input(node: i64, kind: &str, rect: (f64, f64, f64, f64)) -> DomSpec {
    el(node, "INPUT", "", rect).with_attr("type", kind)
}

#[test]
fn a_pointer_cursor_text_named_by_aria_labelledby_keeps_its_click() {
    // The span's CLICK comes from its own pointer cursor (compact `ck`),
    // which observe adds after extract linked it.
    let dom = vec![
        el(10, "SPAN", "Nickname", (20.0, 20.0, 60.0, 20.0))
            .with_attr("id", "nm")
            .with_compact_state(json!({"ck": 1})),
        input(11, "checkbox", (90.0, 24.0, 13.0, 13.0)).with_attr("aria-labelledby", "nm"),
    ];
    let ax = vec![AxSpec::new(
        111,
        "checkbox",
        "Nickname",
        (90.0, 24.0, 13.0, 13.0),
    )];
    let manifold = observe(&[PageSpec::new(dom, ax, "http://127.0.0.1/", "T")]).remove(0);
    assert_eq!(ids(&manifold), ["n110", "n111"]);
    assert!(manifold
        .get_str("n110")
        .unwrap()
        .actions()
        .contains(&Action::Click));
}

#[test]
fn a_transparent_ancestor_is_not_a_visible_control() {
    // Label and field inside one `opacity: 0` box: neither is visible, so
    // nothing folds either way.
    let hidden_box = |checkbox: bool| {
        let kind = if checkbox { "checkbox" } else { "text" };
        let dom = vec![
            el(10, "DIV", "", (0.0, 0.0, 400.0, 60.0)).with_children(vec![
                el(11, "LABEL", "Agree", (20.0, 20.0, 60.0, 20.0)).with_attr("for", "x"),
                input(12, kind, (90.0, 24.0, 13.0, 13.0)).with_attr("id", "x"),
            ]),
        ];
        let role = if checkbox { "checkbox" } else { "textbox" };
        let ax = vec![AxSpec::new(112, role, "Agree", (90.0, 24.0, 13.0, 13.0))];
        PageSpec::new(dom, ax, "http://127.0.0.1/", "T").style(10, vec![("opacity", "0")])
    };
    for checkbox in [false, true] {
        let manifold = observe(&[hidden_box(checkbox)]).remove(0);
        assert_eq!(ids(&manifold), ["n111", "n112"], "checkbox {checkbox}");
        assert_eq!(manifold.get_str("n111").unwrap().role(), Role::Generic);
    }
    // A transparent box between the label and its checkbox (inside the
    // label): the label is what a reader sees, so it stands for the checkbox.
    let dom = vec![
        el(11, "LABEL", "Agree", (20.0, 20.0, 120.0, 20.0)).with_children(vec![el(
            13,
            "SPAN",
            "",
            (22.0, 22.0, 16.0, 16.0),
        )
        .with_children(vec![input(12, "checkbox", (22.0, 24.0, 13.0, 13.0))])]),
    ];
    let ax = vec![AxSpec::new(
        112,
        "checkbox",
        "Agree",
        (22.0, 24.0, 13.0, 13.0),
    )];
    let page = PageSpec::new(dom, ax, "http://127.0.0.1/", "T").style(13, vec![("opacity", "0")]);
    let manifold = observe(&[page]).remove(0);
    assert_eq!(ids(&manifold), ["n111"]);
    assert_eq!(manifold.get_str("n111").unwrap().role(), Role::Checkbox);
}

#[test]
fn a_checkbox_whose_evidence_flips_keeps_its_id() {
    // opacity: 0 at rest, 1 on focus: the folded region is the input's box
    // one time and the label's the next. Same role and name, nearby: the
    // IdentityMap carries the id across the flip in both directions.
    let at_rest = PageSpec::labelled_form(false).style(55, vec![("opacity", "0")]);
    let focused = PageSpec::labelled_form(false);
    let seen = observe(&[focused.clone(), at_rest, focused]);
    for manifold in &seen {
        let named: Vec<_> = manifold
            .regions()
            .filter(|r| r.label() == "Remember me")
            .map(|r| (r.id().as_str(), r.role()))
            .collect();
        assert_eq!(named, [("n105", Role::Checkbox)]);
    }
    assert_eq!(seen[0].get_str("n105").unwrap().rect().width(), 13.0);
    assert_eq!(seen[1].get_str("n105").unwrap().rect().width(), 120.0);
}
