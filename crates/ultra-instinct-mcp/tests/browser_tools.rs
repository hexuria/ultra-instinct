//! browser_* tools over the replay harness: a scripted Chrome drives the
//! same `Server::call_tool` path a live session takes. Each test pins the
//! CDP call sequence a tool emits — navigation, observe, gate → ticket →
//! press for mutations, one-shot diagnostics.

use std::cell::RefCell;
use std::rc::Rc;

use serde_json::{json, Value};
use ultra_instinct_browser::script::{Control, PageSpec, ScriptBuilder};
use ultra_instinct_browser::{CdpError, CdpTransport, ReplayTransport};
use ultra_instinct_mcp::{Server, ToolError};

const CDP: &str = "http://127.0.0.1:9222";

struct Logged {
    inner: ReplayTransport,
    log: Rc<RefCell<Vec<String>>>,
}

impl CdpTransport for Logged {
    fn call(&mut self, method: &str, params_json: &str) -> Result<String, CdpError> {
        self.log.borrow_mut().push(method.to_owned());
        self.inner.call(method, params_json)
    }

    fn drain_events(&mut self) -> Vec<ultra_instinct_cdp::CdpEvent> {
        self.inner.drain_events()
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

/// Interactive index of a control in [`page`]: reading order
/// `(floor(y/20), x, id)` — Email y=20 → 1, Sign in y=80 → 2, Country y=140 → 3.
fn page() -> PageSpec {
    let select = Control {
        tag: "SELECT",
        role: "combobox",
        ..Control::combobox(12, 102, "Country", (20.0, 140.0, 200.0, 28.0))
    };
    let mut page = PageSpec::of(
        &[
            Control::text_field(10, 100, "Email", (20.0, 20.0, 300.0, 28.0)),
            Control::button(11, 101, "Sign in", (20.0, 80.0, 120.0, 28.0)),
            select,
        ],
        "http://127.0.0.1/form",
        "Form",
    );
    page.dom[2] = page.dom[2]
        .clone()
        .with_compact_state(json!({"o": ["Canada", "France"], "sel": "Canada"}));
    page
}

#[test]
fn navigate_sends_page_navigate() {
    let (mut server, _log) =
        server(ScriptBuilder::new().call("Page.navigate", json!({"frameId": "F1"})));
    let out = call(
        &mut server,
        "browser_navigate",
        json!({"cdp": CDP, "url": "http://127.0.0.1/next"}),
    );
    assert_eq!(out["navigated"], "http://127.0.0.1/next", "{out}");
}

#[test]
fn get_state_indexes_elements_in_reading_order() {
    let (mut server, _log) = server(ScriptBuilder::new().observe_compact(&page()));
    let out = call(&mut server, "browser_get_state", json!({"cdp": CDP}));
    let elements = out["elements"].as_array().expect("elements");
    assert_eq!(elements.len(), 3, "{elements:?}");
    assert_eq!(elements[0]["label"], "Email");
    assert_eq!(elements[1]["label"], "Sign in");
    assert_eq!(elements[2]["label"], "Country");
}

#[test]
fn find_matches_by_label() {
    let (mut server, _log) = server(ScriptBuilder::new().observe_compact(&page()));
    let out = call(
        &mut server,
        "browser_find",
        json!({"cdp": CDP, "query": "Sign"}),
    );
    let matches = out["matches"].as_array().expect("matches");
    assert_eq!(matches.len(), 1, "{matches:?}");
    assert_eq!(matches[0]["label"], "Sign in");
}

#[test]
fn exec_returns_evaluate_result() {
    let (mut server, _log) = server(ScriptBuilder::new().call(
        "Runtime.evaluate",
        json!({"result": {"type": "number", "value": 42}}),
    ));
    let out = call(
        &mut server,
        "browser_exec",
        json!({"cdp": CDP, "expression": "1+1"}),
    );
    assert_eq!(out["result"], 42, "{out}");
}

#[test]
fn extract_content_returns_markdown() {
    let (mut server, _log) = server(ScriptBuilder::new().call(
        "Runtime.evaluate",
        json!({"result": {"type": "string", "value": "# Title\nbody"}}),
    ));
    let out = call(&mut server, "browser_extract_content", json!({"cdp": CDP}));
    assert_eq!(out["content"], "# Title\nbody", "{out}");
}

#[test]
fn read_console_enables_domains_and_drains() {
    let (mut server, log) = server(
        ScriptBuilder::new()
            .call("Runtime.enable", json!({}))
            .call("Log.enable", json!({})),
    );
    let out = call(&mut server, "browser_read_console", json!({"cdp": CDP}));
    assert_eq!(out["entries"], json!([]), "{out}");
    // A second read must not re-enable — domains are tracked per session key.
    let out = call(&mut server, "browser_read_console", json!({"cdp": CDP}));
    assert_eq!(out["entries"], json!([]), "{out}");
    assert_eq!(
        log.borrow()
            .iter()
            .filter(|m| *m == "Runtime.enable")
            .count(),
        1
    );
}

#[test]
fn read_network_enables_and_drains() {
    let (mut server, _log) = server(ScriptBuilder::new().call("Network.enable", json!({})));
    let out = call(&mut server, "browser_read_network", json!({"cdp": CDP}));
    assert_eq!(out["entries"], json!([]), "{out}");
}

#[test]
fn click_gates_tickets_and_presses_dom_side() {
    // get_state's observe → gated click's fresh executor observe → press.
    let (mut server, log) = server(
        ScriptBuilder::new()
            .observe_compact(&page())
            .observe_compact(&page())
            .dom_click(11),
    );
    let out = call(
        &mut server,
        "browser_click",
        json!({"cdp": CDP, "index": 2}),
    );
    assert_eq!(out["acted"], "click", "{out}");
    assert!(
        log.borrow().iter().any(|m| m == "DOM.resolveNode"),
        "DOM-tier press expected: {:?}",
        log.borrow()
    );
}

#[test]
fn click_with_mouse_true_presses_a_trusted_pointer() {
    let (mut server, log) = server(
        ScriptBuilder::new()
            .observe_compact(&page())
            .observe_compact(&page())
            .pointer_click(),
    );
    let out = call(
        &mut server,
        "browser_click",
        json!({"cdp": CDP, "index": 2, "mouse": true}),
    );
    assert_eq!(out["acted"], "click", "{out}");
    let log = log.borrow();
    assert!(
        log.iter().any(|m| m == "Input.dispatchMouseEvent"),
        "pointer press expected: {log:?}"
    );
    assert!(
        !log.iter().any(|m| m == "DOM.resolveNode"),
        "no DOM tier: {log:?}"
    );
}

#[test]
fn double_click_tickets_then_dispatches_press_release() {
    let (mut server, log) = server(
        ScriptBuilder::new()
            .observe_compact(&page())
            .call("Input.dispatchMouseEvent", json!({}))
            .call("Input.dispatchMouseEvent", json!({}))
            .call("Input.dispatchMouseEvent", json!({})),
    );
    let out = call(
        &mut server,
        "browser_double_click",
        json!({"cdp": CDP, "index": 2}),
    );
    assert_eq!(out["acted"], true, "{out}");
    assert_eq!(
        log.borrow()
            .iter()
            .filter(|m| *m == "Input.dispatchMouseEvent")
            .count(),
        3,
        "moved + pressed + released"
    );
}

#[test]
fn right_click_dispatches_with_right_button() {
    let (mut server, log) = server(
        ScriptBuilder::new()
            .observe_compact(&page())
            .call("Input.dispatchMouseEvent", json!({}))
            .call("Input.dispatchMouseEvent", json!({}))
            .call("Input.dispatchMouseEvent", json!({})),
    );
    let out = call(
        &mut server,
        "browser_right_click",
        json!({"cdp": CDP, "index": 2}),
    );
    assert_eq!(out["acted"], true, "{out}");
    assert_eq!(
        log.borrow()
            .iter()
            .filter(|m| *m == "Input.dispatchMouseEvent")
            .count(),
        3
    );
}

#[test]
fn hover_moves_pointer_without_ticket() {
    let (mut server, log) = server(
        ScriptBuilder::new()
            .observe_compact(&page())
            .call("Input.dispatchMouseEvent", json!({})),
    );
    let out = call(
        &mut server,
        "browser_hover",
        json!({"cdp": CDP, "index": 2}),
    );
    assert_eq!(out["hovered"], 2, "{out}");
    assert_eq!(
        log.borrow()
            .iter()
            .filter(|m| *m == "Input.dispatchMouseEvent")
            .count(),
        1
    );
}

#[test]
fn drag_dispatches_move_press_move_release() {
    // move to A, press, DRAG_STEPS moves toward B, release at B.
    let events = 3 + ultra_instinct_browser::DRAG_STEPS as usize;
    let mut script = ScriptBuilder::new().observe_compact(&page());
    for _ in 0..events {
        script = script.call("Input.dispatchMouseEvent", json!({}));
    }
    let (mut server, log) = server(script);
    let out = call(
        &mut server,
        "browser_drag",
        json!({"cdp": CDP, "from_index": 1, "to_index": 2}),
    );
    assert_eq!(out["dragged"], json!({"from": 1, "to": 2}), "{out}");
    assert_eq!(
        log.borrow()
            .iter()
            .filter(|m| *m == "Input.dispatchMouseEvent")
            .count(),
        events
    );
}

#[test]
fn a_drag_that_fails_mid_gesture_still_releases_the_button() {
    // Move to A, press, one move, then the second move fails.
    let script = ScriptBuilder::new()
        .observe_compact(&page())
        .call("Input.dispatchMouseEvent", json!({}))
        .call("Input.dispatchMouseEvent", json!({}))
        .call("Input.dispatchMouseEvent", json!({}))
        .call_error("Input.dispatchMouseEvent", "target closed")
        .call("Input.dispatchMouseEvent", json!({}));
    let (mut server, log) = server(script);
    assert!(server
        .call_tool(
            "browser_drag",
            &json!({"cdp": CDP, "from_index": 1, "to_index": 2}),
        )
        .is_err());
    assert_eq!(
        log.borrow()
            .iter()
            .filter(|m| *m == "Input.dispatchMouseEvent")
            .count(),
        5,
        "the release follows the failed move"
    );
}

#[test]
fn drag_onto_its_own_source_is_refused_before_any_press() {
    let (mut server, log) = server(ScriptBuilder::new().observe_compact(&page()));
    let err = server
        .call_tool(
            "browser_drag",
            &json!({"cdp": CDP, "from_index": 2, "to_index": 2}),
        )
        .unwrap_err();
    assert!(
        matches!(&err, ToolError::TicketInvalid(m) if m == "unsupported-action"),
        "{err:?}"
    );
    assert!(!log.borrow().iter().any(|m| m == "Input.dispatchMouseEvent"));
}

#[test]
fn drag_of_a_drag_only_list_item_binds_both_ends() {
    // Sortable rows claim drag, not click: the click gate would refuse them.
    let row = |node_id, backend, label, y| Control {
        tag: "LI",
        role: "listitem",
        ..Control::button(node_id, backend, label, (20.0, y, 120.0, 20.0))
    };
    let mut page = PageSpec::of(
        &[row(10, 100, "Ana", 20.0), row(11, 101, "Bo", 60.0)],
        "http://127.0.0.1/list",
        "List",
    );
    for node in &mut page.dom {
        *node = node.clone().with_compact_state(json!({"dg": 1}));
    }
    let events = 3 + ultra_instinct_browser::DRAG_STEPS as usize;
    let mut script = ScriptBuilder::new().observe_compact(&page);
    for _ in 0..events {
        script = script.call("Input.dispatchMouseEvent", json!({}));
    }
    let (mut server, log) = server(script);
    let out = call(
        &mut server,
        "browser_drag",
        json!({"cdp": CDP, "from_index": 2, "to_index": 1}),
    );
    assert_eq!(out["dragged"], json!({"from": 2, "to": 1}), "{out}");
    assert_eq!(
        log.borrow()
            .iter()
            .filter(|m| *m == "Input.dispatchMouseEvent")
            .count(),
        events
    );
}

#[test]
fn send_key_dispatches_down_and_up() {
    let (mut server, log) = server(
        ScriptBuilder::new()
            .call("Input.dispatchKeyEvent", json!({}))
            .call("Input.dispatchKeyEvent", json!({})),
    );
    let out = call(
        &mut server,
        "browser_send_key",
        json!({"cdp": CDP, "key": "Enter"}),
    );
    assert_eq!(out["key"], "Enter", "{out}");
    assert_eq!(
        log.borrow()
            .iter()
            .filter(|m| *m == "Input.dispatchKeyEvent")
            .count(),
        2
    );
}

#[test]
fn select_dropdown_grounds_choice_in_observed_options() {
    // observe → fresh executor observe → DOM select.
    let (mut server, _log) = server(
        ScriptBuilder::new()
            .observe_compact(&page())
            .observe_compact(&page())
            .dom_input(12),
    );
    let out = call(
        &mut server,
        "browser_select_dropdown",
        json!({"cdp": CDP, "index": 3, "text": "France"}),
    );
    assert_eq!(out["acted"], "select", "{out}");
}

#[test]
fn select_dropdown_rejects_unobserved_option() {
    let (mut server, _log) = server(
        ScriptBuilder::new()
            .observe_compact(&page())
            .observe_compact(&page()),
    );
    let err = server
        .call_tool(
            "browser_select_dropdown",
            &json!({"cdp": CDP, "index": 3, "text": "Atlantis"}),
        )
        .expect_err("unobserved option must not press");
    let _ = err;
}

#[test]
fn get_dropdown_options_reads_observed_state() {
    let (mut server, _log) = server(ScriptBuilder::new().observe_compact(&page()));
    let out = call(
        &mut server,
        "browser_get_dropdown_options",
        json!({"cdp": CDP, "index": 3}),
    );
    assert_eq!(out["options"], json!(["Canada", "France"]), "{out}");
    assert_eq!(out["selected"], "Canada");
}

#[test]
fn file_upload_sets_files_on_the_bound_node() {
    let (mut server, log) = server(
        ScriptBuilder::new()
            .observe_compact(&page())
            .call("DOM.setFileInputFiles", json!({})),
    );
    let out = call(
        &mut server,
        "browser_file_upload",
        json!({"cdp": CDP, "index": 2, "path": "/tmp/f.pdf"}),
    );
    assert_eq!(out["acted"], true, "{out}");
    assert!(
        log.borrow().iter().any(|m| m == "DOM.setFileInputFiles"),
        "{:?}",
        log.borrow()
    );
}

#[test]
fn save_as_pdf_returns_base64() {
    let (mut server, _log) =
        server(ScriptBuilder::new().call("Page.printToPDF", json!({"data": "JVBERi0="})));
    let out = call(&mut server, "browser_save_as_pdf", json!({"cdp": CDP}));
    assert_eq!(out["data"], "JVBERi0=", "{out}");
}

#[test]
fn zoom_sets_page_scale_factor() {
    let (mut server, log) =
        server(ScriptBuilder::new().call("Emulation.setPageScaleFactor", json!({})));
    let out = call(
        &mut server,
        "browser_zoom",
        json!({"cdp": CDP, "factor": 1.5}),
    );
    assert_eq!(out["zoomed"], 1.5, "{out}");
    assert!(log
        .borrow()
        .iter()
        .any(|m| m == "Emulation.setPageScaleFactor"));
}

#[test]
fn scroll_to_scrolls_the_indexed_element_into_view() {
    let (mut server, _log) = server(
        ScriptBuilder::new()
            .observe_compact(&page())
            .call("DOM.scrollIntoViewIfNeeded", json!({})),
    );
    let out = call(
        &mut server,
        "browser_scroll_to",
        json!({"cdp": CDP, "index": 2}),
    );
    assert_eq!(out["scrolled"], true, "{out}");
}

#[test]
fn go_back_uses_current_index_not_entry_id() {
    // Entry ids are opaque (101/102 do not match positions 0/1). The tool
    // must navigate to entries[currentIndex - 1], not entries.find(id == 0).
    let (mut server, log) = server(
        ScriptBuilder::new()
            .call(
                "Page.getNavigationHistory",
                json!({"currentIndex": 1, "entries": [
                    {"id": 101, "url": "http://127.0.0.1/a", "title": "A"},
                    {"id": 102, "url": "http://127.0.0.1/b", "title": "B"},
                ]}),
            )
            .call("Page.navigateToHistoryEntry", json!({})),
    );
    let out = call(&mut server, "browser_go_back", json!({"cdp": CDP}));
    assert_eq!(out["went_back"], true, "{out}");
    assert!(log
        .borrow()
        .iter()
        .any(|m| m == "Page.navigateToHistoryEntry"));
}

#[test]
fn get_state_reports_url_and_title_after_observe() {
    let (mut server, _log) = server(ScriptBuilder::new().observe_compact(&page()));
    let out = call(&mut server, "browser_get_state", json!({"cdp": CDP}));
    assert_eq!(out["url"], "http://127.0.0.1/form", "{out}");
    assert_eq!(out["title"], "Form", "{out}");
}

#[test]
fn missing_index_is_an_argument_error_not_a_press() {
    let (mut server, log) = server(ScriptBuilder::new().observe_compact(&page()));
    let err = server
        .call_tool("browser_click", &json!({"cdp": CDP, "index": 99}))
        .expect_err("index out of range");
    let _ = err;
    assert!(
        log.borrow()
            .iter()
            .all(|m| m != "DOM.resolveNode" && m != "Input.dispatchMouseEvent"),
        "no press may be dispatched: {:?}",
        log.borrow()
    );
}

/// `path` writes the decoded artifact, not its base64, for both tools that
/// return one; the body still carries the base64 and reports what it wrote.
#[test]
fn screenshot_and_pdf_write_decoded_bytes_to_path() {
    let dir = std::env::temp_dir().join(format!(
        "ultra-instinct-mcp-artifacts-{}",
        std::process::id()
    ));
    std::fs::create_dir_all(&dir).expect("temp dir");
    // "iVBORw0KGgo=" and "JVBERi0=" decode to the PNG and PDF signatures.
    let cases = [
        (
            "browser_screenshot",
            "Page.captureScreenshot",
            "iVBORw0KGgo=",
            "base64",
            &b"\x89PNG\r\n\x1a\n"[..],
            "shot.png",
        ),
        (
            "browser_save_as_pdf",
            "Page.printToPDF",
            "JVBERi0=",
            "data",
            &b"%PDF-"[..],
            "page.pdf",
        ),
    ];
    for (tool, method, encoded, field, decoded, file) in cases {
        let (mut server, _log) =
            server(ScriptBuilder::new().call(method, json!({"data": encoded})));
        let path = dir.join(file);
        let out = call(&mut server, tool, json!({"cdp": CDP, "path": path}));
        assert_eq!(std::fs::read(&path).expect("written"), decoded, "{tool}");
        assert_eq!(out[field], encoded, "{tool}: {out}");
        assert_eq!(out["path"], json!(path), "{tool}: {out}");
        assert_eq!(out["bytes"], decoded.len(), "{tool}: {out}");
    }
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn unwritable_artifact_path_is_an_error() {
    let (mut server, _log) = server(
        ScriptBuilder::new().call("Page.captureScreenshot", json!({"data": "iVBORw0KGgo="})),
    );
    let err = server
        .call_tool(
            "browser_screenshot",
            &json!({"cdp": CDP, "path": "/nonexistent-ultra-instinct-dir/shot.png"}),
        )
        .expect_err("cannot write there");
    assert!(matches!(err, ToolError::InvalidArguments(_)), "{err:?}");
}
