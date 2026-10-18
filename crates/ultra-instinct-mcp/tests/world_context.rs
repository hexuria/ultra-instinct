//! Observe -> guard through the MCP server against scripted Chrome pages.
//!
//! Proves the extraction wiring, not only the guard math: `role="dialog"` +
//! `aria-modal="true"` (or the accessibility `modal` property) become a
//! modal front layer; the DOM tree becomes parent links; the accessibility
//! `focused` property becomes `near: "focus"`. No page is clicked.

use std::cell::RefCell;
use std::rc::Rc;

use serde_json::{json, Value};
use ultra_instinct_browser::script::{AxSpec, DomSpec, PageSpec, ScriptBuilder};
use ultra_instinct_browser::{CdpError, CdpTransport, ReplayTransport};
use ultra_instinct_mcp::Server;

const TAB: &str = "ws://mock/world";

struct Logged {
    inner: ReplayTransport,
    log: Rc<RefCell<Vec<String>>>,
}

impl CdpTransport for Logged {
    fn call(&mut self, method: &str, params_json: &str) -> Result<String, CdpError> {
        self.log.borrow_mut().push(method.to_owned());
        self.inner.call(method, params_json)
    }
}

fn server(script: ScriptBuilder) -> (Server, Rc<RefCell<Vec<String>>>) {
    let log = Rc::new(RefCell::new(Vec::new()));
    let shared = Rc::clone(&log);
    let mut script = Some(script);
    let server = Server::with_connector(move |_url| {
        let script = script.take().ok_or_else(|| CdpError::Transport {
            message: "one connect per test".into(),
        })?;
        Ok(Box::new(Logged {
            inner: ReplayTransport::parse(&script.to_json())?,
            log: Rc::clone(&shared),
        }) as Box<dyn CdpTransport>)
    });
    (server, log)
}

fn call(server: &mut Server, tool: &str, arguments: Value) -> Value {
    server
        .call_tool(tool, &arguments)
        .unwrap_or_else(|err| panic!("{tool} {arguments}: {err:?}"))
}

fn presses(log: &Rc<RefCell<Vec<String>>>) -> usize {
    log.borrow()
        .iter()
        .filter(|method| *method == "DOM.resolveNode" || *method == "Input.dispatchMouseEvent")
        .count()
}

fn region<'a>(observed: &'a Value, id: &str) -> &'a Value {
    observed["regions"]
        .as_array()
        .unwrap()
        .iter()
        .find(|region| region["id"] == id)
        .unwrap_or_else(|| panic!("no {id} in {observed}"))
}

/// Project settings. With `dialog`, a confirm dialog is open over it.
/// `dom_modal` puts `aria-modal="true"` on the dialog element; `ax_modal`
/// sets the accessibility `modal` property (what `showModal()` gives).
fn settings_page(dialog: bool, dom_modal: bool, ax_modal: bool) -> PageSpec {
    let mut dom = vec![
        DomSpec::button(10, 100, "Delete project", (1200.0, 780.0, 160.0, 36.0)),
        DomSpec::button(11, 110, "Cancel", (1060.0, 780.0, 100.0, 36.0)),
    ];
    let mut ax = vec![
        AxSpec::new(
            100,
            "button",
            "Delete project",
            (1200.0, 780.0, 160.0, 36.0),
        ),
        AxSpec::new(110, "button", "Cancel", (1060.0, 780.0, 100.0, 36.0)),
    ];
    if dialog {
        let mut container = DomSpec::container(
            20,
            200,
            "dialog",
            "Delete project?",
            (520.0, 300.0, 400.0, 240.0),
        )
        .with_children(vec![
            DomSpec::button(21, 210, "Cancel", (560.0, 480.0, 100.0, 36.0)),
            DomSpec::button(22, 220, "Delete", (780.0, 480.0, 100.0, 36.0)),
        ]);
        if dom_modal {
            container = container.with_attr("aria-modal", "true");
        }
        dom.push(container);
        let node = AxSpec::new(
            200,
            "dialog",
            "Delete project?",
            (520.0, 300.0, 400.0, 240.0),
        );
        ax.push(if ax_modal { node.modal() } else { node });
        ax.push(AxSpec::new(
            210,
            "button",
            "Cancel",
            (560.0, 480.0, 100.0, 36.0),
        ));
        ax.push(AxSpec::new(
            220,
            "button",
            "Delete",
            (780.0, 480.0, 100.0, 36.0),
        ));
    }
    let mut page = PageSpec::new(dom, ax, "http://127.0.0.1/settings", "Project settings");
    page.width = 1440.0;
    page.height = 900.0;
    page
}

#[test]
fn observe_reports_the_modal_front_layer_parents_and_buried_regions() {
    let page = settings_page(true, true, false);
    let (mut server, log) = server(ScriptBuilder::new().observe(&page));
    let observed = call(&mut server, "observe", json!({"cdp": TAB}));
    assert_eq!(
        observed["front_layer"],
        json!([{"id": "n200", "label": "Delete project?", "modal": true}]),
        "{observed}"
    );
    assert_eq!(region(&observed, "n200")["role"], "dialog");
    assert_eq!(region(&observed, "n210")["parent"], "n200");
    assert_eq!(region(&observed, "n100")["parent"], Value::Null);
    assert_eq!(region(&observed, "n100")["state"]["visibility"], "occluded");
    assert_eq!(region(&observed, "n110")["state"]["visibility"], "occluded");
    assert_eq!(region(&observed, "n210")["state"]["visibility"], "visible");
    assert_eq!(presses(&log), 0);
}

#[test]
fn guard_refuses_a_click_behind_the_modal_and_allows_the_dialog_twin() {
    for (dom_modal, ax_modal) in [(true, false), (false, true)] {
        let page = settings_page(true, dom_modal, ax_modal);
        let script = ScriptBuilder::new()
            .observe(&page)
            .observe(&page)
            .observe(&page);
        let (mut server, log) = server(script);
        let buried = call(
            &mut server,
            "guard",
            json!({"cdp": TAB, "target": "Delete project", "role": "button"}),
        );
        assert_eq!(buried["decision"], "refuse", "{buried}");
        assert_eq!(buried["reason"], "front-layer", "{buried}");
        assert_eq!(buried["executed"], false);

        let proposed = call(
            &mut server,
            "guard",
            json!({"cdp": TAB, "target": "Cancel", "role": "button", "proposed": "n110"}),
        );
        assert_eq!(proposed["reason"], "front-layer", "{proposed}");

        let twin = call(
            &mut server,
            "guard",
            json!({"cdp": TAB, "target": "Cancel", "role": "button"}),
        );
        assert_eq!(twin["decision"], "allow", "{twin}");
        assert_eq!(twin["target"]["id"], "n210", "{twin}");
        assert_eq!(presses(&log), 0);
    }
}

#[test]
fn guard_escalates_when_a_dialog_opened_after_the_host_observed() {
    let closed = settings_page(false, false, false);
    let open = settings_page(true, true, false);
    let script = ScriptBuilder::new().observe(&closed).observe(&open);
    let (mut server, log) = server(script);
    let seen = call(&mut server, "observe", json!({"cdp": TAB}));
    assert_eq!(seen["front_layer"], json!([]));
    let decision = call(
        &mut server,
        "guard",
        json!({
            "cdp": TAB,
            "target": "Delete project",
            "role": "button",
            "proposed": "n100",
            "seen_snapshot": seen["snapshot"],
        }),
    );
    assert_eq!(decision["decision"], "escalate", "{decision}");
    assert_eq!(decision["reason"], "world-changed", "{decision}");
    assert_eq!(presses(&log), 0);
}

/// Two rows, each a `role="row"` container with a Hostname field and a
/// Suspend button. `focus_beta` puts accessibility focus on beta's field.
fn rows_page(focus_beta: bool) -> PageSpec {
    let row = |node: i64, label: &str, y: f64| {
        DomSpec::container(node, node * 10, "row", label, (40.0, y, 1200.0, 48.0)).with_children(
            vec![
                DomSpec::button(
                    node + 1,
                    (node + 1) * 10,
                    "Hostname",
                    (60.0, y + 6.0, 300.0, 36.0),
                )
                .with_tag("INPUT"),
                DomSpec::button(
                    node + 2,
                    (node + 2) * 10,
                    "Suspend",
                    (1100.0, y + 6.0, 100.0, 36.0),
                ),
            ],
        )
    };
    let dom = vec![row(30, "alpha", 140.0), row(40, "beta", 200.0)];
    let beta_host = AxSpec::new(410, "textbox", "Hostname", (60.0, 206.0, 300.0, 36.0));
    let ax = vec![
        AxSpec::new(310, "textbox", "Hostname", (60.0, 146.0, 300.0, 36.0)),
        AxSpec::new(320, "button", "Suspend", (1100.0, 146.0, 100.0, 36.0)),
        if focus_beta {
            beta_host.focused()
        } else {
            beta_host
        },
        AxSpec::new(420, "button", "Suspend", (1100.0, 206.0, 100.0, 36.0)),
    ];
    PageSpec::new(dom, ax, "http://127.0.0.1/servers", "Servers")
}

#[test]
fn near_focus_and_within_resolve_twin_suspend_buttons() {
    let page = rows_page(true);
    let script = ScriptBuilder::new()
        .observe(&page)
        .observe(&page)
        .observe(&page)
        .observe(&page);
    let (mut server, log) = server(script);
    let observed = call(&mut server, "observe", json!({"cdp": TAB}));
    assert_eq!(observed["focused"], "n410", "{observed}");
    assert_eq!(region(&observed, "n420")["parent"], "n400");

    let bare = call(
        &mut server,
        "guard",
        json!({"cdp": TAB, "target": "Suspend", "role": "button"}),
    );
    assert_eq!(bare["decision"], "escalate", "{bare}");
    assert_eq!(bare["reason"], "ambiguous");

    let focused = call(
        &mut server,
        "guard",
        json!({"cdp": TAB, "target": "Suspend", "role": "button", "near": "focus"}),
    );
    assert_eq!(focused["decision"], "allow", "{focused}");
    assert_eq!(focused["target"]["id"], "n420");
    assert_eq!(
        focused["scope"],
        json!({"within": null, "near": "n410", "near_scope": "n400"})
    );

    let within = call(
        &mut server,
        "guard",
        json!({"cdp": TAB, "target": "Suspend", "role": "button", "within": "n300"}),
    );
    assert_eq!(within["decision"], "allow", "{within}");
    assert_eq!(within["target"]["id"], "n320");
    assert_eq!(presses(&log), 0);
}

#[test]
fn near_focus_without_a_focused_region_is_the_default_ranking() {
    let page = rows_page(false);
    let (mut server, _log) = server(ScriptBuilder::new().observe(&page));
    let decision = call(
        &mut server,
        "guard",
        json!({"cdp": TAB, "target": "Suspend", "role": "button", "near": "focus"}),
    );
    assert_eq!(decision["focused"], Value::Null);
    assert_eq!(decision["reason"], "ambiguous", "{decision}");
    assert_eq!(decision["scope"]["near"], Value::Null);
}

/// A custom backdrop `div` (not `role=dialog`) covers the page Save button.
/// Dialog front-layer logic would allow the click; hit-test must refuse.
#[test]
fn hit_test_refuses_a_click_under_a_custom_backdrop() {
    let backdrop = DomSpec::container(
        50,
        500,
        "generic",
        "Cookie consent",
        (0.0, 0.0, 1440.0, 900.0),
    );
    let save = DomSpec::button(10, 100, "Save", (1200.0, 780.0, 100.0, 36.0));
    let accept = DomSpec::button(51, 510, "Accept all", (1200.0, 40.0, 120.0, 36.0));
    // No AX node for the backdrop: Chrome's AX tree skips generic containers
    // (`ax_role` returns None), and ScriptBuilder must not emit a matching
    // getBoxModel for a node observe will never request.
    let mut page = PageSpec::new(
        vec![save, backdrop.with_children(vec![accept])],
        vec![
            AxSpec::new(100, "button", "Save", (1200.0, 780.0, 100.0, 36.0)),
            AxSpec::new(510, "button", "Accept all", (1200.0, 40.0, 120.0, 36.0)),
        ],
        "http://127.0.0.1/docs",
        "Docs",
    );
    page.width = 1440.0;
    page.height = 900.0;
    // Without cover(), hit-test would say Save owns its center and the guard
    // would allow. The backdrop's backend under Save's center is the proof.
    page = page.cover(100, 500);
    // observe + two guards each re-observe.
    let script = ScriptBuilder::new()
        .observe(&page)
        .observe(&page)
        .observe(&page);
    let (mut server, log) = server(script);
    let observed = call(&mut server, "observe", json!({"cdp": TAB}));
    assert_eq!(
        region(&observed, "n100")["state"]["visibility"],
        "occluded",
        "{observed}"
    );
    assert_eq!(
        region(&observed, "n510")["state"]["visibility"],
        "visible",
        "{observed}"
    );
    // Label-only would allow Save; hit-test occlusion refuses.
    let buried = call(
        &mut server,
        "guard",
        json!({"cdp": TAB, "target": "Save", "role": "button"}),
    );
    assert_eq!(buried["decision"], "refuse", "{buried}");
    assert_eq!(buried["reason"], "occluded", "{buried}");
    let accept_click = call(
        &mut server,
        "guard",
        json!({"cdp": TAB, "target": "Accept all", "role": "button"}),
    );
    assert_eq!(accept_click["decision"], "allow", "{accept_click}");
    assert_eq!(presses(&log), 0);
}

/// A fixed high-z toast covers Save. Hit-test is left clear (no cover()), so
/// only the stacking map can mark Save occluded.
#[test]
fn stacking_map_refuses_under_high_z_toast_without_hit_override() {
    let save = DomSpec::button(10, 100, "Save", (1200.0, 780.0, 100.0, 36.0));
    let toast = DomSpec::button(20, 200, "Dismiss", (1180.0, 760.0, 160.0, 80.0));
    let mut page = PageSpec::new(
        vec![save, toast],
        vec![
            AxSpec::new(100, "button", "Save", (1200.0, 780.0, 100.0, 36.0)),
            AxSpec::new(200, "button", "Dismiss", (1180.0, 760.0, 160.0, 80.0)),
        ],
        "http://127.0.0.1/docs",
        "Docs",
    );
    page.width = 1440.0;
    page.height = 900.0;
    page = page.style(
        20,
        vec![
            ("z-index", "9999"),
            ("position", "fixed"),
            ("opacity", "1"),
            ("transform", "none"),
            ("filter", "none"),
            ("isolation", "auto"),
            ("mix-blend-mode", "normal"),
            ("will-change", "auto"),
            ("pointer-events", "auto"),
        ],
    );
    let script = ScriptBuilder::new()
        .observe(&page)
        .observe(&page)
        .observe(&page);
    let (mut server, log) = server(script);
    let observed = call(&mut server, "observe", json!({"cdp": TAB}));
    assert_eq!(
        region(&observed, "n100")["state"]["visibility"],
        "occluded",
        "{observed}"
    );
    assert_eq!(
        region(&observed, "n200")["state"]["visibility"],
        "visible",
        "{observed}"
    );
    let buried = call(
        &mut server,
        "guard",
        json!({"cdp": TAB, "target": "Save", "role": "button"}),
    );
    assert_eq!(buried["decision"], "refuse", "{buried}");
    assert_eq!(buried["reason"], "occluded", "{buried}");
    let dismiss = call(
        &mut server,
        "guard",
        json!({"cdp": TAB, "target": "Dismiss", "role": "button"}),
    );
    assert_eq!(dismiss["decision"], "allow", "{dismiss}");
    assert_eq!(presses(&log), 0);
}

#[test]
fn seen_snapshot_escalates_when_occlusion_appears() {
    let save = DomSpec::button(10, 100, "Save", (1200.0, 780.0, 100.0, 36.0));
    let accept = DomSpec::button(20, 200, "Accept all", (1200.0, 40.0, 120.0, 36.0));
    let clear = {
        let mut page = PageSpec::new(
            vec![save.clone(), accept.clone()],
            vec![
                AxSpec::new(100, "button", "Save", (1200.0, 780.0, 100.0, 36.0)),
                AxSpec::new(200, "button", "Accept all", (1200.0, 40.0, 120.0, 36.0)),
            ],
            "http://127.0.0.1/docs",
            "Docs",
        );
        page.width = 1440.0;
        page.height = 900.0;
        page
    };
    let covered = clear.clone().cover(100, 200);
    // observe clear, then re-observe covered twice for two guards... actually:
    // flow: observe clear -> snapshot; observe covered -> guard with seen.
    let script = ScriptBuilder::new()
        .observe(&clear)
        .observe(&covered)
        .observe(&covered);
    let (mut server, log) = server(script);
    let first = call(&mut server, "observe", json!({"cdp": TAB}));
    let seen = first["snapshot"].clone();
    assert_eq!(region(&first, "n100")["state"]["visibility"], "visible");
    let decision = call(
        &mut server,
        "guard",
        json!({
            "cdp": TAB,
            "target": "Accept all",
            "role": "button",
            "seen_snapshot": seen,
        }),
    );
    assert_eq!(decision["decision"], "escalate", "{decision}");
    assert_eq!(decision["reason"], "world-changed", "{decision}");
    assert_eq!(presses(&log), 0);
}
