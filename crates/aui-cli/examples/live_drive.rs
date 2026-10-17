//! Manual, opt-in live drive: JEV (System One) chooses ultra-instinct MCP calls
//! against a throwaway local Chrome. Not CI, not a benchmark.
//!
//! JEV answers named choice questions about a JSON state. It does not run a
//! tool loop of its own, so this harness is the loop: each step it asks JEV
//! which tool to call, then asks for that tool's arguments as choices built
//! from the task text and the last observation, runs the call on a
//! `ultra-instinct mcp` child, and appends the reply to the state.
//!
//! Product tools are observe / guard / verify. `guard` decides Allow or Refuse
//! and never clicks. This harness owns the click: after `GuardDecision::Allow`
//! it inspects the allowed region for geometry and presses the center through
//! its own CDP connection (`Input.dispatchMouseEvent`). A Refuse is journaled
//! and does not click. Deprecated `act` is not offered.
//!
//! Caller-side rules the harness enforces (the server enforces the rest):
//! - guard takes locate-style text/role/position (and optional proposed /
//!   seen_snapshot); expectation text for verify can only be a string quoted
//!   in the task.
//!
//! Each task gets a fresh tab (`PUT /json/new`) and a fresh `ultra-instinct mcp`
//! process on that tab's page websocket. After the task the harness reads the
//! page's real URL, title, and snackbar text through its own CDP connection, so the
//! transcript has ground truth next to what ultra-instinct reported.
//!
//! Bench mode (`--task-json`, used by `bench/arms/a5_jev_ultra_instinct.py`): run one task
//! given as JSON `{id, start_url, text}`, stream every event to `--trace` as
//! JSONL while it happens (the bench harness enforces caps from outside), and
//! stop after `--max-steps` steps. The built-in Acme Mail tasks are unchanged.
//!
//! Run (see examples/live-drive/README.md):
//! `ULTRA_INSTINCT_JEV=1 TYPESAFE_API_KEY=... cargo run --release -p aui-cli
//!  --features "jev,hgra" --example live_drive -- --bin target/release/ultra-instinct`

use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpStream;
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};
use std::time::{Duration, Instant};

use aui_browser::{CdpTransport, WebSocketTransport};
use aui_remote::typesafe_sdk::{self, JsonContent, Question};
use serde_json::{json, Value};

const MAX_STEPS: usize = 14;

/// After this many consecutive identical ambiguous guard refuses, inject
/// `within` / `near` choices into the next locate/guard JEV prompts (same
/// spirit as MCP `REPEAT_THRESHOLD` for locate). The gate still refuses until
/// the query is scoped — this only widens the option set the harness offers.
const AMBIGUOUS_SCOPE_THRESHOLD: usize = 2;

struct Task {
    id: &'static str,
    /// Page the fresh tab opens, relative to the site root.
    start: &'static str,
    text: &'static str,
    /// What the page should show afterwards. Recorded, not scored.
    expect: &'static str,
}

const TASKS: [Task; 8] = [
    Task {
        id: "t1-compose-send",
        start: "index.html",
        text: r#"Open "Compose", then click the "Send" button in the compose window and verify "Message sent" appears."#,
        expect: "snackbar Message sent",
    },
    Task {
        id: "t2-nav-settings",
        start: "index.html",
        text: r#"Open "Settings" from the left navigation and confirm the page changed."#,
        expect: "title Settings - Acme Mail",
    },
    Task {
        id: "t3-reveal-cc",
        start: "index.html",
        text: r#"Open "Compose", click "Add Cc" to reveal the "Cc recipients" field, and verify it appeared."#,
        expect: "cc field visible",
    },
    Task {
        id: "t4-enabled-save",
        start: "settings.html",
        text: r#"Click the enabled "Save" button (the Signature one), not the disabled Save, and verify "Settings saved" appears."#,
        expect: "snackbar Settings saved",
    },
    Task {
        id: "t5-help-link",
        start: "index.html",
        text: r#"Open "Help" from the top bar and verify the URL changed."#,
        expect: "title Help - Acme Mail",
    },
    Task {
        id: "t6-thread-archive",
        // Thread toolbar Archive is only on #thread/<id>; start already there.
        start: "index.html#thread/q3",
        text: r#"On the open "Q3 launch checklist" thread, click the visible "Archive" button in its toolbar and verify "Conversation archived" appears."#,
        expect: "snackbar Conversation archived",
    },
    Task {
        id: "t7-thread-reply",
        start: "index.html#thread/q3",
        text: r#"On the open "Q3 launch checklist" thread, click "Send" in the quick reply box and verify "Reply sent" appears."#,
        expect: "snackbar Reply sent, url #thread/q3",
    },
    Task {
        id: "t8-twin-send",
        // preset=twin opens Compose + fills quick reply so both Send buttons are live.
        start: "index.html?preset=twin#thread/q3",
        text: r#"On the open "Q3 launch checklist" thread with Compose already open, click "Send"."#,
        expect: "unspecified (twin probe: compose Send vs quick-reply Send both visible)",
    },
];

struct Args {
    bin: PathBuf,
    site: String,
    cdp: String,
    out: PathBuf,
    only: Option<String>,
    screenshot_only: bool,
    task_json: Option<PathBuf>,
    trace: Option<PathBuf>,
    max_steps: usize,
}

fn parse_args() -> Args {
    let mut args = Args {
        bin: PathBuf::from("target/release/ultra-instinct"),
        site: "http://127.0.0.1:8765".into(),
        cdp: "http://127.0.0.1:9333".into(),
        out: PathBuf::from("/tmp/aui-live"),
        only: None,
        screenshot_only: false,
        task_json: None,
        trace: None,
        max_steps: MAX_STEPS,
    };
    let mut it = std::env::args().skip(1);
    while let Some(flag) = it.next() {
        let mut value = || it.next().unwrap_or_else(|| panic!("{flag} needs a value"));
        match flag.as_str() {
            "--bin" => args.bin = PathBuf::from(value()),
            "--site" => args.site = value(),
            "--cdp" => args.cdp = value(),
            "--out" => args.out = PathBuf::from(value()),
            "--only" => args.only = Some(value()),
            "--screenshot-only" => args.screenshot_only = true,
            "--task-json" => args.task_json = Some(PathBuf::from(value())),
            "--trace" => args.trace = Some(PathBuf::from(value())),
            "--max-steps" => args.max_steps = value().parse().expect("--max-steps takes a number"),
            other => panic!("unknown flag {other}"),
        }
    }
    for url in [&args.site, &args.cdp] {
        assert!(
            url.starts_with("http://127.0.0.1:"),
            "{url}: only 127.0.0.1 is allowed"
        );
    }
    args
}

// ---------- Chrome HTTP endpoint and ground truth ----------

fn http(method: &str, base: &str, path: &str) -> String {
    let host = base.trim_start_matches("http://");
    let mut stream = TcpStream::connect(host).expect("connect to Chrome");
    stream
        .set_read_timeout(Some(Duration::from_secs(10)))
        .unwrap();
    write!(
        stream,
        "{method} {path} HTTP/1.1\r\nHost: {host}\r\nConnection: close\r\n\r\n"
    )
    .unwrap();
    // Chrome's devtools HTTP server may keep the socket open, so read by
    // Content-Length instead of to EOF.
    let mut raw = Vec::new();
    let mut chunk = [0u8; 4096];
    loop {
        let n = match stream.read(&mut chunk) {
            Ok(0) | Err(_) => break,
            Ok(n) => n,
        };
        raw.extend_from_slice(&chunk[..n]);
        let text = String::from_utf8_lossy(&raw);
        if let Some((head, body)) = text.split_once("\r\n\r\n") {
            let length = head.lines().find_map(|line| {
                let (name, value) = line.split_once(':')?;
                if name.eq_ignore_ascii_case("content-length") {
                    value.trim().parse::<usize>().ok()
                } else {
                    None
                }
            });
            if length.is_some_and(|length| body.len() >= length) {
                break;
            }
        }
    }
    let text = String::from_utf8_lossy(&raw).into_owned();
    text.split_once("\r\n\r\n")
        .map(|(_, body)| body.to_owned())
        .unwrap_or_default()
}

struct Tab {
    id: String,
    ws: String,
}

fn open_tab(cdp: &str, url: &str) -> Tab {
    // `/json/new?{url}` cannot carry a `#fragment` in the HTTP request-target.
    // Open a blank tab, then `Page.navigate` to the full URL (query + hash) so
    // thread starts (`#thread/q3`) and `?preset=twin` apply in document order.
    let body = http("PUT", cdp, "/json/new?about:blank");
    let value: Value = serde_json::from_str(&body).expect("json/new");
    let tab = Tab {
        id: value["id"].as_str().unwrap().to_owned(),
        ws: value["webSocketDebuggerUrl"].as_str().unwrap().to_owned(),
    };
    let mut socket = WebSocketTransport::connect(&tab.ws).expect("cdp for navigate");
    let nav = json!({"url": url});
    socket
        .call("Page.navigate", &nav.to_string())
        .expect("Page.navigate");
    // Let the page load (and hash route / preset) before the first observe.
    std::thread::sleep(Duration::from_millis(1400));
    tab
}

fn close_tab(cdp: &str, tab: &Tab) {
    let _ = http("GET", cdp, &format!("/json/close/{}", tab.id));
}

const TRUTH_JS: &str = "JSON.stringify({url: location.href, title: document.title, \
    snackbar: (document.querySelector('#snackbar:not(.hidden)') || {getAttribute: () => ''}).getAttribute('aria-label') || '', \
    thread_open: !!document.querySelector('#thread-view:not(.hidden)'), compose_open: !!document.querySelector('#compose:not(.hidden)'), \
    cc_visible: !!document.querySelector('#cc-line:not(.hidden)'), \
    focused: (document.activeElement || {}).id || ''})";

fn ground_truth(tab: &Tab) -> Value {
    let mut socket = match WebSocketTransport::connect(&tab.ws) {
        Ok(socket) => socket,
        Err(err) => return json!({"error": err.to_string()}),
    };
    let params = json!({"expression": TRUTH_JS, "returnByValue": true}).to_string();
    match socket.call("Runtime.evaluate", &params) {
        Ok(raw) => {
            let value: Value = serde_json::from_str(&raw).unwrap_or(Value::Null);
            value["result"]["value"]
                .as_str()
                .and_then(|text| serde_json::from_str(text).ok())
                .unwrap_or(value)
        }
        Err(err) => json!({"error": err.to_string()}),
    }
}

/// Harness-owned click after guard Allow. Uses the tab's page websocket, not MCP.
fn press_center(tab: &Tab, x: f64, y: f64, width: f64, height: f64) -> Result<(), String> {
    let mut socket = WebSocketTransport::connect(&tab.ws).map_err(|e| e.to_string())?;
    let cx = x + width / 2.0;
    let cy = y + height / 2.0;
    for kind in ["mousePressed", "mouseReleased"] {
        let params = json!({
            "type": kind,
            "x": cx,
            "y": cy,
            "button": "left",
            "clickCount": 1
        })
        .to_string();
        socket
            .call("Input.dispatchMouseEvent", &params)
            .map_err(|e| e.to_string())?;
    }
    // Let the page react before the next observe/verify.
    std::thread::sleep(Duration::from_millis(350));
    Ok(())
}

/// Screenshots at each width, then one with Compose open. Uses a device
/// metrics override on this tab only.
fn screenshots(tab: &Tab, out: &Path) -> Result<Vec<PathBuf>, String> {
    let mut socket = WebSocketTransport::connect(&tab.ws).map_err(|e| e.to_string())?;
    let mut paths = Vec::new();
    let mut shot = |socket: &mut WebSocketTransport, name: &str| -> Result<(), String> {
        let raw = socket
            .call("Page.captureScreenshot", r#"{"format":"png"}"#)
            .map_err(|e| e.to_string())?;
        let value: Value = serde_json::from_str(&raw).map_err(|e| e.to_string())?;
        let data = value["data"].as_str().ok_or("no data")?;
        let path = out.join(name);
        std::fs::write(&path, base64_decode(data)?).map_err(|e| e.to_string())?;
        paths.push(path);
        Ok(())
    };
    for width in [1280, 1440] {
        let metrics =
            json!({"width": width, "height": 860, "deviceScaleFactor": 1, "mobile": false});
        socket
            .call("Emulation.setDeviceMetricsOverride", &metrics.to_string())
            .map_err(|e| e.to_string())?;
        std::thread::sleep(Duration::from_millis(400));
        shot(&mut socket, &format!("screenshot-inbox-{width}.png"))?;
        for (hash, name) in [("#thread/q3", "thread"), ("#inbox", "")] {
            let go = json!({"expression": format!("location.hash = '{hash}'; true")});
            socket
                .call("Runtime.evaluate", &go.to_string())
                .map_err(|e| e.to_string())?;
            std::thread::sleep(Duration::from_millis(300));
            if !name.is_empty() {
                shot(&mut socket, &format!("screenshot-{name}-{width}.png"))?;
            }
        }
    }
    let open = json!({"expression": "location.hash = '#thread/q3'; document.getElementById('compose-open').click(); document.getElementById('add-cc').click(); true"});
    socket
        .call("Runtime.evaluate", &open.to_string())
        .map_err(|e| e.to_string())?;
    std::thread::sleep(Duration::from_millis(300));
    shot(&mut socket, "screenshot-compose-1440.png")?;
    for page in ["settings.html", "help.html"] {
        let nav = json!({"expression": format!("location.href = '{page}'; true")});
        socket
            .call("Runtime.evaluate", &nav.to_string())
            .map_err(|e| e.to_string())?;
        std::thread::sleep(Duration::from_millis(900));
        shot(
            &mut socket,
            &format!("screenshot-{}-1440.png", page.trim_end_matches(".html")),
        )?;
    }
    socket
        .call("Emulation.clearDeviceMetricsOverride", "{}")
        .map_err(|e| e.to_string())?;
    Ok(paths)
}

fn base64_decode(input: &str) -> Result<Vec<u8>, String> {
    let mut out = Vec::with_capacity(input.len() * 3 / 4);
    let (mut buf, mut bits) = (0u32, 0u32);
    for byte in input.bytes() {
        let v = match byte {
            b'A'..=b'Z' => byte - b'A',
            b'a'..=b'z' => byte - b'a' + 26,
            b'0'..=b'9' => byte - b'0' + 52,
            b'+' => 62,
            b'/' => 63,
            b'=' | b'\n' | b'\r' => continue,
            other => return Err(format!("bad base64 byte {other}")),
        };
        buf = (buf << 6) | u32::from(v);
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((buf >> bits) as u8);
            buf &= (1 << bits) - 1;
        }
    }
    Ok(out)
}

// ---------- MCP child ----------

struct Mcp {
    child: Child,
    stdin: ChildStdin,
    stdout: BufReader<ChildStdout>,
    next_id: u64,
}

impl Mcp {
    fn spawn(bin: &Path) -> Self {
        let mut child = Command::new(bin)
            .arg("mcp")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .expect("spawn ultra-instinct mcp");
        let stdin = child.stdin.take().unwrap();
        let stdout = BufReader::new(child.stdout.take().unwrap());
        Self {
            child,
            stdin,
            stdout,
            next_id: 1,
        }
    }

    fn request(&mut self, method: &str, params: Value) -> Value {
        let id = self.next_id;
        self.next_id += 1;
        let line = json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params});
        writeln!(self.stdin, "{line}").unwrap();
        self.stdin.flush().unwrap();
        let mut reply = String::new();
        self.stdout.read_line(&mut reply).unwrap();
        serde_json::from_str(&reply).unwrap_or_else(|_| json!({"raw": reply}))
    }

    fn tool(&mut self, name: &str, arguments: &Value) -> (bool, Value) {
        let reply = self.request("tools/call", json!({"name": name, "arguments": arguments}));
        let result = &reply["result"];
        let text = result["content"][0]["text"].as_str().unwrap_or("null");
        (
            result["isError"] == true,
            serde_json::from_str(text).unwrap_or(Value::Null),
        )
    }

    fn close(mut self) {
        drop(self.stdin);
        let _ = self.child.wait();
    }
}

// ---------- JEV ----------

struct Jev {
    client: typesafe_sdk::blocking::Client,
}

struct Pick {
    choice: String,
    confidence: f64,
    latency_ms: u128,
}

impl Jev {
    fn ask(
        &self,
        state: &Value,
        name: &str,
        prompt: String,
        options: &[(String, String)],
    ) -> Result<Pick, String> {
        let criteria = options
            .iter()
            .map(|(label, about)| (label.clone(), Some(JsonContent::from(about.clone()))));
        let started = Instant::now();
        let response = self
            .client
            .system_one(state.clone(), [(name, Question::choice(prompt, criteria))])
            .map_err(|err| err.to_string())?;
        let answer = response.choice(name).map_err(|err| err.to_string())?;
        Ok(Pick {
            choice: answer.choice.clone(),
            confidence: answer.confidence,
            latency_ms: started.elapsed().as_millis(),
        })
    }
}

// ---------- the loop ----------

fn quoted(text: &str) -> Vec<String> {
    text.split('"')
        .skip(1)
        .step_by(2)
        .map(str::to_owned)
        .collect()
}

/// `(zone, candidate id)` pairs from a `repeated_query` signal on the last
/// locate, for the candidates that have a separating zone.
/// Position zone that produced the last low-confidence refuse, if any.
fn low_confidence_position(
    last_guard: Option<&Value>,
    last_guard_args: Option<&Value>,
) -> Option<String> {
    let guard = last_guard?;
    let reason = guard["reason"]
        .as_str()
        .or_else(|| guard["fallback"].as_str())?;
    if reason != "low-confidence" {
        return None;
    }
    last_guard_args?
        .get("position")
        .and_then(|v| v.as_str())
        .map(str::to_owned)
}

/// JEV choice: (value offered to the model, short about text).
type JevChoice = (String, String);

#[derive(Clone, Debug, PartialEq, Eq)]
struct AmbiguousStreak {
    key: String,
    count: usize,
}

fn guard_is_ambiguous(guard: &Value) -> bool {
    matches!(
        guard["reason"]
            .as_str()
            .or_else(|| guard["fallback"].as_str()),
        Some("ambiguous")
    )
}

/// Fingerprint for "identical" ambiguous refuses: top twin pair when present
/// (order-insensitive), else text/role/proposed from the call args.
fn ambiguous_refuse_key(guard: &Value, args: &Value) -> Option<String> {
    if !guard_is_ambiguous(guard) {
        return None;
    }
    let twin: Vec<&str> = guard["candidates"]
        .as_array()
        .into_iter()
        .flatten()
        .take(2)
        .filter_map(|c| c["id"].as_str())
        .collect();
    if twin.len() == 2 {
        let (a, b) = if twin[0] <= twin[1] {
            (twin[0], twin[1])
        } else {
            (twin[1], twin[0])
        };
        return Some(format!("twins:{a}|{b}"));
    }
    let text = args.get("text").and_then(|v| v.as_str()).unwrap_or("");
    let role = args.get("role").and_then(|v| v.as_str()).unwrap_or("");
    let proposed = args.get("proposed").and_then(|v| v.as_str()).unwrap_or("");
    Some(format!("query:{text}|{role}|{proposed}"))
}

fn note_ambiguous_refuse(streak: &mut Option<AmbiguousStreak>, guard: &Value, args: &Value) {
    match ambiguous_refuse_key(guard, args) {
        Some(key) => match streak {
            Some(s) if s.key == key => s.count = s.count.saturating_add(1),
            _ => *streak = Some(AmbiguousStreak { key, count: 1 }),
        },
        None => *streak = None,
    }
}

fn prefers_compose_scope(label: &str, modal: bool) -> bool {
    let lower = label.to_ascii_lowercase();
    modal || lower.contains("new message") || lower.contains("compose")
}

/// `within` / `near` option pairs for the next locate/guard after the ambiguous
/// streak hits [`AMBIGUOUS_SCOPE_THRESHOLD`]. Prefers front_layer Compose /
/// "New Message", then twin candidate parents from the observed regions.
fn ambiguous_scope_options(guard: &Value, regions: &[Value]) -> (Vec<JevChoice>, Vec<JevChoice>) {
    let mut within = vec![(
        "none".into(),
        "omit within (twins stay tied if both still match)".into(),
    )];
    let mut near = vec![
        ("none".into(), "omit near (default ranking)".into()),
        ("focus".into(), "near the focused region (cursor)".into()),
    ];
    let push_unique = |opts: &mut Vec<JevChoice>, id: &str, about: String| {
        if id.is_empty() || opts.iter().any(|(existing, _)| existing == id) {
            return;
        }
        opts.push((id.to_owned(), about));
    };

    let mut preferred_within: Vec<JevChoice> = Vec::new();
    let mut other_within: Vec<JevChoice> = Vec::new();
    if let Some(layer) = guard["front_layer"].as_array() {
        for entry in layer {
            let id = entry["id"].as_str().unwrap_or("");
            let label = entry["label"].as_str().unwrap_or("");
            let modal = entry["modal"].as_bool().unwrap_or(false);
            if id.is_empty() {
                continue;
            }
            let about = format!(
                "within front_layer \"{label}\" ({id}) — scopes the query to that container"
            );
            if prefers_compose_scope(label, modal) {
                preferred_within.push((id.to_owned(), about.clone()));
            } else {
                other_within.push((id.to_owned(), about));
            }
            let near_about = format!("near front_layer \"{label}\" ({id})");
            push_unique(&mut near, id, near_about);
        }
    }
    for (id, about) in preferred_within.into_iter().chain(other_within) {
        push_unique(&mut within, &id, about);
    }

    if let Some(cands) = guard["candidates"].as_array() {
        for cand in cands.iter().take(2) {
            let cid = cand["id"].as_str().unwrap_or("");
            let Some(region) = regions.iter().find(|r| r["id"].as_str() == Some(cid)) else {
                continue;
            };
            let Some(parent) = region["parent"].as_str() else {
                continue;
            };
            let parent_label = regions
                .iter()
                .find(|r| r["id"].as_str() == Some(parent))
                .and_then(|r| r["label"].as_str())
                .unwrap_or(parent);
            push_unique(
                &mut within,
                parent,
                format!("within parent \"{parent_label}\" of twin candidate {cid}"),
            );
            push_unique(
                &mut near,
                parent,
                format!("near parent \"{parent_label}\" of twin candidate {cid}"),
            );
            push_unique(
                &mut near,
                cid,
                format!(
                    "near twin candidate {cid} (\"{}\")",
                    cand["label"].as_str().unwrap_or("")
                ),
            );
        }
    }

    (within, near)
}

fn scope_nudge_active(streak: &Option<AmbiguousStreak>) -> bool {
    streak
        .as_ref()
        .is_some_and(|s| s.count >= AMBIGUOUS_SCOPE_THRESHOLD)
}

fn position_options(
    zones: &[&str],
    suggested: &[(String, String)],
    failed_pos: Option<&str>,
) -> Vec<(String, String)> {
    zones
        .iter()
        .map(|z| {
            let mut about = match suggested.iter().find(|(zone, _)| zone == z) {
                Some((_, id)) => format!("position {z} (repeated_query suggests it to pick {id})"),
                None => format!("position {z}"),
            };
            if *z == "none" {
                if let Some(bad) = failed_pos {
                    about = format!(
                        "none (last guard refused low-confidence with position {bad}; clear the zone)"
                    );
                } else {
                    about = "none (omit zone constraint; safest default)".into();
                }
            } else if failed_pos == Some(*z) {
                about = format!("{about} (this zone just refused as low-confidence)");
            }
            (z.to_string(), about)
        })
        .collect()
}

fn repeated_query_hints(last_locate: Option<&Value>) -> Vec<(String, String)> {
    let Some(signals) = last_locate.and_then(|located| located["signals"].as_array()) else {
        return Vec::new();
    };
    signals
        .iter()
        .filter(|signal| signal["kind"] == "repeated_query")
        .flat_map(|signal| [&signal["top"], &signal["runner_up"]])
        .filter_map(|hint| {
            Some((
                hint["suggested_position"].as_str()?.to_owned(),
                hint["id"].as_str()?.to_owned(),
            ))
        })
        .collect()
}

fn summarize(tool: &str, is_error: bool, body: &Value) -> Value {
    if is_error {
        return json!({"error": body});
    }
    match tool {
        "observe" => {
            json!({"snapshot": body["snapshot"], "regions": body["regions"].as_array().map_or(0, Vec::len)})
        }
        "locate" => json!({
            "target": body["target"]["id"],
            "candidates": body["candidates"].as_array().map(|c| c.iter().take(3).map(|x| json!({"id": x["id"], "label": x["label"], "state": x["state"], "confidence": x["confidence"]})).collect::<Vec<_>>()),
            "signals": body["signals"],
        }),
        "inspect" => body["target"].clone(),
        "guard" => json!({
            "decision": body["decision"],
            "reason": body["reason"],
            "fallback": body["fallback"],
            "target": body["target"],
            "confidence": body["confidence"],
            "candidates": body["candidates"].as_array().map(|c| c.iter().take(3).cloned().collect::<Vec<_>>()),
            "clicked": body["clicked"],
            "click_error": body["click_error"],
        }),
        "verify" | "diff" => {
            json!({"verified": body["verified"], "state_delta": body["state_delta"]})
        }
        _ => body.clone(),
    }
}

struct Log {
    lines: Vec<Value>,
    started: Instant,
    /// Bench mode: every event is also appended here as it happens.
    sink: Option<std::fs::File>,
}

impl Log {
    fn push(&mut self, mut event: Value) {
        event["t_ms"] = json!(self.started.elapsed().as_millis());
        if let Some(sink) = self.sink.as_mut() {
            let _ = writeln!(sink, "{event}");
            let _ = sink.flush();
        }
        self.lines.push(event);
    }
}

fn run_task(args: &Args, jev: &Jev, task: &Task, tools: &Value) -> Value {
    let started = Instant::now();
    let mut log = Log {
        lines: Vec::new(),
        started,
        sink: args
            .trace
            .as_ref()
            .map(|path| std::fs::File::create(path).expect("create --trace file")),
    };
    let start = if task.start.starts_with("http://") {
        task.start.to_owned()
    } else {
        format!("{}/{}", args.site, task.start)
    };
    let tab = open_tab(&args.cdp, &start);
    let mut mcp = Mcp::spawn(&args.bin);
    let init = mcp.request(
        "initialize",
        json!({"protocolVersion": "2024-11-05", "capabilities": {}, "clientInfo": {"name": "jev-live-drive", "version": "0"}}),
    );
    log.push(json!({"kind": "mcp-init", "server": init["result"]["serverInfo"]}));

    let mentions = quoted(task.text);
    let mut regions: Vec<Value> = Vec::new();
    let mut inspected: Vec<String> = Vec::new();
    let mut last_locate: Option<Value> = None;
    let mut last_snapshot: Option<u64> = None;
    let mut last_guard: Option<Value> = None;
    let mut last_guard_args: Option<Value> = None;
    let mut last_diff_pair: Option<(u64, u64)> = None;
    let mut history: Vec<Value> = Vec::new();
    let (mut tool_calls, mut jev_calls) = (0usize, 0usize);
    let mut clicks_executed = 0usize;
    let mut outcome = "max-steps".to_owned();
    let mut ambiguous_streak: Option<AmbiguousStreak> = None;

    for step in 1..=args.max_steps {
        let nudge = scope_nudge_active(&ambiguous_streak);
        let mut guidance = vec![
            "observe lists the page regions; locate ranks regions for a text, role, and position query".into(),
            "guard decides Allow or Refuse for a click intent (text/role/position); it never clicks".into(),
            "after Allow this harness clicks the allowed region via CDP; after Refuse do not click — change the query and try again".into(),
            "verify checks that a quoted string is present on the page after a click".into(),
            "locate signals repeated_query means this exact query already ran on this unchanged page and will return the same ranking; use a suggested_position it names, or change the text or role".into(),
            "position is optional: choose none unless you know the target's zone; a wrong zone (for example top on a bottom compose-sheet control) drops confidence and can refuse as low-confidence even when the label ranks first".into(),
            "when the last guard refused low-confidence and the query had a position, retry with position none before changing the text".into(),
            "each region has a state: availability enabled or disabled, visibility visible, occluded, offscreen, or hidden".into(),
            "thread toolbar Archive and quick-reply Send exist only when the thread view is open; do not guard inbox row links or occluded twins to open a thread — this task already starts on the right view when needed".into(),
            "when two Send buttons are visible (Compose and quick reply), prefer the Compose sheet Send unless the task names the quick reply box".into(),
            "choose done when the task's check has passed, or give up when it cannot pass".into(),
        ];
        if nudge {
            let count = ambiguous_streak.as_ref().map(|s| s.count).unwrap_or(0);
            guidance.push(format!(
                "last guard refused ambiguous {count} times with the same twin/query; the next locate/guard offers within/near — pick within the front_layer Compose/\"New Message\" (or near it) so the twins separate; bare text will keep refusing"
            ));
        }
        let state = json!({
            "role": "You drive ultra-instinct, a browser coprocessor, through MCP tools to do one task. Choose the next call. ultra-instinct never navigates and never guesses coordinates.",
            "task": task.text,
            "tools": tools,
            "guidance": guidance,
            "page_regions": regions,
            "last_locate": last_locate,
            "last_guard": last_guard,
            "history": history,
            "step": step,
            "ambiguous_scope_nudge": nudge,
        });
        let guard_about = if nudge {
            "decide Allow/Refuse for a click; on Allow this harness presses via CDP — after repeated ambiguous, pass within/near to scope (Compose/New Message)"
        } else {
            "decide Allow/Refuse for a click; on Allow this harness presses via CDP"
        };
        let locate_about = if nudge {
            "rank regions for a text/role/position query (preview; does not click) — after repeated ambiguous, pass within/near to scope"
        } else {
            "rank regions for a text/role/position query (preview; does not click)"
        };
        let mut tool_options: Vec<(String, String)> = vec![
            ("observe".into(), "list the regions on the page".into()),
            ("locate".into(), locate_about.into()),
            ("guard".into(), guard_about.into()),
        ];
        if !regions.is_empty() {
            tool_options.push((
                "inspect".into(),
                "read one region's role, label, and box".into(),
            ));
        }
        tool_options.push((
            "verify".into(),
            "check that a quoted text is on the page now".into(),
        ));
        if last_diff_pair.is_some() {
            tool_options.push((
                "diff".into(),
                "diff the snapshots before and after the last harness click".into(),
            ));
        }
        tool_options.push(("done".into(), "stop: the task's check passed".into()));
        tool_options.push((
            "give_up".into(),
            "stop: the task cannot be completed".into(),
        ));

        let pick = match jev.ask(
            &state,
            "next_tool",
            "Which call should be made next for this task?".into(),
            &tool_options,
        ) {
            Ok(pick) => pick,
            Err(err) => {
                log.push(json!({"kind": "jev-error", "step": step, "error": err}));
                outcome = "jev-error".into();
                break;
            }
        };
        jev_calls += 1;
        log.push(json!({"kind": "jev", "step": step, "question": "next_tool", "options": tool_options.iter().map(|o| &o.0).collect::<Vec<_>>(), "choice": pick.choice, "confidence": pick.confidence, "latency_ms": pick.latency_ms}));
        let tool = pick.choice.clone();
        if tool == "done" || tool == "give_up" {
            outcome = tool;
            break;
        }

        // Arguments, as choices.
        let mut ask_arg = |name: &str,
                           prompt: &str,
                           options: Vec<(String, String)>|
         -> Option<String> {
            if options.is_empty() {
                return None;
            }
            match jev.ask(
                &state,
                name,
                format!("{prompt} (for the `{tool}` call)"),
                &options,
            ) {
                Ok(pick) => {
                    jev_calls += 1;
                    log.push(json!({"kind": "jev", "step": step, "question": name, "options": options.iter().map(|o| &o.0).collect::<Vec<_>>(), "choice": pick.choice, "confidence": pick.confidence, "latency_ms": pick.latency_ms}));
                    Some(pick.choice)
                }
                Err(err) => {
                    log.push(
                        json!({"kind": "jev-error", "step": step, "question": name, "error": err}),
                    );
                    None
                }
            }
        };
        let region_options = |filter: &dyn Fn(&str) -> bool| -> Vec<(String, String)> {
            regions
                .iter()
                .filter(|r| filter(r["id"].as_str().unwrap_or("")))
                .map(|r| {
                    (
                        r["id"].as_str().unwrap_or("").to_owned(),
                        format!(
                            "{} \"{}\"",
                            r["role"].as_str().unwrap_or(""),
                            r["label"].as_str().unwrap_or("")
                        ),
                    )
                })
                .collect()
        };
        let mut texts: Vec<String> = mentions.clone();
        for region in &regions {
            if let Some(label) = region["label"].as_str() {
                if !label.is_empty() && !texts.iter().any(|t| t == label) {
                    texts.push(label.to_owned());
                }
            }
        }
        let text_options: Vec<(String, String)> = texts
            .iter()
            .map(|t| (t.clone(), format!("search for \"{t}\"")))
            .collect();
        let expect_options: Vec<(String, String)> = mentions
            .iter()
            .map(|t| (t.clone(), format!("expect \"{t}\" on the page after")))
            .collect();

        let mut arguments = json!({"cdp": tab.ws});
        let mut skip = None;
        match tool.as_str() {
            "observe" => {}
            "locate" => {
                match ask_arg("text", "Which text should locate search for?", text_options) {
                    Some(text) => arguments["text"] = json!(text),
                    None => skip = Some("no locate text"),
                }
                let roles = [
                    "any",
                    "button",
                    "link",
                    "text_field",
                    "heading",
                    "navigation",
                ];
                if let Some(role) = ask_arg(
                    "role",
                    "Which role should the target have?",
                    roles
                        .iter()
                        .map(|r| (r.to_string(), format!("role {r}")))
                        .collect(),
                ) {
                    if role != "any" {
                        arguments["role"] = json!(role);
                    }
                }
                let zones = ["none", "left", "right", "top", "bottom", "center"];
                // Surface a repeated_query signal from the last locate: name
                // the candidate each suggested position would favour. Option
                // order does not change.
                let suggested = repeated_query_hints(last_locate.as_ref());
                let failed_pos =
                    low_confidence_position(last_guard.as_ref(), last_guard_args.as_ref());
                if let Some(zone) = ask_arg(
                    "position",
                    "Where on the page is the target? Prefer none unless sure.",
                    position_options(&zones, &suggested, failed_pos.as_deref()),
                ) {
                    if zone != "none" {
                        arguments["position"] = json!(zone);
                    }
                }

                // Harness nudge: after N identical ambiguous refuses, offer
                // within/near so JEV can scope. Guard still refuses until scoped.
                if nudge {
                    if let Some(guard) = last_guard.as_ref() {
                        let (within_opts, near_opts) = ambiguous_scope_options(guard, &regions);
                        if let Some(within) = ask_arg(
                            "within",
                            "Which container should scope this query? Prefer front_layer Compose/New Message after ambiguous twins.",
                            within_opts,
                        ) {
                            if within != "none" {
                                arguments["within"] = json!(within);
                            }
                        }
                        if let Some(near) = ask_arg(
                            "near",
                            "Which anchor should scope this query as near? Prefer the Compose/New Message front_layer after ambiguous twins.",
                            near_opts,
                        ) {
                            if near != "none" {
                                arguments["near"] = json!(near);
                            }
                        }
                    }
                }
            }
            "inspect" => match ask_arg(
                "region",
                "Which region should be inspected?",
                region_options(&|_| true),
            ) {
                Some(id) => arguments["region"] = json!(id),
                None => skip = Some("no regions known"),
            },
            "guard" => {
                match ask_arg(
                    "text",
                    "Which text should guard resolve for a click?",
                    text_options,
                ) {
                    Some(text) => arguments["text"] = json!(text),
                    None => skip = Some("no guard text"),
                }
                let roles = [
                    "any",
                    "button",
                    "link",
                    "text_field",
                    "heading",
                    "navigation",
                ];
                if let Some(role) = ask_arg(
                    "role",
                    "Which role should the click target have?",
                    roles
                        .iter()
                        .map(|r| (r.to_string(), format!("role {r}")))
                        .collect(),
                ) {
                    if role != "any" {
                        arguments["role"] = json!(role);
                    }
                }
                let zones = ["none", "left", "right", "top", "bottom", "center"];
                let suggested = repeated_query_hints(last_locate.as_ref());
                let failed_pos =
                    low_confidence_position(last_guard.as_ref(), last_guard_args.as_ref());
                if let Some(zone) = ask_arg(
                    "position",
                    "Where on the page is the click target? Prefer none unless sure.",
                    position_options(&zones, &suggested, failed_pos.as_deref()),
                ) {
                    if zone != "none" {
                        arguments["position"] = json!(zone);
                    }
                }

                // Harness nudge: after N identical ambiguous refuses, offer
                // within/near so JEV can scope. Guard still refuses until scoped.
                if nudge {
                    if let Some(guard) = last_guard.as_ref() {
                        let (within_opts, near_opts) = ambiguous_scope_options(guard, &regions);
                        if let Some(within) = ask_arg(
                            "within",
                            "Which container should scope this query? Prefer front_layer Compose/New Message after ambiguous twins.",
                            within_opts,
                        ) {
                            if within != "none" {
                                arguments["within"] = json!(within);
                            }
                        }
                        if let Some(near) = ask_arg(
                            "near",
                            "Which anchor should scope this query as near? Prefer the Compose/New Message front_layer after ambiguous twins.",
                            near_opts,
                        ) {
                            if near != "none" {
                                arguments["near"] = json!(near);
                            }
                        }
                    }
                }
                // Optional: pin the last locate's top as proposed.
                if let Some(located) = &last_locate {
                    if let Some(top_id) = located["candidates"][0]["id"].as_str() {
                        let propose = ask_arg(
                            "proposed",
                            "Should guard require a specific region as the top match?",
                            vec![
                                (
                                    "none".into(),
                                    "let guard rank freely from the text query".into(),
                                ),
                                (
                                    "last_locate_top".into(),
                                    format!("propose locate's top candidate {top_id}"),
                                ),
                            ],
                        );
                        if propose.as_deref() == Some("last_locate_top") {
                            arguments["proposed"] = json!(top_id);
                        }
                    }
                }
                if let Some(snap) = last_snapshot {
                    arguments["seen_snapshot"] = json!(snap);
                }
            }
            "verify" => match ask_arg(
                "expect_text",
                "Which text should be verified on the page?",
                expect_options,
            ) {
                Some(text) => arguments["expect_text"] = json!(text),
                None => skip = Some("no quoted text to verify"),
            },
            "diff" => {
                let (before, after) = last_diff_pair.as_ref().unwrap();
                arguments = json!({"before_snapshot": before, "after_snapshot": after});
            }
            other => {
                skip = Some(if other.is_empty() {
                    "empty choice"
                } else {
                    "unknown choice"
                })
            }
        }
        if let Some(reason) = skip {
            log.push(json!({"kind": "skipped", "step": step, "tool": tool, "reason": reason}));
            history.push(json!({"step": step, "tool": tool, "skipped": reason}));
            continue;
        }

        let called = Instant::now();
        let (is_error, body) = mcp.tool(&tool, &arguments);
        tool_calls += 1;
        let mut shown_args = arguments.clone();
        if shown_args.get("cdp").is_some() {
            shown_args["cdp"] = json!("<tab>");
        }
        log.push(json!({"kind": "mcp", "step": step, "tool": tool, "arguments": shown_args, "is_error": is_error, "latency_ms": called.elapsed().as_millis(), "body": body}));
        // Remember the latest snapshot id for guard's seen_snapshot.
        if !is_error {
            if let Some(snap) = body["snapshot"].as_u64() {
                last_snapshot = Some(snap);
            }
        }
        match tool.as_str() {
            "observe" if !is_error => {
                regions = body["regions"].as_array().cloned().unwrap_or_default();
            }
            "locate" if !is_error => {
                last_locate = Some(
                    json!({"query": shown_args, "candidates": body["candidates"].as_array().map(|c| c.iter().take(3).cloned().collect::<Vec<_>>()), "signals": body["signals"]}),
                )
            }
            "inspect" if !is_error => {
                inspected.push(arguments["region"].as_str().unwrap_or("").to_owned())
            }
            "guard" if !is_error => {
                let mut guard_body = body.clone();
                let decision = body["decision"].as_str().unwrap_or("");
                if decision == "allow" {
                    let before_snap = body["snapshot"].as_u64();
                    let allowed_id = body["target"]["id"].as_str().unwrap_or("").to_owned();
                    // Resolve geometry via inspect, then harness CDP click.
                    let (_, inspected_body) =
                        mcp.tool("inspect", &json!({"cdp": tab.ws, "region": allowed_id}));
                    tool_calls += 1;
                    log.push(json!({
                        "kind": "mcp",
                        "step": step,
                        "tool": "inspect",
                        "arguments": {"cdp": "<tab>", "region": allowed_id},
                        "is_error": false,
                        "latency_ms": 0,
                        "body": inspected_body,
                        "note": "harness geometry for post-Allow click",
                    }));
                    let target = &inspected_body["target"];
                    let (x, y, w, h) = (
                        target["x"].as_f64(),
                        target["y"].as_f64(),
                        target["width"].as_f64(),
                        target["height"].as_f64(),
                    );
                    match (x, y, w, h) {
                        (Some(x), Some(y), Some(w), Some(h)) => {
                            match press_center(&tab, x, y, w, h) {
                                Ok(()) => {
                                    clicks_executed += 1;
                                    guard_body["clicked"] = json!(true);
                                    // Page changed: stale regions / locate.
                                    regions.clear();
                                    inspected.clear();
                                    last_locate = None;
                                    // Observe after click for a fresh snapshot (diff + seen).
                                    let (_, after_obs) =
                                        mcp.tool("observe", &json!({"cdp": tab.ws}));
                                    tool_calls += 1;
                                    log.push(json!({
                                        "kind": "mcp",
                                        "step": step,
                                        "tool": "observe",
                                        "arguments": {"cdp": "<tab>"},
                                        "is_error": false,
                                        "latency_ms": 0,
                                        "body": after_obs,
                                        "note": "harness observe after click",
                                    }));
                                    if let Some(after_snap) = after_obs["snapshot"].as_u64() {
                                        last_snapshot = Some(after_snap);
                                        if let Some(before) = before_snap {
                                            last_diff_pair = Some((before, after_snap));
                                        }
                                    }
                                    regions = after_obs["regions"]
                                        .as_array()
                                        .cloned()
                                        .unwrap_or_default();
                                }
                                Err(err) => {
                                    guard_body["clicked"] = json!(false);
                                    guard_body["click_error"] = json!(err);
                                }
                            }
                        }
                        _ => {
                            guard_body["clicked"] = json!(false);
                            guard_body["click_error"] =
                                json!("inspect missing rectangle for allowed region");
                        }
                    }
                } else {
                    guard_body["clicked"] = json!(false);
                }
                last_guard = Some(guard_body.clone());
                last_guard_args = Some(arguments.clone());
                note_ambiguous_refuse(&mut ambiguous_streak, &guard_body, &arguments);
                if nudge || scope_nudge_active(&ambiguous_streak) {
                    log.push(json!({
                        "kind": "ambiguous-scope-nudge",
                        "step": step,
                        "streak": ambiguous_streak.as_ref().map(|s| json!({"key": s.key, "count": s.count})),
                        "active_next": scope_nudge_active(&ambiguous_streak),
                    }));
                }
                // Replace the logged guard body so the transcript shows click outcome
                // (inspect/observe pushes may follow the guard call).
                if let Some(entry) = log
                    .lines
                    .iter_mut()
                    .rev()
                    .find(|e| e["kind"] == "mcp" && e["tool"] == "guard")
                {
                    entry["body"] = guard_body.clone();
                }
                history.push(json!({
                    "step": step,
                    "tool": tool,
                    "arguments": shown_args,
                    "result": summarize(&tool, is_error, &guard_body)
                }));
                continue;
            }
            _ => {}
        }
        history.push(json!({"step": step, "tool": tool, "arguments": shown_args, "result": summarize(&tool, is_error, &body)}));
    }

    let truth = ground_truth(&tab);
    let mcp_events: Vec<&Value> = log.lines.iter().filter(|e| e["kind"] == "mcp").collect();
    let count = |pred: &dyn Fn(&Value) -> bool| mcp_events.iter().filter(|e| pred(e)).count();
    let refuse_reasons: Vec<Value> = mcp_events
        .iter()
        .filter(|e| e["tool"] == "guard" && e["body"]["decision"] != "allow")
        .map(|e| {
            json!({
                "reason": e["body"]["reason"].clone(),
                "fallback": e["body"]["fallback"].clone(),
            })
        })
        .collect();
    let summary = json!({
        "task": task.id,
        "text": task.text,
        "expected_page": task.expect,
        "jev_outcome": outcome,
        "tool_calls": tool_calls,
        "jev_calls": jev_calls,
        "wall_ms": started.elapsed().as_millis(),
        "guards_allow": count(&|e| e["tool"] == "guard" && e["body"]["decision"] == "allow"),
        "guards_refuse": count(&|e| e["tool"] == "guard" && e["body"]["decision"] != "allow" && e["body"]["decision"].is_string()),
        "clicks_executed": clicks_executed,
        "verified_true": count(&|e| e["tool"] == "verify" && e["body"]["verified"] == true),
        "refuse_reasons": refuse_reasons,
        "refused_ambiguous": count(&|e| e["body"]["fallback"] == "ambiguous" || e["body"]["reason"] == "ambiguous"),
        "ambiguous_scope_nudge_events": log.lines.iter().filter(|e| e["kind"] == "ambiguous-scope-nudge").count(),
        "refused_low_confidence": count(&|e| e["body"]["fallback"] == "low-confidence" || e["body"]["reason"] == "low-confidence"),
        "refused_proposed_not_top": count(&|e| e["body"]["reason"] == "proposed-not-top" || e["body"]["fallback"] == "proposed-not-top"),
        "no_effect": count(&|e| e["body"]["fallback"] == "no-effect"),
        "repeated_query_signals": count(&|e| e["tool"] == "locate" && e["body"]["signals"].as_array().is_some_and(|s| s.iter().any(|x| x["kind"] == "repeated_query"))),
        "tool_errors": count(&|e| e["is_error"] == true),
        "error_variants": mcp_events.iter().filter(|e| e["is_error"] == true).map(|e| e["body"]["variant"].clone()).collect::<Vec<_>>(),
        "calls": mcp_events.iter().map(|e| e["tool"].clone()).collect::<Vec<_>>(),
        "page_after": truth,
    });
    log.push(json!({"kind": "final", "outcome": outcome}));
    log.push(json!({"kind": "summary", "summary": summary}));
    mcp.close();
    close_tab(&args.cdp, &tab);

    let dir = args.out.join("transcripts");
    std::fs::create_dir_all(&dir).unwrap();
    let mut file = std::fs::File::create(dir.join(format!("{}.jsonl", task.id))).unwrap();
    for line in &log.lines {
        writeln!(file, "{line}").unwrap();
    }
    summary
}

fn main() {
    let args = parse_args();
    std::fs::create_dir_all(&args.out).unwrap();
    if args.screenshot_only {
        let tab = open_tab(&args.cdp, &format!("{}/index.html", args.site));
        match screenshots(&tab, &args.out) {
            Ok(paths) => {
                for path in paths {
                    println!("screenshot {}", path.display());
                }
            }
            Err(err) => println!("screenshot failed: {err}"),
        }
        close_tab(&args.cdp, &tab);
        return;
    }
    assert_eq!(
        std::env::var("ULTRA_INSTINCT_JEV").ok().as_deref(),
        Some("1"),
        "set ULTRA_INSTINCT_JEV=1 to call System One"
    );
    let jev = Jev {
        client: typesafe_sdk::blocking::Client::from_env().expect("TYPESAFE_API_KEY"),
    };
    let mut probe = Mcp::spawn(&args.bin);
    probe.request("initialize", json!({"protocolVersion": "2024-11-05", "capabilities": {}, "clientInfo": {"name": "jev-live-drive", "version": "0"}}));
    let listed = probe.request("tools/list", json!({}));
    probe.close();
    let tools: Value = listed["result"]["tools"]
        .as_array()
        .map(|tools| {
            tools
                .iter()
                .map(|t| json!({"name": t["name"], "description": t["description"]}))
                .collect()
        })
        .unwrap_or(Value::Null);

    if let Some(path) = &args.task_json {
        let raw = std::fs::read_to_string(path).expect("read --task-json");
        let spec: Value = serde_json::from_str(&raw).expect("--task-json is JSON");
        let field = |name: &str| -> String {
            spec[name]
                .as_str()
                .unwrap_or_else(|| panic!("--task-json needs a string `{name}`"))
                .to_owned()
        };
        let (id, start_url, text) = (field("id"), field("start_url"), field("text"));
        assert!(
            start_url.starts_with("http://127.0.0.1:"),
            "start_url must be on 127.0.0.1"
        );
        // Static lifetime for one process-long task.
        let task = Task {
            id: Box::leak(id.into_boxed_str()),
            start: Box::leak(start_url.into_boxed_str()),
            text: Box::leak(text.into_boxed_str()),
            expect: "bench checker owns the verdict",
        };
        let summary = run_task(&args, &jev, &task, &tools);
        println!("{summary}");
        return;
    }

    let mut summaries = Vec::new();
    for task in TASKS
        .iter()
        .filter(|t| args.only.as_deref().is_none_or(|only| t.id == only))
    {
        let summary = run_task(&args, &jev, task, &tools);
        println!("{summary}");
        summaries.push(summary);
    }
    std::fs::write(
        args.out.join("summary.json"),
        serde_json::to_string_pretty(&summaries).unwrap(),
    )
    .unwrap();
}

#[cfg(test)]
mod ambiguous_scope_tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn ambiguous_key_uses_sorted_twin_pair() {
        let guard = json!({
            "reason": "ambiguous",
            "fallback": "ambiguous",
            "candidates": [
                {"id": "n1013", "label": "Send"},
                {"id": "n954", "label": "Send"}
            ]
        });
        let args = json!({"text": "Send", "role": "button", "proposed": "n1013"});
        assert_eq!(
            ambiguous_refuse_key(&guard, &args).as_deref(),
            Some("twins:n1013|n954")
        );
        // Order of the top two does not reset the streak key.
        let flipped = json!({
            "reason": "ambiguous",
            "candidates": [
                {"id": "n954", "label": "Send"},
                {"id": "n1013", "label": "Send"}
            ]
        });
        assert_eq!(
            ambiguous_refuse_key(&flipped, &args),
            ambiguous_refuse_key(&guard, &args)
        );
    }

    #[test]
    fn streak_hits_threshold_then_clears_on_allow() {
        let mut streak = None;
        let guard = json!({
            "reason": "ambiguous",
            "candidates": [{"id": "a"}, {"id": "b"}]
        });
        let args = json!({"text": "Send"});
        note_ambiguous_refuse(&mut streak, &guard, &args);
        assert!(!scope_nudge_active(&streak));
        note_ambiguous_refuse(&mut streak, &guard, &args);
        assert!(scope_nudge_active(&streak));
        assert_eq!(streak.as_ref().unwrap().count, 2);

        let allow = json!({"decision": "allow", "reason": "ok"});
        note_ambiguous_refuse(&mut streak, &allow, &args);
        assert!(streak.is_none());
        assert!(!scope_nudge_active(&streak));
    }

    #[test]
    fn scope_options_prefer_new_message_front_layer() {
        let guard = json!({
            "reason": "ambiguous",
            "front_layer": [
                {"id": "n900", "label": "Other sheet", "modal": false},
                {"id": "n968", "label": "New Message", "modal": false}
            ],
            "candidates": [
                {"id": "n1013", "label": "Send"},
                {"id": "n954", "label": "Send"}
            ]
        });
        let regions = vec![
            json!({"id": "n1013", "label": "Send", "parent": "n968"}),
            json!({"id": "n954", "label": "Send", "parent": null}),
            json!({"id": "n968", "label": "New Message", "parent": null}),
        ];
        let (within, near) = ambiguous_scope_options(&guard, &regions);
        assert_eq!(within[0].0, "none");
        // New Message preferred ahead of Other sheet.
        assert_eq!(within[1].0, "n968");
        assert!(within[1].1.contains("New Message"));
        assert!(within.iter().any(|(id, _)| id == "n900"));
        assert!(near
            .iter()
            .any(|(id, about)| id == "n968" && about.contains("New Message")));
        assert_eq!(AMBIGUOUS_SCOPE_THRESHOLD, 2);
    }
}
