//! Acme Mail replica: the live drive's t7 and t8 against a scripted Chrome.
//!
//! The page is a [`ScriptBuilder`] replica of `examples/live-drive/site` at
//! 1280x800 with the "Q3 launch checklist" thread open, as the live drive saw
//! it: labeled controls that DOM and accessibility both report, plus the
//! accessibility-only nodes Chrome adds for that page, unnamed SVG icons
//! (`image`) and unnamed or named containers (`group`, `toolbar`, `article`,
//! `region`, `dialog`) that extract as `generic`. Rectangles follow the live
//! layout. It is not a capture and not a model of Chrome.
//!
//! In the live run, "Send" asked as a link tied 63 candidates at 0.50 and an
//! unnamed AX node won on region id; the twin Send refused as ambiguous and
//! the caller asked the same locate eight more times.

use std::cell::RefCell;
use std::rc::Rc;

use serde_json::{json, Value};
use ultra_instinct_browser::script::{AxSpec, Control, PageSpec, ScriptBuilder};
use ultra_instinct_browser::{CdpError, CdpTransport, ReplayTransport};
use ultra_instinct_mcp::Server;

const TAB: &str = "ws://mock/acme-mail";

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

/// One tab with one script, and the log of every CDP method it was asked.
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

fn presses(log: &Rc<RefCell<Vec<String>>>) -> usize {
    log.borrow()
        .iter()
        .filter(|method| *method == "DOM.resolveNode" || *method == "Input.dispatchMouseEvent")
        .count()
}

fn call(server: &mut Server, tool: &str, arguments: Value) -> Value {
    server
        .call_tool(tool, &arguments)
        .unwrap_or_else(|err| panic!("{tool} {arguments}: {err:?}"))
}

/// An unnamed SVG icon inside an icon button, as Chrome's AX tree has it.
fn icon(backend: i64, x: f64, y: f64) -> AxSpec {
    AxSpec::new(backend, "image", "", (x + 10.0, y + 10.0, 20.0, 20.0))
}

/// Reply Send in the quick-reply box. Region id `n714`, DOM node 71.
const REPLY_SEND: (i64, i64) = (71, 714);
/// Compose Send at the bottom of the docked sheet. Region id `n750`.
const COMPOSE_SEND: (i64, i64) = (75, 750);

/// The thread view, optionally with the Compose sheet open over it.
fn thread_page(compose_open: bool) -> PageSpec {
    let icon_button = |node, backend, label: &str, x, y| {
        Control::button(node, backend, label, (x, y, 40.0, 40.0))
    };
    let mut controls = vec![
        icon_button(1, 12, "Main menu", 16.0, 12.0),
        Control::button(3, 30, "Compose", (8.0, 72.0, 140.0, 56.0)),
        Control::link(4, 40, "Inbox", (0.0, 140.0, 256.0, 32.0)),
        Control::link(5, 50, "Starred", (0.0, 172.0, 256.0, 32.0)),
        Control::link(6, 60, "Sent", (0.0, 236.0, 256.0, 32.0)),
        Control::link(7, 70, "Drafts", (0.0, 268.0, 256.0, 32.0)),
        Control::link(8, 80, "Settings", (0.0, 300.0, 256.0, 32.0)),
        Control::link(2, 24, "Help", (1100.0, 12.0, 40.0, 40.0)),
        Control::link(9, 90, "Back to Inbox", (272.0, 72.0, 40.0, 40.0)),
        icon_button(68, 686, "Archive", -10000.0, 72.0),
        icon_button(69, 690, "Archive", 316.0, 72.0),
        icon_button(70, 694, "Delete", 360.0, 72.0),
        icon_button(72, 698, "Mark as unread", 404.0, 72.0),
        Control::text_field(73, 702, "Reply to Maya Reyes", (328.0, 330.0, 760.0, 56.0)),
        Control::button(
            REPLY_SEND.0,
            REPLY_SEND.1,
            "Send",
            (344.0, 390.0, 66.0, 36.0),
        ),
        icon_button(74, 716, "Formatting options", 420.0, 388.0),
        icon_button(76, 720, "Attach files", 464.0, 388.0),
    ];
    let mut ax_only = vec![
        icon(1012, 16.0, 12.0),
        icon(1013, 8.0, 80.0),
        icon(1014, 1100.0, 12.0),
        icon(1015, 272.0, 72.0),
        icon(1016, 316.0, 72.0),
        icon(1017, 360.0, 72.0),
        icon(1018, 404.0, 72.0),
        icon(1019, 420.0, 388.0),
        icon(1020, 464.0, 388.0),
        AxSpec::new(1021, "group", "", (256.0, 64.0, 1008.0, 720.0)),
        AxSpec::new(
            1022,
            "article",
            "Open message",
            (256.0, 64.0, 1008.0, 720.0),
        ),
        AxSpec::new(
            1023,
            "toolbar",
            "Message actions",
            (272.0, 64.0, 992.0, 48.0),
        ),
        AxSpec::new(1024, "region", "Quick reply", (328.0, 320.0, 760.0, 120.0)),
        AxSpec::new(1025, "group", "", (328.0, 384.0, 760.0, 48.0)),
    ];
    if compose_open {
        controls.extend([
            Control::button(80, 800, "Minimize", (1172.0, 248.0, 28.0, 28.0)),
            Control::button(81, 810, "Full screen", (1200.0, 248.0, 28.0, 28.0)),
            Control::button(82, 820, "Save and close", (1228.0, 248.0, 28.0, 28.0)),
            Control::text_field(83, 830, "To recipients", (740.0, 288.0, 440.0, 40.0)),
            Control::button(84, 840, "Add Cc", (1190.0, 292.0, 60.0, 30.0)),
            Control::text_field(85, 850, "Subject", (740.0, 328.0, 508.0, 40.0)),
            Control::text_field(86, 860, "Message Body", (740.0, 376.0, 508.0, 340.0)),
            Control::button(
                COMPOSE_SEND.0,
                COMPOSE_SEND.1,
                "Send",
                (740.0, 752.0, 80.0, 36.0),
            ),
            Control::button(87, 870, "Insert link", (880.0, 752.0, 36.0, 36.0)),
            Control::button(88, 880, "Discard draft", (1220.0, 752.0, 36.0, 36.0)),
        ]);
        ax_only.extend([
            AxSpec::new(1030, "dialog", "New Message", (724.0, 240.0, 540.0, 560.0)),
            icon(1031, 1172.0, 248.0),
            icon(1032, 1200.0, 248.0),
            icon(1033, 1228.0, 248.0),
            icon(1034, 880.0, 752.0),
            icon(1035, 1220.0, 752.0),
            AxSpec::new(1036, "group", "", (724.0, 744.0, 540.0, 52.0)),
        ]);
    }
    let mut page = PageSpec::of(
        &controls,
        "http://127.0.0.1:8765/index.html#thread/q3",
        "Q3 launch checklist - Acme Mail",
    );
    page.ax.extend(ax_only);
    page.width = 1280.0;
    page.height = 800.0;
    page
}

/// Compose is a non-modal, accessibility-only dialog docked at the right.
/// It is a front layer, but it only blocks what its box covers, and the
/// compose controls inside its box are its content. The quick-reply Send
/// stays clickable, so the twin Send is still ambiguous, not refused.
#[test]
fn guard_with_compose_open_blocks_nothing_outside_the_compose_box() {
    let page = thread_page(true);
    let (mut server, log) = server(ScriptBuilder::new().observe(&page).observe(&page));
    let observed = call(&mut server, "observe", json!({"cdp": TAB}));
    assert_eq!(
        observed["front_layer"],
        json!([{"id": "ax1030", "label": "New Message", "modal": false}]),
        "{observed}"
    );
    let visibility = |id: &str| {
        observed["regions"]
            .as_array()
            .unwrap()
            .iter()
            .find(|region| region["id"] == id)
            .map(|region| region["state"]["visibility"].clone())
            .unwrap()
    };
    assert_eq!(visibility("n714"), "visible");
    assert_eq!(visibility("n750"), "visible");
    let decision = call(
        &mut server,
        "guard",
        json!({"cdp": TAB, "target": "Send", "role": "button"}),
    );
    assert_eq!(decision["decision"], "escalate", "{decision}");
    assert_eq!(decision["reason"], "ambiguous", "{decision}");
    assert_eq!(presses(&log), 0);
}

/// t7: "Send" asked as a link. No link is named Send, so every candidate is a
/// miss. A named Send button (right label, wrong role) must rank above every
/// unnamed node, and the act must still refuse below the gate.
#[ignore = "actuation removed; re-home under guard"]
#[test]
fn t7_send_as_a_link_ranks_the_named_send_above_unnamed_nodes_and_does_not_click() {
    let page = thread_page(false);
    let (mut server, log) = server(ScriptBuilder::new().observe(&page).observe(&page));

    let observed = call(&mut server, "observe", json!({"cdp": TAB}));
    let unnamed = observed["regions"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|region| region["label"] == "")
        .count();
    assert!(
        unnamed >= 10,
        "replica must carry unnamed AX nodes: {observed}"
    );

    let located = call(
        &mut server,
        "locate",
        json!({"cdp": TAB, "text": "Send", "role": "link"}),
    );
    let candidates = located["candidates"].as_array().unwrap();
    assert_eq!(candidates[0]["id"], "n714", "{located}");
    assert_eq!(candidates[0]["label"], "Send");
    assert!((candidates[0]["confidence"].as_f64().unwrap() - 0.5).abs() < 1e-12);
    for row in candidates.iter().filter(|row| row["label"] == "") {
        assert_eq!(
            row["confidence"],
            ultra_instinct_resonance::TEXT_MISS_CAP,
            "{row}"
        );
    }
    assert!(candidates
        .iter()
        .all(|row| row["confidence"].as_f64().unwrap() < 0.55));

    let top = &candidates[0];
    let runner = &candidates[1];
    let acted = call(
        &mut server,
        "act",
        json!({
            "cdp": TAB,
            "region": top["id"],
            "confidence": top["confidence"],
            "runner_up": {"id": runner["id"], "confidence": runner["confidence"]},
            "expect_text": "Reply sent",
        }),
    );
    assert_eq!(acted["executed"], false, "{acted}");
    assert_eq!(acted["fallback"], "low-confidence", "{acted}");
    assert_eq!(presses(&log), 0);
}

/// t8: thread open and Compose open, so two visible "Send" buttons. The act
/// refuses as ambiguous; the second identical locate carries repeated_query
/// with a position that separates each Send; asking with the reply's
/// position resolves it and the press goes to the reply Send.
#[ignore = "actuation removed; re-home under guard"]
#[test]
fn t8_twin_send_refuses_then_repeated_query_names_a_separating_position() {
    let page = thread_page(true);
    let script = ScriptBuilder::new()
        .observe(&page) // locate 1
        .observe(&page) // locate 2 (identical)
        .observe(&page) // locate 3 with the suggested position
        .dom_click(REPLY_SEND.0) // act on the reply Send
        .observe(&page); // act observe_after (cdp default)
    let (mut server, log) = server(script);
    let ask = json!({"cdp": TAB, "text": "Send", "role": "button"});

    let first = call(&mut server, "locate", ask.clone());
    assert_eq!(first["signals"], json!([]));
    let candidates = first["candidates"].as_array().unwrap();
    let pair: Vec<(&str, f64)> = candidates[..2]
        .iter()
        .map(|row| {
            (
                row["id"].as_str().unwrap(),
                row["confidence"].as_f64().unwrap(),
            )
        })
        .collect();
    assert_eq!(pair, [("n714", 1.0), ("n750", 1.0)]);

    let refused = call(
        &mut server,
        "act",
        json!({
            "cdp": TAB,
            "region": "n714",
            "confidence": 1.0,
            "runner_up": {"id": "n750", "confidence": 1.0},
        }),
    );
    assert_eq!(refused["executed"], false, "{refused}");
    assert_eq!(refused["fallback"], "ambiguous", "{refused}");
    assert_eq!(presses(&log), 0);

    let second = call(&mut server, "locate", ask);
    assert_eq!(second["candidates"], first["candidates"]);
    assert_eq!(
        second["signals"],
        json!([{
            "kind": "repeated_query",
            "count": 2,
            "top": {"id": "n714", "suggested_position": "left"},
            "runner_up": {"id": "n750", "suggested_position": "bottom"},
        }])
    );

    let hinted = call(
        &mut server,
        "locate",
        json!({"cdp": TAB, "text": "Send", "role": "button", "position": "left"}),
    );
    assert_eq!(hinted["signals"], json!([]));
    let top = &hinted["candidates"][0];
    let runner = &hinted["candidates"][1];
    assert_eq!(top["id"], "n714");
    let margin = top["confidence"].as_f64().unwrap() - runner["confidence"].as_f64().unwrap();
    assert!(margin >= 0.05, "{hinted}");

    let acted = call(
        &mut server,
        "act",
        json!({
            "cdp": TAB,
            "region": "n714",
            "confidence": top["confidence"],
            "runner_up": {"id": runner["id"], "confidence": runner["confidence"]},
        }),
    );
    assert_eq!(acted["executed"], true, "{acted}");
    assert_eq!(presses(&log), 1);
}
