//! Live Chrome parallel-tab test. Ignored by default (not in CI).
//!
//! ```text
//! chrome --headless=new --remote-debugging-port=9333 --user-data-dir=/tmp/ultra-instinct-par
//! ULTRA_INSTINCT_CDP=http://127.0.0.1:9333 cargo test -p ultra-instinct-agent --test live_parallel -- --ignored --nocapture
//! ```
//!
//! Opens independent background tabs, runs eight agents concurrently and
//! sequentially, and verifies each click changed only its own page.

use std::collections::HashSet;
use std::io::{Read, Write};
use std::net::TcpStream;
use std::time::{Duration, Instant};

use serde_json::Value;
use ultra_instinct_agent::{Agent, AgentBuilder, BrowserRuntime, VerificationKind};
use ultra_instinct_browser::{open_tab, BrowserSession, WebSocketTransport};
use ultra_instinct_core::ActionKind;
use ultra_instinct_policy::InstinctPolicy;

const N: usize = 8;

fn assert_send<T: Send>() {}

#[test]
fn websocket_agent_is_send() {
    assert_send::<Agent<BrowserSession<WebSocketTransport>, InstinctPolicy>>();
}

fn data_url(index: usize) -> String {
    let page = format!(
        "<!doctype html><title>tab {index}</title>\
         <button onclick=\"document.getElementById('out').textContent='clicked {index}'\">Go</button>\
         <p id=out></p>"
    );
    let mut out = String::from("data:text/html,");
    for byte in page.bytes() {
        if byte.is_ascii_alphanumeric() || b"-_.~".contains(&byte) {
            out.push(byte as char);
        } else {
            out.push_str(&format!("%{byte:02X}"));
        }
    }
    out
}

fn run_tab(endpoint: &str, index: usize) -> String {
    let transport = open_tab(endpoint).unwrap_or_else(|err| panic!("open tab {index}: {err}"));
    let target_id = transport
        .target_id()
        .unwrap_or_else(|| panic!("tab {index} has no owned target id"))
        .to_owned();
    assert!(
        page_target_ids(endpoint).contains(&target_id),
        "tab {index} target did not survive browser websocket closure: {target_id}"
    );

    let mut session = BrowserSession::new(transport);
    session
        .navigate(&data_url(index))
        .unwrap_or_else(|err| panic!("navigate tab {index}: {err}"));
    session.settle();

    let mut agent = AgentBuilder::new(session, InstinctPolicy::default())
        .max_steps(4)
        .build("Click Go");
    let outcome = agent.run();
    eprintln!("tab {index}: {outcome:?}");
    let first = outcome
        .steps()
        .first()
        .unwrap_or_else(|| panic!("tab {index} has no action step: {outcome:?}"));
    assert_eq!(first.kind, ActionKind::Click, "tab {index}: {outcome:?}");
    assert_eq!(
        first.verification,
        VerificationKind::StateChanged,
        "tab {index}: {outcome:?}"
    );

    let mut session = agent.into_browser();
    let manifold = BrowserRuntime::observe(&mut session)
        .unwrap_or_else(|err| panic!("re-observe tab {index}: {err}"))
        .clone();
    let expected = format!("clicked {index}");
    assert!(
        manifold
            .regions()
            .any(|region| region.label() == expected.as_str()),
        "tab {index} did not show its own result `{expected}`"
    );
    drop(session);
    target_id
}

fn page_target_ids(endpoint: &str) -> Vec<String> {
    let host = endpoint
        .strip_prefix("http://")
        .expect("debugging endpoint must be http://")
        .split('/')
        .next()
        .expect("endpoint host");
    let mut stream = TcpStream::connect(host).expect("connect to Chrome /json/list");
    stream
        .set_read_timeout(Some(Duration::from_secs(5)))
        .expect("set /json/list timeout");
    write!(
        stream,
        "GET /json/list HTTP/1.1\r\nHost: {host}\r\nConnection: close\r\n\r\n"
    )
    .expect("write /json/list request");

    let mut response = Vec::new();
    let mut chunk = [0u8; 4096];
    let header_end = loop {
        if let Some(index) = response.windows(4).position(|part| part == b"\r\n\r\n") {
            break index;
        }
        let count = stream.read(&mut chunk).expect("read /json/list headers");
        assert_ne!(count, 0, "Chrome closed /json/list before sending headers");
        response.extend_from_slice(&chunk[..count]);
    };
    let headers = String::from_utf8_lossy(&response[..header_end]).to_ascii_lowercase();
    let content_length = headers
        .lines()
        .find_map(|line| line.strip_prefix("content-length:"))
        .and_then(|value| value.trim().parse::<usize>().ok())
        .expect("Chrome /json/list Content-Length");
    let body_start = header_end + 4;
    let body_end = body_start + content_length;
    while response.len() < body_end {
        let count = stream.read(&mut chunk).expect("read /json/list body");
        assert_ne!(count, 0, "Chrome closed /json/list before sending its body");
        response.extend_from_slice(&chunk[..count]);
    }
    let targets: Vec<Value> =
        serde_json::from_slice(&response[body_start..body_end]).expect("parse /json/list");
    targets
        .iter()
        .filter_map(|target| target.get("id").and_then(Value::as_str))
        .map(str::to_owned)
        .collect()
}

fn assert_targets_closed(endpoint: &str, ids: &[String]) {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let remaining: Vec<_> = page_target_ids(endpoint)
            .into_iter()
            .filter(|id| ids.contains(id))
            .collect();
        if remaining.is_empty() {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "owned targets {remaining:?} remain in /json/list after their transports dropped"
        );
        std::thread::sleep(Duration::from_millis(50));
    }
}

#[test]
#[ignore = "needs a live Chrome with --remote-debugging-port (ULTRA_INSTINCT_CDP)"]
fn eight_agents_use_independent_tabs_in_parallel_and_sequentially() {
    let endpoint =
        std::env::var("ULTRA_INSTINCT_CDP").unwrap_or_else(|_| "http://127.0.0.1:9333".to_owned());

    let parallel_started = Instant::now();
    let parallel_ids = std::thread::scope(|scope| {
        let handles: Vec<_> = (0..N)
            .map(|index| {
                let endpoint = endpoint.as_str();
                scope.spawn(move || run_tab(endpoint, index))
            })
            .collect();
        handles
            .into_iter()
            .map(|handle| handle.join().expect("parallel agent thread"))
            .collect::<Vec<_>>()
    });
    let parallel_time = parallel_started.elapsed();
    let distinct: HashSet<_> = parallel_ids.iter().collect();
    assert_eq!(distinct.len(), N, "parallel agents shared a target id");
    assert_targets_closed(&endpoint, &parallel_ids);

    let sequential_started = Instant::now();
    let sequential_ids: Vec<_> = (0..N).map(|index| run_tab(&endpoint, index)).collect();
    let sequential_time = sequential_started.elapsed();
    assert_targets_closed(&endpoint, &sequential_ids);
    eprintln!("parallel {N} agents: {parallel_time:?}");
    eprintln!("sequential {N} agents: {sequential_time:?}");
}
