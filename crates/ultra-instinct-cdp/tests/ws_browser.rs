//! `CdpBrowser` over the browser websocket (`connect_websocket`) and the
//! attached `SessionBackend`, against a fake Chrome: a 127.0.0.1 listener
//! speaking tungstenite on a test-only thread.

#![cfg(unix)]

use std::net::{TcpListener, TcpStream};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use serde_json::{json, Value};
use tungstenite::{Message, WebSocket};
use ultra_instinct_cdp::{
    CdpBrowser, CdpError, CdpTransport, PipeLimits, SessionBackend, TargetBackend, ATTACH_ENDPOINT,
};

/// Chrome's side of the browser socket.
struct Fake {
    socket: WebSocket<TcpStream>,
}

impl Fake {
    /// The next request, `None` once the client closed the socket.
    fn recv(&mut self) -> Option<Value> {
        loop {
            match self.socket.read() {
                Ok(Message::Text(text)) => {
                    return Some(serde_json::from_str(text.as_str()).expect("request json"))
                }
                Ok(_) => continue,
                Err(_) => return None,
            }
        }
    }

    fn expect(&mut self, method: &str) -> Value {
        let request = self.recv().expect("a request");
        assert_eq!(request["method"], method, "{request}");
        request
    }

    fn send(&mut self, value: Value) {
        self.socket
            .send(Message::Text(value.to_string().into()))
            .expect("send");
    }

    fn reply(&mut self, request: &Value, result: Value) {
        self.send(json!({"id": request["id"], "result": result}));
    }

    fn attach(&mut self, session: &str) {
        let request = self.expect("Target.attachToTarget");
        assert_eq!(request["params"]["flatten"], true);
        self.reply(&request, json!({"sessionId": session}));
    }
}

/// A fake browser socket URL; `script` runs after the connect's
/// `Browser.getVersion` is answered.
fn fake(script: impl FnOnce(&mut Fake) + Send + 'static) -> (String, JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
    let port = listener.local_addr().expect("addr").port();
    let handle = thread::spawn(move || {
        let (stream, _) = listener.accept().expect("accept");
        let mut fake = Fake {
            socket: tungstenite::accept(stream).expect("ws upgrade"),
        };
        let version = fake.expect("Browser.getVersion");
        fake.reply(&version, json!({"product": "Chrome/156"}));
        script(&mut fake);
    });
    (
        format!("ws://127.0.0.1:{port}/devtools/browser/abc-123"),
        handle,
    )
}

#[test]
fn a_session_reply_and_events_reach_their_own_queues() {
    let (url, fake) = fake(|fake| {
        fake.attach("S1");
        fake.attach("S2");
        let eval = fake.expect("Runtime.evaluate");
        assert_eq!(eval["sessionId"], "S1", "{eval}");
        fake.send(json!({"method": "Page.loadEventFired", "sessionId": "S2", "params": {}}));
        fake.send(json!({"method": "Page.frameNavigated", "sessionId": "S1", "params": {}}));
        fake.send(json!({"method": "Target.targetCreated", "params": {"x": 1}}));
        fake.send(json!({"id": 999, "result": {"who": "stray"}}));
        fake.reply(&eval, json!({"result": {"value": 7}}));
        while fake.recv().is_some() {}
    });
    let browser = CdpBrowser::connect_websocket(&url).expect("connect");
    let mut one = browser.page("T1").expect("attach");
    let mut two = browser.page("T2").expect("attach");
    let out = one
        .call("Runtime.evaluate", r#"{"expression":"7"}"#)
        .expect("evaluate");
    assert_eq!(out, r#"{"result":{"value":7}}"#);
    let methods = |events: Vec<ultra_instinct_cdp::CdpEvent>| {
        events.into_iter().map(|e| e.method).collect::<Vec<_>>()
    };
    assert_eq!(methods(one.drain_events()), ["Page.frameNavigated"]);
    assert_eq!(methods(two.drain_events()), ["Page.loadEventFired"]);
    assert_eq!(methods(browser.drain_events()), ["Target.targetCreated"]);
    drop((one, two, browser));
    fake.join().expect("fake");
}

#[test]
fn a_destroyed_target_ends_its_session_and_not_the_socket() {
    let (url, fake) = fake(|fake| {
        fake.attach("S1");
        let eval = fake.expect("Runtime.evaluate");
        assert_eq!(eval["sessionId"], "S1");
        fake.send(json!({"method": "Target.targetDestroyed", "params": {"targetId": "T1"}}));
        let version = fake.expect("Browser.getVersion");
        fake.reply(&version, json!({"product": "still here"}));
        while fake.recv().is_some() {}
    });
    let browser = CdpBrowser::connect_websocket(&url).expect("connect");
    let mut page = browser.page("T1").expect("attach");
    let gone = CdpError::TargetGone {
        target_id: "T1".into(),
    };
    assert_eq!(page.call("Runtime.evaluate", "{}"), Err(gone.clone()));
    assert_eq!(page.call("Runtime.evaluate", "{}"), Err(gone));
    let version = browser.call("Browser.getVersion", "{}").expect("browser");
    assert!(version.contains("still here"), "{version}");
    drop((page, browser));
    fake.join().expect("fake");
}

#[test]
fn the_socket_closing_fails_every_later_call() {
    let (url, fake) = fake(|fake| {
        fake.expect("Target.getTargets");
        // Dropping the socket closes it with no reply.
    });
    let browser = CdpBrowser::connect_websocket(&url).expect("connect");
    let started = Instant::now();
    match browser.targets() {
        Err(CdpError::PipeClosed { .. }) => {}
        other => panic!("expected PipeClosed, got {other:?}"),
    }
    assert!(started.elapsed() < Duration::from_secs(5));
    fake.join().expect("fake");
    match browser.call("Browser.getVersion", "{}") {
        Err(CdpError::PipeClosed { .. }) => {}
        other => panic!("still closed, got {other:?}"),
    }
}

#[test]
fn a_big_request_goes_out_while_the_peer_writes_first() {
    // The peer writes 8 MiB of events before it reads the next request, and
    // the request is 4 MiB: blocking writes on both sides would deadlock.
    let (url, fake) = fake(|fake| {
        let filler = "x".repeat(4096);
        for _ in 0..2048 {
            fake.send(json!({"method": "Log.entryAdded", "params": {"text": filler}}));
        }
        let call = fake.expect("Runtime.evaluate");
        fake.reply(
            &call,
            json!({"len": call["params"]["expression"].as_str().map(str::len)}),
        );
        while fake.recv().is_some() {}
    });
    let browser = CdpBrowser::connect_websocket(&url).expect("connect");
    let big = "y".repeat(4 << 20);
    let out = browser
        .call("Runtime.evaluate", &json!({"expression": big}).to_string())
        .expect("the big call");
    assert_eq!(out, format!(r#"{{"len":{}}}"#, 4 << 20));
    drop(browser);
    fake.join().expect("fake");
}

#[test]
fn an_unanswered_connect_says_to_accept_the_prompt() {
    // Chrome holds the first call while its "Allow remote debugging?" prompt waits.
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
    let port = listener.local_addr().expect("addr").port();
    let held = thread::spawn(move || {
        let (stream, _) = listener.accept().expect("accept");
        let mut socket = tungstenite::accept(stream).expect("ws upgrade");
        let _ = socket.read();
        thread::sleep(Duration::from_millis(800));
    });
    let url = format!("ws://127.0.0.1:{port}/devtools/browser/x");
    let wait = Duration::from_millis(300);
    let err = CdpBrowser::connect_websocket_with(&url, PipeLimits::default(), wait)
        .err()
        .expect("no answer");
    let text = err.to_string();
    assert!(text.contains("Allow remote debugging"), "{text}");
    assert!(
        text.contains("chrome://inspect/#remote-debugging"),
        "{text}"
    );
    held.join().expect("held");

    // A peer that never finishes the upgrade gets the same hint.
    let silent = TcpListener::bind("127.0.0.1:0").expect("bind");
    let port = silent.local_addr().expect("addr").port();
    let held = thread::spawn(move || {
        let _stream = silent.accept();
        thread::sleep(Duration::from_millis(800));
    });
    let url = format!("ws://127.0.0.1:{port}/devtools/browser/x");
    let err = CdpBrowser::connect_websocket_with(&url, PipeLimits::default(), wait)
        .err()
        .expect("no upgrade");
    assert!(err.to_string().contains("Allow remote debugging"), "{err}");
    held.join().expect("held");
}

#[test]
fn wss_is_refused_without_a_socket() {
    let err = CdpBrowser::connect_websocket("wss://127.0.0.1:1/devtools/browser/x")
        .err()
        .expect("refused");
    assert!(err.to_string().contains("wss is not supported"), "{err}");
}

#[test]
fn the_attached_backend_closes_only_the_tabs_it_opened() {
    let (url, fake) = fake(|fake| {
        let discover = fake.expect("Target.setDiscoverTargets");
        fake.reply(&discover, json!({}));
        let list = fake.expect("Target.getTargets");
        fake.reply(
            &list,
            json!({"targetInfos": [
                {"targetId": "U1", "type": "page", "url": "https://mail.example/", "title": "Mail"},
                {"targetId": "W1", "type": "service_worker", "url": "", "title": ""},
            ]}),
        );
        for id in ["A1", "A2"] {
            let create = fake.expect("Target.createTarget");
            fake.reply(&create, json!({"targetId": id}));
            let activate = fake.expect("Target.activateTarget");
            assert_eq!(activate["params"]["targetId"], id);
            fake.reply(&activate, json!({}));
        }
        let close = fake.expect("Target.closeTarget");
        assert_eq!(close["params"]["targetId"], "A2");
        fake.reply(&close, json!({"success": true}));
        // The drop: only the tab still open that the backend made.
        let mut after = Vec::new();
        while let Some(request) = fake.recv() {
            after.push(request.clone());
            fake.reply(&request, json!({"success": true}));
        }
        let methods: Vec<_> = after
            .iter()
            .map(|r| format!("{} {}", r["method"], r["params"]["targetId"]))
            .collect();
        assert_eq!(methods, [r#""Target.closeTarget" "A1""#], "{after:?}");
    });
    let browser = CdpBrowser::connect_websocket(&url).expect("connect");
    let mut backend = SessionBackend::attached(browser);
    assert_eq!(backend.endpoint(), Some(ATTACH_ENDPOINT));
    let tabs = backend.list_pages(ATTACH_ENDPOINT).expect("tabs");
    assert_eq!(tabs.len(), 1);
    assert_eq!(tabs[0].key, "attach:U1");
    let one = backend
        .create_page(ATTACH_ENDPOINT, "about:blank")
        .expect("tab");
    assert_eq!(one.key, "attach:A1");
    backend
        .create_page(ATTACH_ENDPOINT, "about:blank")
        .expect("tab");
    assert!(backend.close_page(ATTACH_ENDPOINT, "A2").expect("close"));
    drop(backend);
    fake.join().expect("fake");
}
