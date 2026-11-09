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

/// The logged CDP methods minus `Page.enable`, which `browser_extra` sends
/// once per tab for dialog tracking before any page-driving call.
fn page_driving(log: &[String]) -> Vec<&String> {
    log.iter()
        .filter(|method| method.as_str() != "Page.enable")
        .collect()
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
    let out = call(
        &mut server,
        "browser_get_state",
        json!({"cdp": CDP, "format": "json"}),
    );
    let elements = out["elements"].as_array().expect("elements");
    assert_eq!(elements.len(), 3, "{elements:?}");
    assert_eq!(elements[0]["label"], "Email");
    assert_eq!(elements[1]["label"], "Sign in");
    assert_eq!(elements[2]["label"], "Country");
}

/// The default state is one line per control, in the same order and with
/// the same 1-based refs the index-taking tools accept.
#[test]
fn get_state_text_is_one_line_per_control() {
    let (mut server, _log) = server(ScriptBuilder::new().observe_compact(&page()));
    let out = call(&mut server, "browser_get_state", json!({"cdp": CDP}));
    assert_eq!(out["format"], "text", "{out}");
    let text = out["text"].as_str().expect("text");
    let lines: Vec<&str> = text.lines().collect();
    assert_eq!(lines[0], "Form  http://127.0.0.1/form", "{text}");
    assert!(lines[1].starts_with("@e1 textbox \"Email\""), "{text}");
    assert!(lines[2].starts_with("@e2 button \"Sign in\""), "{text}");
    assert!(lines[3].starts_with("@e3 combobox \"Country\""), "{text}");
    assert!(lines[3].contains("selected=\"Canada\""), "{text}");
    assert!(lines[4].starts_with("3 controls · snapshot "), "{text}");
    assert_eq!(lines.len(), 5, "{text}");
}

#[test]
fn get_state_rejects_an_unknown_format() {
    let (mut server, _log) = server(ScriptBuilder::new());
    let err = server
        .call_tool("browser_get_state", &json!({"cdp": CDP, "format": "yaml"}))
        .expect_err("unknown format");
    assert!(matches!(err, ToolError::InvalidArguments(_)), "{err:?}");
}

/// `index` is the same 1-based index as an integer, a digit string, or the
/// `@eN` ref the text state prints.
#[test]
fn index_accepts_integer_digit_string_and_at_e_ref() {
    for index in [json!(2), json!("2"), json!("@e2")] {
        let (mut server, log) = server(
            ScriptBuilder::new()
                .observe_compact(&page())
                .observe_compact(&page())
                .dom_click(11),
        );
        let out = call(
            &mut server,
            "browser_click",
            json!({"cdp": CDP, "index": index}),
        );
        assert_eq!(out["acted"], "click", "{index}: {out}");
        assert!(log.borrow().iter().any(|m| m == "DOM.resolveNode"));
    }
    for bad in [json!("e2"), json!("@x2"), json!("two"), json!(-1)] {
        let (mut server, log) = server(ScriptBuilder::new().observe_compact(&page()));
        let err = server
            .call_tool("browser_click", &json!({"cdp": CDP, "index": bad}))
            .expect_err("bad index");
        assert!(
            matches!(err, ToolError::InvalidArguments(_)),
            "{bad}: {err:?}"
        );
        // `Page.enable` (dialog tracking, browser_extra) may run first; it
        // neither observes nor presses.
        assert!(
            page_driving(&log.borrow()).is_empty(),
            "{bad}: nothing observed or pressed"
        );
    }
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
    let out = call(
        &mut server,
        "browser_get_state",
        json!({"cdp": CDP, "format": "json"}),
    );
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

/// A conditional wait stops at the first look where the URL holds.
#[test]
fn wait_for_url_stops_on_the_first_look_that_holds() {
    let (mut server, _log) = server(ScriptBuilder::new().observe_compact(&page()));
    let out = call(
        &mut server,
        "browser_wait",
        json!({"cdp": CDP, "url": "/form"}),
    );
    assert_eq!(out["seen"], true, "{out}");
    assert_eq!(out["looks"], 1, "{out}");
    assert!(out["after_ms"].as_u64().is_some(), "{out}");
}

/// A control that shows up on the second observe ends the wait there.
#[test]
fn wait_for_control_observes_again_until_it_appears() {
    let before = PageSpec::of(
        &[Control::button(
            11,
            101,
            "Sign in",
            (20.0, 80.0, 120.0, 28.0),
        )],
        "http://127.0.0.1/form",
        "Form",
    );
    let (mut server, _log) = server(
        ScriptBuilder::new()
            .observe_compact(&before)
            .observe_compact(&page()),
    );
    let out = call(
        &mut server,
        "browser_wait",
        json!({"cdp": CDP, "control": "country", "seconds": 5}),
    );
    assert_eq!(out["seen"], true, "{out}");
    assert_eq!(out["looks"], 2, "{out}");
}

/// `text` reads the page text after the observe; it is matched
/// case-insensitively.
#[test]
fn wait_for_text_reads_page_text() {
    let (mut server, log) = server(ScriptBuilder::new().observe_compact(&page()).call(
        "Runtime.evaluate",
        json!({"result": {"type": "string", "value": "Welcome back, Kid"}}),
    ));
    let out = call(
        &mut server,
        "browser_wait",
        json!({"cdp": CDP, "text": "welcome BACK"}),
    );
    assert_eq!(out["seen"], true, "{out}");
    assert!(log.borrow().iter().any(|m| m == "Runtime.evaluate"));
}

/// Out of time: `seen: false` with the last URL, and nothing was pressed.
#[test]
fn wait_timeout_reports_not_seen_and_the_last_url() {
    let (mut server, log) = server(ScriptBuilder::new().observe_compact(&page()));
    let out = call(
        &mut server,
        "browser_wait",
        json!({"cdp": CDP, "url": "/dashboard", "seconds": 0}),
    );
    assert_eq!(out["seen"], false, "{out}");
    assert_eq!(out["url"], "http://127.0.0.1/form", "{out}");
    assert_eq!(out["looks"], 1, "{out}");
    assert!(log
        .borrow()
        .iter()
        .all(|m| m != "Input.dispatchMouseEvent" && m != "DOM.resolveNode"));
}

/// `gone` holds once no control has exactly that label: "Sign" (a
/// substring) never blocks it, "  SIGN   in " (same label) does until the
/// button leaves.
#[test]
fn wait_for_gone_observes_until_the_exact_label_leaves() {
    let after = PageSpec::of(
        &[Control::text_field(
            10,
            100,
            "Email",
            (20.0, 20.0, 300.0, 28.0),
        )],
        "http://127.0.0.1/form",
        "Form",
    );
    let (mut srv, _log) = server(
        ScriptBuilder::new()
            .observe_compact(&page())
            .observe_compact(&after),
    );
    let out = call(
        &mut srv,
        "browser_wait",
        json!({"cdp": CDP, "gone": "  SIGN   in ", "seconds": 5}),
    );
    assert_eq!(out["seen"], true, "{out}");
    assert_eq!(out["looks"], 2, "{out}");

    let (mut srv, _log) = server(ScriptBuilder::new().observe_compact(&page()));
    let out = call(
        &mut srv,
        "browser_wait",
        json!({"cdp": CDP, "gone": "Sign"}),
    );
    assert_eq!(out["seen"], true, "a substring is not the label: {out}");
    assert_eq!(out["looks"], 1, "{out}");
}

/// `load` reads `document.readyState` after each observe: `domcontentloaded`
/// holds at interactive, `load` waits for complete.
#[test]
fn wait_for_load_reads_ready_state() {
    let (mut srv, log) = server(
        ScriptBuilder::new()
            .observe_compact(&page())
            .ready_state("loading")
            .observe_compact(&page())
            .ready_state("interactive"),
    );
    let out = call(
        &mut srv,
        "browser_wait",
        json!({"cdp": CDP, "load": "domcontentloaded", "seconds": 5}),
    );
    assert_eq!(out["seen"], true, "{out}");
    assert_eq!(out["looks"], 2, "{out}");
    assert!(log.borrow().iter().any(|m| m == "Runtime.evaluate"));

    let (mut srv, _log) = server(
        ScriptBuilder::new()
            .observe_compact(&page())
            .ready_state("interactive")
            .observe_compact(&page())
            .ready_state("complete"),
    );
    let out = call(
        &mut srv,
        "browser_wait",
        json!({"cdp": CDP, "load": "load", "seconds": 5}),
    );
    assert_eq!(out["seen"], true, "{out}");
    assert_eq!(out["looks"], 2, "{out}");
}

/// A `load` other than domcontentloaded|load is refused before any CDP call.
#[test]
fn wait_rejects_an_unknown_load_state() {
    let (mut server, log) = server(ScriptBuilder::new());
    for load in ["networkidle", ""] {
        let err = server
            .call_tool("browser_wait", &json!({"cdp": CDP, "load": load}))
            .expect_err("unknown load state");
        assert!(matches!(err, ToolError::InvalidArguments(_)), "{err:?}");
    }
    let err = server
        .call_tool("browser_wait", &json!({"cdp": CDP, "gone": " "}))
        .expect_err("empty gone");
    assert!(matches!(err, ToolError::InvalidArguments(_)), "{err:?}");
    assert!(log.borrow().is_empty());
}

/// No condition keeps the plain sleep and never connects.
#[test]
fn wait_without_a_condition_only_sleeps() {
    let (mut server, log) = server(ScriptBuilder::new());
    let out = call(&mut server, "browser_wait", json!({"seconds": 0}));
    assert_eq!(out, json!({"waited": 0.0}));
    assert!(log.borrow().is_empty());
}

fn policy(allow: &str, confirm: &str) -> ultra_instinct_guard::Policy {
    ultra_instinct_guard::Policy::from_lists(allow, confirm).unwrap()
}

/// `browser_navigate` checks the URL against the allowlist before any CDP
/// call; an allowed host navigates as before.
#[test]
fn navigate_off_the_allowlist_is_refused_before_cdp() {
    let (server_, log) =
        server(ScriptBuilder::new().call("Page.navigate", json!({"frameId": "F1"})));
    let mut server = server_.with_policy(policy("127.0.0.1, *.shop.example", ""));
    for url in ["https://evil.test/", "https://shop.example/"] {
        let err = server
            .call_tool("browser_navigate", &json!({"cdp": CDP, "url": url}))
            .expect_err("off the allowlist");
        // Typed now (`domain_not_allowed`); the wire variant and message
        // are what they were.
        assert!(
            matches!(&err, ToolError::DomainNotAllowed { .. }),
            "{url}: {err:?}"
        );
        assert_eq!(err.to_value()["variant"], "TicketInvalid");
        assert!(
            err.to_value()["message"]
                .as_str()
                .is_some_and(|m| m.starts_with("domain-not-allowed")),
            "{url}: {err:?}"
        );
    }
    assert!(
        page_driving(&log.borrow()).is_empty(),
        "no CDP call but Page.enable: {:?}",
        log.borrow()
    );
    let out = call(
        &mut server,
        "browser_navigate",
        json!({"cdp": CDP, "url": "http://127.0.0.1/next"}),
    );
    assert_eq!(out["navigated"], "http://127.0.0.1/next", "{out}");
}

/// A click on a label that matches a confirm pattern refuses with the
/// label (no press); `confirm` naming that label presses it.
#[test]
fn click_needs_confirm_until_confirm_names_the_label() {
    let (server_, log) = server(ScriptBuilder::new().observe_compact(&page()));
    let mut refusing = server_.with_policy(policy("", "sign*"));
    let err = refusing
        .call_tool("browser_click", &json!({"cdp": CDP, "index": "@e2"}))
        .expect_err("needs confirm");
    assert_eq!(
        err,
        ToolError::NeedsConfirm {
            label: "Sign in".into()
        }
    );
    assert_eq!(
        err.to_value(),
        json!({"code": "needs_confirm", "variant": "TicketInvalid",
               "message": "needs-confirm: \"Sign in\"", "label": "Sign in"})
    );
    assert_eq!(
        err.to_string(),
        ToolError::TicketInvalid("needs-confirm: \"Sign in\"".into()).to_string()
    );
    assert!(log.borrow().iter().all(|m| m != "DOM.resolveNode"));

    let (server_, log) = server(
        ScriptBuilder::new()
            .observe_compact(&page())
            .observe_compact(&page())
            .dom_click(11),
    );
    let mut server = server_.with_policy(policy("", "sign*"));
    let out = call(
        &mut server,
        "browser_click",
        json!({"cdp": CDP, "index": 2, "confirm": "sign in"}),
    );
    assert_eq!(out["acted"], "click", "{out}");
    assert!(out["ticket"].as_u64().is_some(), "{out}");
    assert!(log.borrow().iter().any(|m| m == "DOM.resolveNode"));
}

/// The consume_ticket_once press path (double click) checks the page URL
/// too, and refuses before any mouse event.
#[test]
fn double_click_on_a_page_off_the_allowlist_is_refused() {
    let (server_, log) = server(ScriptBuilder::new().observe_compact(&page()));
    let mut server = server_.with_policy(policy("shop.example", ""));
    let err = server
        .call_tool("browser_double_click", &json!({"cdp": CDP, "index": 2}))
        .expect_err("off the allowlist");
    assert_eq!(
        err,
        ToolError::DomainNotAllowed {
            host: "127.0.0.1".into()
        }
    );
    assert_eq!(
        err.to_value(),
        json!({"code": "domain_not_allowed", "variant": "TicketInvalid",
               "message": "domain-not-allowed: 127.0.0.1", "host": "127.0.0.1"})
    );
    assert!(log.borrow().iter().all(|m| m != "Input.dispatchMouseEvent"));
}

/// Typing is not a press: a confirm pattern on its label does not refuse.
#[test]
fn confirm_patterns_do_not_refuse_typing() {
    let (server_, _log) = server(
        ScriptBuilder::new()
            .observe_compact(&page())
            .observe_compact(&page())
            .dom_input(10),
    );
    let mut server = server_.with_policy(policy("127.0.0.1", "email"));
    let out = call(
        &mut server,
        "browser_type",
        json!({"cdp": CDP, "index": 1, "text": "kid@example.com"}),
    );
    assert_eq!(out["acted"], "type", "{out}");
}

/// `verify: true` observes again after the press and names the effect.
#[test]
fn click_with_verify_reports_the_verification() {
    let (mut server, _log) = server(
        ScriptBuilder::new()
            .observe_compact(&page())
            .observe_compact(&page())
            .dom_click(11)
            .ready_state("complete")
            .observe_compact(&page()),
    );
    let out = call(
        &mut server,
        "browser_click",
        json!({"cdp": CDP, "index": "@e2", "verify": true}),
    );
    assert_eq!(out["verification"], "dispatched-no-visible-change", "{out}");
    assert!(out["ticket"].as_u64().is_some(), "{out}");
}

/// `browser_send_key` with an index is a ticketed KEY: a target that takes
/// no keys refuses before any key event.
#[test]
fn send_key_with_an_index_is_gated() {
    let (mut server, log) = server(ScriptBuilder::new().observe_compact(&page()));
    let err = server
        .call_tool(
            "browser_send_key",
            &json!({"cdp": CDP, "index": 2, "key": "Enter"}),
        )
        .expect_err("a button takes no keys");
    assert_eq!(err, ToolError::TicketInvalid("unsupported-action".into()));
    assert!(log.borrow().iter().all(|m| m != "Input.dispatchKeyEvent"));
}
