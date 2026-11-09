//! `browser_set` / `browser_download` / `browser_network` over the replay
//! harness. A scripted Chrome answers each CDP call in order; a hook on the
//! wire turns chosen calls into unsolicited events, the way a live socket
//! buffers frames that arrive while a command is in flight.

use std::cell::RefCell;
use std::path::PathBuf;
use std::rc::Rc;

use serde_json::{json, Value};
use ultra_instinct_browser::script::{Control, PageSpec, ScriptBuilder};
use ultra_instinct_browser::{CdpError, CdpTransport, ReplayTransport};
use ultra_instinct_cdp::CdpEvent;
use ultra_instinct_mcp::{Server, ToolError};

const CDP: &str = "http://127.0.0.1:9222";

type Calls = Rc<RefCell<Vec<(String, Value)>>>;
type Hook = Box<dyn FnMut(&str, &Value) -> Vec<CdpEvent>>;

struct Wire {
    inner: ReplayTransport,
    calls: Calls,
    pending: Vec<CdpEvent>,
    hook: Hook,
}

impl CdpTransport for Wire {
    fn call(&mut self, method: &str, params_json: &str) -> Result<String, CdpError> {
        let params: Value = serde_json::from_str(params_json).unwrap_or(Value::Null);
        self.calls
            .borrow_mut()
            .push((method.to_owned(), params.clone()));
        let events = (self.hook)(method, &params);
        self.pending.extend(events);
        self.inner.call(method, params_json)
    }

    fn drain_events(&mut self) -> Vec<CdpEvent> {
        std::mem::take(&mut self.pending)
    }
}

fn server_with(script: ScriptBuilder, hook: Hook) -> (Server, Calls) {
    let calls: Calls = Rc::default();
    let shared = Rc::clone(&calls);
    let mut parts = Some((script, hook));
    let server = Server::with_connector(move |_url| {
        let (script, hook) = parts.take().ok_or_else(|| CdpError::Transport {
            message: "one connect per test".into(),
        })?;
        Ok(Box::new(Wire {
            inner: ReplayTransport::parse(&script.to_json())?,
            calls: Rc::clone(&shared),
            pending: Vec::new(),
            hook,
        }) as Box<dyn CdpTransport>)
    });
    (server, calls)
}

fn server(script: ScriptBuilder) -> (Server, Calls) {
    server_with(script, Box::new(|_, _| Vec::new()))
}

fn call(server: &mut Server, tool: &str, arguments: Value) -> Value {
    server
        .call_tool(tool, &arguments)
        .unwrap_or_else(|err| panic!("{tool} {arguments}: {err:?}"))
}

fn methods(calls: &Calls) -> Vec<String> {
    calls.borrow().iter().map(|(m, _)| m.clone()).collect()
}

fn params_of(calls: &Calls, method: &str) -> Vec<Value> {
    calls
        .borrow()
        .iter()
        .filter(|(m, _)| m == method)
        .map(|(_, p)| p.clone())
        .collect()
}

/// Presses sent: DOM script clicks plus pointer button-downs (a move alone
/// presses nothing).
fn presses(calls: &Calls) -> usize {
    calls
        .borrow()
        .iter()
        .filter(|(m, p)| {
            m == "DOM.resolveNode"
                || (m == "Input.dispatchMouseEvent" && p["type"] == "mousePressed")
        })
        .count()
}

fn event(method: &str, params: Value) -> CdpEvent {
    CdpEvent {
        method: method.to_owned(),
        params,
    }
}

fn scratch(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "ultra-instinct-env-test-{}-{name}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// Email (index 1) and a Download button (index 2).
fn page() -> PageSpec {
    PageSpec::of(
        &[
            Control::text_field(10, 100, "Email", (20.0, 20.0, 300.0, 28.0)),
            Control::button(11, 101, "Download report", (20.0, 80.0, 160.0, 28.0)),
        ],
        "http://127.0.0.1/reports",
        "Reports",
    )
}

/// The same page after the button went away.
fn drifted_page() -> PageSpec {
    PageSpec::of(
        &[Control::text_field(
            10,
            100,
            "Email",
            (20.0, 20.0, 300.0, 28.0),
        )],
        "http://127.0.0.1/reports",
        "Reports",
    )
}

// ---- browser_set ------------------------------------------------------------

#[test]
fn set_viewport_overrides_metrics_and_the_next_observe_reads_it() {
    // At 390 px wide, a button at x = 600 is fully offscreen: the observe
    // after the override must read the new layout metrics, not a cached one.
    let mut narrow = PageSpec::of(
        &[
            Control::button(11, 101, "Near", (20.0, 80.0, 120.0, 28.0)),
            Control::button(12, 102, "Far", (600.0, 80.0, 120.0, 28.0)),
        ],
        "http://127.0.0.1/",
        "Wide",
    );
    narrow.width = 390.0;
    narrow.height = 844.0;
    let (mut server, calls) = server(
        ScriptBuilder::new()
            .call("Emulation.setDeviceMetricsOverride", json!({}))
            .observe_compact(&narrow),
    );
    let out = call(
        &mut server,
        "browser_set",
        json!({"cdp": CDP, "what": "viewport", "width": 390, "height": 844}),
    );
    assert_eq!(out["set"], "viewport", "{out}");
    assert_eq!(
        params_of(&calls, "Emulation.setDeviceMetricsOverride"),
        [json!({"width": 390, "height": 844, "deviceScaleFactor": 1.0, "mobile": false})]
    );
    let state = call(
        &mut server,
        "browser_get_state",
        json!({"cdp": CDP, "format": "json"}),
    );
    let labels: Vec<&str> = state["elements"]
        .as_array()
        .unwrap()
        .iter()
        .map(|e| e["label"].as_str().unwrap())
        .collect();
    assert_eq!(labels, ["Near"], "{state}");
    let log = methods(&calls);
    let overridden = log
        .iter()
        .position(|m| m == "Emulation.setDeviceMetricsOverride")
        .unwrap();
    let measured = log
        .iter()
        .position(|m| m == "Page.getLayoutMetrics")
        .unwrap();
    assert!(overridden < measured, "{log:?}");
}

#[test]
fn set_viewport_rejects_out_of_range_sizes_without_cdp() {
    let (mut server, calls) = server(ScriptBuilder::new());
    let err = server
        .call_tool(
            "browser_set",
            &json!({"cdp": CDP, "what": "viewport", "width": 0, "height": 844}),
        )
        .unwrap_err();
    assert!(matches!(err, ToolError::InvalidArguments(_)), "{err:?}");
    assert!(calls.borrow().is_empty());
}

#[test]
fn set_device_sets_metrics_touch_and_user_agent() {
    let (mut server, calls) = server(
        ScriptBuilder::new()
            .call("Emulation.setDeviceMetricsOverride", json!({}))
            .call("Emulation.setTouchEmulationEnabled", json!({}))
            .call("Emulation.setUserAgentOverride", json!({})),
    );
    let out = call(
        &mut server,
        "browser_set",
        json!({"cdp": CDP, "what": "device", "name": "iPhone 15"}),
    );
    assert_eq!(out["name"], "iPhone 15", "{out}");
    assert_eq!(
        params_of(&calls, "Emulation.setDeviceMetricsOverride"),
        [json!({"width": 393, "height": 852, "deviceScaleFactor": 3.0, "mobile": true})]
    );
    assert_eq!(
        params_of(&calls, "Emulation.setTouchEmulationEnabled"),
        [json!({"enabled": true, "maxTouchPoints": 5})]
    );
    let ua = params_of(&calls, "Emulation.setUserAgentOverride");
    assert!(
        ua[0]["userAgent"].as_str().unwrap().contains("iPhone"),
        "{ua:?}"
    );

    let err = server
        .call_tool(
            "browser_set",
            &json!({"cdp": CDP, "what": "device", "name": "Nokia 3310"}),
        )
        .unwrap_err();
    assert!(
        matches!(&err, ToolError::InvalidArguments(m) if m.contains("Pixel 8")),
        "{err:?}"
    );
}

#[test]
fn set_media_emulates_the_color_scheme() {
    let (mut server, calls) =
        server(ScriptBuilder::new().call("Emulation.setEmulatedMedia", json!({})));
    call(
        &mut server,
        "browser_set",
        json!({"cdp": CDP, "what": "media", "color_scheme": "dark"}),
    );
    assert_eq!(
        params_of(&calls, "Emulation.setEmulatedMedia"),
        [json!({"media": "", "features": [{"name": "prefers-color-scheme", "value": "dark"}]})]
    );
    let err = server
        .call_tool(
            "browser_set",
            &json!({"cdp": CDP, "what": "media", "color_scheme": "sepia"}),
        )
        .unwrap_err();
    assert!(matches!(err, ToolError::InvalidArguments(_)), "{err:?}");
}

#[test]
fn set_offline_enables_network_once_and_emulates_conditions() {
    let (mut server, calls) = server(
        ScriptBuilder::new()
            .call("Network.enable", json!({}))
            .call("Network.emulateNetworkConditions", json!({}))
            .call("Network.emulateNetworkConditions", json!({})),
    );
    call(
        &mut server,
        "browser_set",
        json!({"cdp": CDP, "what": "offline", "on": true}),
    );
    call(
        &mut server,
        "browser_set",
        json!({"cdp": CDP, "what": "offline", "on": false}),
    );
    assert_eq!(
        methods(&calls),
        [
            "Network.enable",
            "Network.emulateNetworkConditions",
            "Network.emulateNetworkConditions"
        ]
    );
    let sent = params_of(&calls, "Network.emulateNetworkConditions");
    assert_eq!(sent[0]["offline"], true);
    assert_eq!(sent[1]["offline"], false);
    assert_eq!(sent[0]["downloadThroughput"], -1);
}

#[test]
fn set_headers_sends_the_map_and_echoes_names_only() {
    let (mut server, calls) = server(
        ScriptBuilder::new()
            .call("Network.enable", json!({}))
            .call("Network.setExtraHTTPHeaders", json!({})),
    );
    let out = call(
        &mut server,
        "browser_set",
        json!({"cdp": CDP, "what": "headers", "map": {"X-Trace": "abc", "Authorization": "Bearer s3cret"}}),
    );
    assert_eq!(
        params_of(&calls, "Network.setExtraHTTPHeaders"),
        [json!({"headers": {"X-Trace": "abc", "Authorization": "Bearer s3cret"}})]
    );
    assert!(!out.to_string().contains("s3cret"), "{out}");
    let err = server
        .call_tool(
            "browser_set",
            &json!({"cdp": CDP, "what": "headers", "map": {"X-Count": 3}}),
        )
        .unwrap_err();
    assert!(matches!(err, ToolError::InvalidArguments(_)), "{err:?}");
}

#[test]
fn reset_clears_emulation_and_network_overrides_it_set() {
    // Without Network enabled, reset leaves the Network domain alone.
    let (mut server, calls) = server(
        ScriptBuilder::new()
            .call("Emulation.clearDeviceMetricsOverride", json!({}))
            .call("Emulation.setTouchEmulationEnabled", json!({}))
            .call("Emulation.setUserAgentOverride", json!({}))
            .call("Emulation.setEmulatedMedia", json!({})),
    );
    let out = call(
        &mut server,
        "browser_set",
        json!({"cdp": CDP, "what": "reset"}),
    );
    assert_eq!(out["cleared"].as_array().unwrap().len(), 4, "{out}");
    assert_eq!(
        params_of(&calls, "Emulation.setUserAgentOverride"),
        [json!({"userAgent": ""})]
    );
    assert_eq!(
        params_of(&calls, "Emulation.setEmulatedMedia"),
        [json!({"media": "", "features": [{"name": "prefers-color-scheme", "value": ""}]})]
    );

    // After offline, reset also restores the network conditions and headers.
    let (mut server, calls) = self::server(
        ScriptBuilder::new()
            .call("Network.enable", json!({}))
            .call("Network.emulateNetworkConditions", json!({}))
            .call("Emulation.clearDeviceMetricsOverride", json!({}))
            .call("Emulation.setTouchEmulationEnabled", json!({}))
            .call("Emulation.setUserAgentOverride", json!({}))
            .call("Emulation.setEmulatedMedia", json!({}))
            .call("Network.emulateNetworkConditions", json!({}))
            .call("Network.setExtraHTTPHeaders", json!({})),
    );
    call(
        &mut server,
        "browser_set",
        json!({"cdp": CDP, "what": "offline", "on": true}),
    );
    call(
        &mut server,
        "browser_set",
        json!({"cdp": CDP, "what": "reset"}),
    );
    let conditions = params_of(&calls, "Network.emulateNetworkConditions");
    assert_eq!(conditions[1]["offline"], false);
    assert_eq!(
        params_of(&calls, "Network.setExtraHTTPHeaders"),
        [json!({"headers": {}})]
    );
}

// ---- browser_download -------------------------------------------------------

/// Scripted download: Chrome begins it on the press (the pointer release),
/// and completes it on the
/// `complete_on`-th `Browser.getVersion` pump (0 = never), writing the file
/// into the `downloadPath` the tool asked for.
fn download_hook(complete_on: usize, dir_seen: Rc<RefCell<Option<PathBuf>>>) -> Hook {
    let mut pumps = 0;
    Box::new(move |method, params| match method {
        "Browser.setDownloadBehavior" => {
            if let Some(path) = params["downloadPath"].as_str() {
                *dir_seen.borrow_mut() = Some(PathBuf::from(path));
            }
            Vec::new()
        }
        "Input.dispatchMouseEvent" if params["type"] == "mouseReleased" => vec![event(
            "Browser.downloadWillBegin",
            json!({"guid": "g-1", "url": "http://127.0.0.1/r.pdf", "suggestedFilename": "report.pdf"}),
        )],
        "Browser.getVersion" => {
            pumps += 1;
            if pumps != complete_on {
                return vec![event(
                    "Browser.downloadProgress",
                    json!({"guid": "g-1", "state": "inProgress", "receivedBytes": 2, "totalBytes": 5}),
                )];
            }
            let dir = dir_seen.borrow().clone().unwrap();
            std::fs::write(dir.join("g-1"), b"%PDF!").unwrap();
            vec![event(
                "Browser.downloadProgress",
                json!({"guid": "g-1", "state": "completed", "receivedBytes": 5, "totalBytes": 5}),
            )]
        }
        _ => Vec::new(),
    })
}

#[test]
fn download_presses_through_the_ticket_and_waits_for_completed() {
    let out_dir = scratch("download-ok");
    let dest = out_dir.join("report.pdf");
    let dir_seen = Rc::new(RefCell::new(None));
    let (mut server, calls) = server_with(
        ScriptBuilder::new()
            .call("Browser.setDownloadBehavior", json!({}))
            .observe_compact(&page())
            .observe_compact(&page())
            .pointer_click()
            .call("Browser.getVersion", json!({}))
            .call("Browser.getVersion", json!({}))
            .call("Browser.setDownloadBehavior", json!({})),
        download_hook(2, Rc::clone(&dir_seen)),
    );
    let out = call(
        &mut server,
        "browser_download",
        json!({"cdp": CDP, "index": "@e2", "path": dest.to_str().unwrap()}),
    );
    assert_eq!(out["bytes"], 5, "{out}");
    assert_eq!(out["suggested_name"], "report.pdf");
    assert_eq!(out["path"], dest.to_str().unwrap());
    assert_eq!(std::fs::read(&dest).unwrap(), b"%PDF!");

    let behaviors = params_of(&calls, "Browser.setDownloadBehavior");
    assert_eq!(behaviors[0]["behavior"], "allowAndName");
    assert_eq!(behaviors[0]["eventsEnabled"], true);
    assert_eq!(behaviors[1], json!({"behavior": "default"}));
    assert_eq!(presses(&calls), 1, "{:?}", methods(&calls));
    assert!(
        params_of(&calls, "Runtime.callFunctionOn").is_empty(),
        "a pointer press, no script click: {:?}",
        methods(&calls)
    );
    let temp = dir_seen.borrow().clone().unwrap();
    assert!(!temp.exists(), "temp dir {temp:?} must be removed");
    let _ = std::fs::remove_dir_all(out_dir);
}

/// The download's press is a click: with an allow-list, `browser_download`
/// also needs `browser_click`, else `tool_not_allowed` (naming the click)
/// before any CDP call. With the click allowed too, it downloads.
#[test]
fn download_needs_the_click_on_the_allow_list() {
    let out_dir = scratch("download-allow");
    let dest = out_dir.join("report.pdf");
    let (server, calls) = server_with(ScriptBuilder::new(), Box::new(|_, _| Vec::new()));
    let mut server = server.with_tool_allowlist(["browser_get_state", "browser_download"]);
    let err = server
        .call_tool(
            "browser_download",
            &json!({"cdp": CDP, "index": 2, "path": dest.to_str().unwrap()}),
        )
        .unwrap_err();
    assert_eq!(
        err,
        ToolError::ToolNotAllowed {
            tool: "browser_click".into()
        }
    );
    assert!(calls.borrow().is_empty(), "{:?}", methods(&calls));

    let dir_seen = Rc::new(RefCell::new(None));
    let (server, calls) = server_with(
        ScriptBuilder::new()
            .call("Browser.setDownloadBehavior", json!({}))
            .observe_compact(&page())
            .observe_compact(&page())
            .pointer_click()
            .call("Browser.getVersion", json!({}))
            .call("Browser.setDownloadBehavior", json!({})),
        download_hook(1, Rc::clone(&dir_seen)),
    );
    let mut server =
        server.with_tool_allowlist(["browser_get_state", "browser_download", "browser_click"]);
    let out = call(
        &mut server,
        "browser_download",
        json!({"cdp": CDP, "index": 2, "path": dest.to_str().unwrap()}),
    );
    assert_eq!(out["bytes"], 5, "{out}");
    assert_eq!(presses(&calls), 1);
    let _ = std::fs::remove_dir_all(out_dir);
}

#[test]
fn download_times_out_cleanly_and_restores_behavior() {
    let out_dir = scratch("download-timeout");
    let dest = out_dir.join("never.pdf");
    let dir_seen = Rc::new(RefCell::new(None));
    let (mut server, calls) = server_with(
        ScriptBuilder::new()
            .call("Browser.setDownloadBehavior", json!({}))
            .observe_compact(&page())
            .observe_compact(&page())
            .pointer_click()
            .call("Browser.getVersion", json!({}))
            .call("Browser.getVersion", json!({}))
            .call("Browser.setDownloadBehavior", json!({})),
        download_hook(0, Rc::clone(&dir_seen)),
    );
    let err = server
        .call_tool(
            "browser_download",
            &json!({"cdp": CDP, "index": 2, "path": dest.to_str().unwrap(), "timeout_ms": 200}),
        )
        .unwrap_err();
    assert!(
        matches!(&err, ToolError::Browser(m) if m.contains("did not complete within 200 ms")),
        "{err:?}"
    );
    assert_eq!(params_of(&calls, "Browser.getVersion").len(), 2);
    assert_eq!(
        params_of(&calls, "Browser.setDownloadBehavior")[1],
        json!({"behavior": "default"})
    );
    assert!(!dest.exists());
    assert!(!dir_seen.borrow().clone().unwrap().exists());
    let _ = std::fs::remove_dir_all(out_dir);
}

#[test]
fn download_on_a_drifted_page_refuses_without_pressing() {
    let out_dir = scratch("download-stale");
    let dest = out_dir.join("report.pdf");
    let (mut server, calls) = server(
        ScriptBuilder::new()
            .call("Browser.setDownloadBehavior", json!({}))
            .observe_compact(&page())
            .observe_compact(&drifted_page())
            .call("Browser.setDownloadBehavior", json!({})),
    );
    let err = server
        .call_tool(
            "browser_download",
            &json!({"cdp": CDP, "index": 2, "path": dest.to_str().unwrap()}),
        )
        .unwrap_err();
    // A drifted page is a stale ref (its wire `variant` is `TicketInvalid`).
    assert!(matches!(err, ToolError::StaleRef { .. }), "{err:?}");
    assert_eq!(err.to_value()["variant"], "TicketInvalid");
    assert_eq!(presses(&calls), 0, "{:?}", methods(&calls));
    assert!(params_of(&calls, "Input.dispatchMouseEvent").is_empty());
    assert!(params_of(&calls, "Browser.getVersion").is_empty());
    assert_eq!(
        params_of(&calls, "Browser.setDownloadBehavior")[1],
        json!({"behavior": "default"})
    );
    let _ = std::fs::remove_dir_all(out_dir);
}

#[test]
fn download_refuses_a_directory_or_an_existing_file_before_any_cdp() {
    let out_dir = scratch("download-dest");
    let existing = out_dir.join("exists.pdf");
    std::fs::write(&existing, b"old").unwrap();
    let (mut server, calls) = server(ScriptBuilder::new());
    for path in [out_dir.as_path(), existing.as_path()] {
        let err = server
            .call_tool(
                "browser_download",
                &json!({"cdp": CDP, "index": 2, "path": path.to_str().unwrap()}),
            )
            .unwrap_err();
        assert!(matches!(err, ToolError::InvalidArguments(_)), "{err:?}");
    }
    // A directory stays refused even with overwrite.
    let err = server
        .call_tool(
            "browser_download",
            &json!({"cdp": CDP, "index": 2, "path": out_dir.to_str().unwrap(), "overwrite": true}),
        )
        .unwrap_err();
    assert!(matches!(err, ToolError::InvalidArguments(_)), "{err:?}");
    assert!(calls.borrow().is_empty());
    assert_eq!(std::fs::read(&existing).unwrap(), b"old");
    let _ = std::fs::remove_dir_all(out_dir);
}

#[test]
fn download_with_overwrite_replaces_an_existing_file() {
    let out_dir = scratch("download-overwrite");
    let dest = out_dir.join("report.pdf");
    std::fs::write(&dest, b"old").unwrap();
    let dir_seen = Rc::new(RefCell::new(None));
    let (mut server, _calls) = server_with(
        ScriptBuilder::new()
            .call("Browser.setDownloadBehavior", json!({}))
            .observe_compact(&page())
            .observe_compact(&page())
            .pointer_click()
            .call("Browser.getVersion", json!({}))
            .call("Browser.setDownloadBehavior", json!({})),
        download_hook(1, dir_seen),
    );
    call(
        &mut server,
        "browser_download",
        json!({"cdp": CDP, "index": 2, "path": dest.to_str().unwrap(), "overwrite": true}),
    );
    assert_eq!(std::fs::read(&dest).unwrap(), b"%PDF!");
    let _ = std::fs::remove_dir_all(out_dir);
}

// ---- browser_network --------------------------------------------------------

#[test]
fn block_and_unblock_maintain_the_blocked_url_list() {
    let (mut server, calls) = server(
        ScriptBuilder::new()
            .call("Network.enable", json!({}))
            .call("Network.setBlockedURLs", json!({}))
            .call("Network.setBlockedURLs", json!({}))
            .call("Network.setBlockedURLs", json!({}))
            .call("Network.setBlockedURLs", json!({})),
    );
    call(
        &mut server,
        "browser_network",
        json!({"cdp": CDP, "action": "block", "patterns": ["*.ads.*"]}),
    );
    let out = call(
        &mut server,
        "browser_network",
        json!({"cdp": CDP, "action": "block", "patterns": ["*.track.*", "*.ads.*"]}),
    );
    assert_eq!(out["blocked"], json!(["*.ads.*", "*.track.*"]));
    call(
        &mut server,
        "browser_network",
        json!({"cdp": CDP, "action": "unblock", "patterns": ["*.ads.*"]}),
    );
    call(
        &mut server,
        "browser_network",
        json!({"cdp": CDP, "action": "unblock"}),
    );
    let sent: Vec<Value> = params_of(&calls, "Network.setBlockedURLs");
    assert_eq!(
        sent,
        [
            json!({"urls": ["*.ads.*"]}),
            json!({"urls": ["*.ads.*", "*.track.*"]}),
            json!({"urls": ["*.track.*"]}),
            json!({"urls": []}),
        ]
    );
    assert_eq!(
        methods(&calls)
            .iter()
            .filter(|m| *m == "Network.enable")
            .count(),
        1
    );
    assert!(!methods(&calls).iter().any(|m| m.starts_with("Fetch.")));
}

fn request(id: &str, url: &str, ts: f64) -> CdpEvent {
    event(
        "Network.requestWillBeSent",
        json!({
            "requestId": id, "timestamp": ts, "wallTime": 1_700_000_000.0 + ts, "type": "Document",
            "request": {"url": url, "method": "GET", "headers": {
                "Cookie": "session=abc", "Authorization": "Bearer s3cret", "Accept": "text/html"
            }}
        }),
    )
}

fn response(id: &str, url: &str, ts: f64) -> CdpEvent {
    event(
        "Network.responseReceived",
        json!({
            "requestId": id, "timestamp": ts,
            "response": {"url": url, "status": 200, "statusText": "OK", "mimeType": "text/html",
                "protocol": "http/1.1", "headers": {"Set-Cookie": "id=1\nother=2", "Content-Type": "text/html"}}
        }),
    )
}

fn finished(id: &str, ts: f64) -> CdpEvent {
    event(
        "Network.loadingFinished",
        json!({"requestId": id, "timestamp": ts, "encodedDataLength": 512}),
    )
}

#[test]
fn har_records_alongside_read_network_and_strips_secrets() {
    let out_dir = scratch("har");
    let har_path = out_dir.join("out.har");
    let mut exec = 0;
    let hook: Hook = Box::new(move |method, _| match method {
        // In flight before recording starts: buffered, but not in the HAR.
        "Network.enable" => vec![request("r0", "http://127.0.0.1/before", 0.5)],
        "Runtime.evaluate" => {
            exec += 1;
            if exec == 1 {
                vec![
                    request("r1", "http://127.0.0.1/a?x=1", 1.0),
                    response("r1", "http://127.0.0.1/a?x=1", 1.25),
                    finished("r1", 1.5),
                ]
            } else {
                vec![
                    request("r2", "http://127.0.0.1/b", 2.0),
                    event(
                        "Network.loadingFailed",
                        json!({"requestId": "r2", "timestamp": 2.1, "errorText": "net::ERR_BLOCKED_BY_CLIENT"}),
                    ),
                ]
            }
        }
        _ => Vec::new(),
    });
    let (mut server, calls) = server_with(
        ScriptBuilder::new()
            .call("Network.enable", json!({}))
            .call("Runtime.evaluate", json!({"result": {"value": 1}}))
            .call("Runtime.enable", json!({}))
            .call("Log.enable", json!({}))
            .call("Runtime.evaluate", json!({"result": {"value": 2}})),
        hook,
    );
    call(
        &mut server,
        "browser_network",
        json!({"cdp": CDP, "action": "har_start"}),
    );
    call(
        &mut server,
        "browser_exec",
        json!({"cdp": CDP, "script": "1"}),
    );
    // read_console takes the buffer and puts the Network events back: the
    // tap must not record them twice.
    let console = call(&mut server, "browser_read_console", json!({"cdp": CDP}));
    assert_eq!(console["entries"], json!([]), "{console}");

    // read_network drains the shared buffer while the HAR records.
    let read = call(&mut server, "browser_read_network", json!({"cdp": CDP}));
    let read_ids: Vec<&str> = read["entries"]
        .as_array()
        .unwrap()
        .iter()
        .map(|e| e["requestId"].as_str().unwrap())
        .collect();
    assert_eq!(read_ids, ["r0", "r1"], "{read}");
    assert_eq!(read["entries"][1]["entry"]["status"], 200);

    call(
        &mut server,
        "browser_exec",
        json!({"cdp": CDP, "script": "2"}),
    );
    let stopped = call(
        &mut server,
        "browser_network",
        json!({"cdp": CDP, "action": "har_stop", "path": har_path.to_str().unwrap()}),
    );
    assert_eq!(stopped["entries"], 2, "{stopped}");
    // read_network enabled nothing again: one shared Network.enable.
    assert_eq!(params_of(&calls, "Network.enable").len(), 1);

    let text = std::fs::read_to_string(&har_path).unwrap();
    assert!(!text.contains("s3cret") && !text.contains("session=abc") && !text.contains("id=1"));
    let har: Value = serde_json::from_str(&text).unwrap();
    assert_eq!(har["log"]["version"], "1.2");
    let entries = har["log"]["entries"].as_array().unwrap();
    let first = &entries[0];
    assert_eq!(first["request"]["url"], "http://127.0.0.1/a?x=1");
    assert_eq!(first["startedDateTime"], "2023-11-14T22:13:21.000Z");
    assert_eq!(
        first["request"]["queryString"],
        json!([{"name": "x", "value": "1"}])
    );
    let header = |list: &Value, name: &str| -> Vec<String> {
        list.as_array()
            .unwrap()
            .iter()
            .filter(|h| h["name"] == name)
            .map(|h| h["value"].as_str().unwrap().to_owned())
            .collect()
    };
    assert_eq!(header(&first["request"]["headers"], "Cookie"), ["•••"]);
    assert_eq!(
        header(&first["request"]["headers"], "Authorization"),
        ["•••"]
    );
    assert_eq!(
        header(&first["request"]["headers"], "Accept"),
        ["text/html"]
    );
    assert_eq!(
        header(&first["response"]["headers"], "Set-Cookie"),
        ["•••", "•••"]
    );
    assert_eq!(first["response"]["status"], 200);
    assert_eq!(first["response"]["httpVersion"], "HTTP/1.1");
    assert_eq!(first["timings"]["wait"], 250.0);
    assert_eq!(first["time"], 500.0);
    assert!(first["response"]["content"].get("text").is_none());
    assert_eq!(
        entries[1]["response"]["_error"],
        "net::ERR_BLOCKED_BY_CLIENT"
    );

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        let mode = std::fs::metadata(&har_path).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o600);
    }

    // read_network still sees the events recorded after its first read.
    let read = call(&mut server, "browser_read_network", json!({"cdp": CDP}));
    assert_eq!(read["entries"][0]["requestId"], "r2", "{read}");
    let _ = std::fs::remove_dir_all(out_dir);
}

#[test]
fn har_stop_without_start_and_onto_a_directory_refuse() {
    let out_dir = scratch("har-refuse");
    let (mut server, _calls) = server(ScriptBuilder::new());
    let err = server
        .call_tool(
            "browser_network",
            &json!({"cdp": CDP, "action": "har_stop", "path": out_dir.join("x.har").to_str().unwrap()}),
        )
        .unwrap_err();
    assert!(matches!(err, ToolError::InvalidArguments(_)), "{err:?}");
    let err = server
        .call_tool(
            "browser_network",
            &json!({"cdp": CDP, "action": "har_stop", "path": out_dir.to_str().unwrap()}),
        )
        .unwrap_err();
    assert!(matches!(err, ToolError::InvalidArguments(_)), "{err:?}");
    let _ = std::fs::remove_dir_all(out_dir);
}

#[test]
fn env_tools_are_listed_with_schemas() {
    let mut server = Server::with_connector(|_| {
        Err(CdpError::Transport {
            message: "no browser".into(),
        })
    });
    let reply = server
        .handle_line(r#"{"jsonrpc":"2.0","id":1,"method":"tools/list"}"#)
        .unwrap();
    let reply: Value = serde_json::from_str(&reply).unwrap();
    let tools = reply["result"]["tools"].as_array().unwrap();
    for name in ["browser_set", "browser_download", "browser_network"] {
        let tool = tools
            .iter()
            .find(|t| t["name"] == name)
            .unwrap_or_else(|| panic!("{name} missing"));
        assert!(tool["inputSchema"]["properties"].get("cdp").is_some());
        assert!(!tool["inputSchema"]["required"]
            .as_array()
            .unwrap()
            .is_empty());
    }
}

/// A [`Wire`] whose `n`-th `method` call times out (typed) instead.
struct TimesOutOn {
    wire: Wire,
    method: &'static str,
    n: usize,
}

impl CdpTransport for TimesOutOn {
    fn call(&mut self, method: &str, params_json: &str) -> Result<String, CdpError> {
        if method == self.method {
            self.n -= 1;
            if self.n == 0 {
                return Err(CdpError::Timeout {
                    method: method.to_owned(),
                    after: std::time::Duration::from_secs(30),
                });
            }
        }
        self.wire.call(method, params_json)
    }

    fn drain_events(&mut self) -> Vec<CdpEvent> {
        self.wire.drain_events()
    }
}

/// The download fails on its own (canceled) and the best-effort restore
/// after it times out: the call reports the download's own error, not the
/// restore's typed timeout.
#[test]
fn a_failed_download_reports_its_own_error_when_the_restore_times_out() {
    let out_dir = scratch("download-canceled");
    let dest = out_dir.join("never.pdf");
    let script = ScriptBuilder::new()
        .call("Browser.setDownloadBehavior", json!({}))
        .observe_compact(&page())
        .observe_compact(&page())
        .pointer_click()
        .call("Browser.getVersion", json!({}));
    let hook: Hook = Box::new(|method, params| match method {
        "Input.dispatchMouseEvent" if params["type"] == "mouseReleased" => vec![event(
            "Browser.downloadWillBegin",
            json!({"guid": "g-1", "url": "http://127.0.0.1/r.pdf", "suggestedFilename": "r.pdf"}),
        )],
        "Browser.getVersion" => vec![event(
            "Browser.downloadProgress",
            json!({"guid": "g-1", "state": "canceled"}),
        )],
        _ => Vec::new(),
    });
    let mut parts = Some((script, hook));
    let mut server = Server::with_connector(move |_url| {
        let (script, hook) = parts.take().ok_or_else(|| CdpError::Transport {
            message: "one connect per test".into(),
        })?;
        Ok(Box::new(TimesOutOn {
            wire: Wire {
                inner: ReplayTransport::parse(&script.to_json())?,
                calls: Rc::default(),
                pending: Vec::new(),
                hook,
            },
            method: "Browser.setDownloadBehavior",
            n: 2,
        }) as Box<dyn CdpTransport>)
    });
    let err = server
        .call_tool(
            "browser_download",
            &json!({"cdp": CDP, "index": 2, "path": dest.to_str().unwrap(), "timeout_ms": 200}),
        )
        .unwrap_err();
    assert!(
        matches!(&err, ToolError::Browser(m) if m == "download was canceled"),
        "{err:?}"
    );
    assert_eq!(err.code(), "browser");
    let _ = std::fs::remove_dir_all(out_dir);
}
