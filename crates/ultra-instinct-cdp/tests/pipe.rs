//! `PipeBrowser` / `PipePage` against a fake Chrome on an in-process pipe
//! pair: `std::io::pipe()` per direction, the fake on a test-only thread.

#![cfg(unix)]

use std::io::{pipe, BufRead, BufReader, PipeReader, PipeWriter, Write};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use proptest::prelude::*;
use serde_json::{json, Value};
use ultra_instinct_cdp::{
    CdpError, CdpTransport, FrameSplitter, PipeBrowser, PipeLimits, PipeTarget,
};

/// Chrome's side of the pipe: reads NUL-terminated requests, writes frames.
struct Fake {
    requests: BufReader<PipeReader>,
    out: PipeWriter,
}

impl Fake {
    /// The next request, `None` at EOF (the browser side was dropped).
    fn recv(&mut self) -> Option<Value> {
        let mut frame = Vec::new();
        let n = self
            .requests
            .read_until(0, &mut frame)
            .expect("read request");
        if n == 0 {
            return None;
        }
        assert_eq!(frame.pop(), Some(0), "request frame ends with NUL");
        Some(serde_json::from_slice(&frame).expect("request json"))
    }

    fn expect(&mut self, method: &str) -> Value {
        let request = self.recv().expect("a request");
        assert_eq!(request["method"], method, "{request}");
        request
    }

    fn send(&mut self, value: Value) {
        self.raw(format!("{value}\0").as_bytes());
    }

    fn raw(&mut self, bytes: &[u8]) {
        self.out.write_all(bytes).expect("write frame");
        self.out.flush().expect("flush");
    }

    fn reply(&mut self, request: &Value, result: Value) {
        self.send(json!({"id": request["id"], "result": result}));
    }

    /// Answer an attach with session `session`.
    fn attach(&mut self, session: &str) {
        let request = self.expect("Target.attachToTarget");
        assert_eq!(request["params"]["flatten"], true);
        self.reply(&request, json!({"sessionId": session}));
    }
}

fn pause() {
    thread::sleep(Duration::from_millis(30));
}

fn fake_with(
    limits: PipeLimits,
    script: impl FnOnce(&mut Fake) + Send + 'static,
) -> (PipeBrowser, JoinHandle<()>) {
    let (requests, browser_out) = pipe().expect("pipe");
    let (browser_in, out) = pipe().expect("pipe");
    let browser = PipeBrowser::with_limits(browser_in, browser_out, limits);
    let fake = thread::spawn(move || {
        let mut fake = Fake {
            requests: BufReader::new(requests),
            out,
        };
        script(&mut fake);
    });
    (browser, fake)
}

fn fake(script: impl FnOnce(&mut Fake) + Send + 'static) -> (PipeBrowser, JoinHandle<()>) {
    fake_with(PipeLimits::default(), script)
}

fn short_deadline(millis: u64) -> PipeLimits {
    PipeLimits {
        call_deadline: Duration::from_millis(millis),
        ..PipeLimits::default()
    }
}

/// One end of a socketpair as both fd 3 and fd 4 (buwiz-computer's launcher):
/// the host passes two dups of its end, `PipeBrowser::new(s.try_clone()?, s)`.
#[test]
fn one_socket_dup_as_reader_and_writer_routes_and_closes() {
    use std::os::fd::OwnedFd;
    use std::os::unix::net::UnixStream;

    let (host, chrome) = UnixStream::pair().expect("socketpair");
    let browser = PipeBrowser::new(host.try_clone().expect("dup"), host);
    let fake = thread::spawn(move || {
        let mut fake = Fake {
            requests: BufReader::new(PipeReader::from(OwnedFd::from(
                chrome.try_clone().expect("dup"),
            ))),
            out: PipeWriter::from(OwnedFd::from(chrome)),
        };
        let version = fake.expect("Browser.getVersion");
        fake.reply(&version, json!({"product": "Chrome/156"}));
        fake.attach("S1");
        let eval = fake.expect("Runtime.evaluate");
        assert_eq!(eval["sessionId"], "S1", "{eval}");
        fake.send(json!({"method": "Page.loadEventFired", "sessionId": "S1", "params": {}}));
        fake.reply(&eval, json!({"result": {"value": 7}}));
        // Dropping both dups of Chrome's end closes the socket.
    });
    let version = browser.call("Browser.getVersion", "{}").expect("version");
    assert!(version.contains("Chrome/156"), "{version}");
    let mut page = browser.page("T1").expect("attach");
    let out = page
        .call("Runtime.evaluate", r#"{"expression":"7"}"#)
        .expect("evaluate");
    assert!(out.contains('7'), "{out}");
    let events = page.drain_events();
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].method, "Page.loadEventFired");
    fake.join().expect("fake");
    match page.call("Runtime.evaluate", "{}") {
        Err(CdpError::PipeClosed { .. }) => {}
        other => panic!("expected PipeClosed after the peer closed, got {other:?}"),
    }
}

#[test]
fn defaults_match_the_websocket_transport() {
    let limits = PipeLimits::default();
    assert_eq!(limits.max_frame_bytes, 64 << 20);
    assert_eq!(limits.call_deadline, Duration::from_secs(30));
}

#[test]
fn frames_split_across_reads_several_per_read_and_nul_on_a_read_boundary() {
    let (browser, fake) = fake(|fake| {
        // Split across reads: half a frame, a pause, the rest.
        let request = fake.expect("A.one");
        let frame = format!("{}\0", json!({"id": request["id"], "result": {"n": 1}}));
        let (head, tail) = frame.as_bytes().split_at(frame.len() / 2);
        fake.raw(head);
        pause();
        fake.raw(tail);
        // Several in one write: two events, then the reply.
        let request = fake.expect("A.two");
        let batch = format!(
            "{}\0{}\0{}\0",
            json!({"method": "E.one", "params": {}}),
            json!({"method": "E.two", "params": {"k": 2}}),
            json!({"id": request["id"], "result": {"n": 2}}),
        );
        fake.raw(batch.as_bytes());
        // The NUL alone on the next read, glued to the reply.
        let request = fake.expect("A.three");
        fake.raw(json!({"method": "E.three"}).to_string().as_bytes());
        pause();
        fake.raw(format!("\0{}\0", json!({"id": request["id"], "result": {"n": 3}})).as_bytes());
    });
    assert_eq!(browser.call("A.one", "{}"), Ok(r#"{"n":1}"#.into()));
    assert_eq!(browser.call("A.two", "{}"), Ok(r#"{"n":2}"#.into()));
    assert_eq!(browser.call("A.three", "{}"), Ok(r#"{"n":3}"#.into()));
    let events = browser.drain_events();
    let methods: Vec<_> = events.iter().map(|e| e.method.as_str()).collect();
    assert_eq!(methods, ["E.one", "E.two", "E.three"]);
    assert_eq!(events[1].params["k"], 2);
    assert_eq!(events[2].params, Value::Null);
    fake.join().unwrap();
}

#[test]
fn an_answer_out_of_order_never_reaches_the_wrong_call() {
    let (browser, fake) = fake_with(short_deadline(150), |fake| {
        let first = fake.expect("Slow.call");
        assert_eq!(first["id"], 1);
        // The client gives up on id 1, then asks again.
        let second = fake.expect("Next.call");
        assert_eq!(second["id"], 2);
        fake.send(json!({"id": 99, "result": {"who": "stray"}}));
        fake.send(json!({"id": 1, "result": {"who": "late"}}));
        fake.send(json!({"id": 2, "error": {"code": -32000, "message": "nope"}}));
        let third = fake.expect("Third.call");
        fake.reply(&third, json!({"who": "third"}));
    });
    assert!(matches!(
        browser.call("Slow.call", "{}"),
        Err(CdpError::Transport { .. })
    ));
    assert_eq!(
        browser.call("Next.call", "{}"),
        Err(CdpError::Protocol {
            message: "nope".into()
        })
    );
    assert_eq!(
        browser.call("Third.call", "{}"),
        Ok(r#"{"who":"third"}"#.into())
    );
    fake.join().unwrap();
}

#[test]
fn two_pages_share_one_pipe_and_each_drains_only_its_own_events() {
    let (browser, fake) = fake(|fake| {
        fake.attach("S1");
        fake.attach("S2");
        let request = fake.expect("Runtime.evaluate");
        assert_eq!(request["sessionId"], "S1");
        assert_eq!(request["params"]["expression"], "1");
        fake.send(json!({"sessionId": "S2", "method": "Page.loadEventFired", "params": {"p": 2}}));
        fake.send(json!({"sessionId": "S1", "method": "Page.frameNavigated", "params": {"p": 1}}));
        fake.send(json!({"method": "Target.targetInfoChanged", "params": {}}));
        fake.send(json!({"sessionId": "S9", "method": "Unknown.session", "params": {}}));
        fake.send(json!({"id": request["id"], "sessionId": "S1", "result": {"page": 1}}));
        let request = fake.expect("Runtime.evaluate");
        assert_eq!(request["sessionId"], "S2");
        fake.send(json!({"sessionId": "S1", "method": "Page.domContentEventFired", "params": {}}));
        fake.send(json!({"id": request["id"], "sessionId": "S2", "result": {"page": 2}}));
        // Detach on drop, page 1 then page 2.
        assert_eq!(
            fake.expect("Target.detachFromTarget")["params"]["sessionId"],
            "S1"
        );
        assert_eq!(
            fake.expect("Target.detachFromTarget")["params"]["sessionId"],
            "S2"
        );
    });
    let mut one = browser.page("T1").expect("page one");
    let mut two = browser.page("T2").expect("page two");
    assert_eq!((one.session_id(), one.target_id()), ("S1", "T1"));
    assert_eq!(
        one.call("Runtime.evaluate", r#"{"expression":"1"}"#),
        Ok(r#"{"page":1}"#.into())
    );
    assert_eq!(
        two.call("Runtime.evaluate", r#"{"expression":"2"}"#),
        Ok(r#"{"page":2}"#.into())
    );
    let methods = |events: Vec<ultra_instinct_cdp::CdpEvent>| {
        events.into_iter().map(|e| e.method).collect::<Vec<_>>()
    };
    assert_eq!(
        methods(one.drain_events()),
        ["Page.frameNavigated", "Page.domContentEventFired"]
    );
    assert_eq!(methods(two.drain_events()), ["Page.loadEventFired"]);
    // Browser-level events only; the unattached session's event is dropped.
    assert_eq!(
        methods(browser.drain_events()),
        ["Target.targetInfoChanged"]
    );
    assert!(one.drain_events().is_empty());
    drop(one);
    drop(two);
    drop(browser);
    fake.join().unwrap();
}

#[test]
fn browser_level_events_queue_on_the_browser_and_survive_page_calls() {
    let (browser, fake) = fake(|fake| {
        fake.attach("S1");
        let request = fake.expect("DOM.enable");
        fake.send(json!({"method": "Target.attachedToTarget", "params": {"sessionId": "S1"}}));
        fake.reply(&request, json!({}));
        let request = fake.expect("Browser.getVersion");
        assert!(request.get("sessionId").is_none(), "{request}");
        fake.send(json!({"method": "Target.targetCreated", "params": {"targetInfo": {}}}));
        fake.reply(&request, json!({"product": "Fake/1"}));
        let _ = fake.recv();
    });
    let mut page = browser.page("T1").expect("page");
    page.call("DOM.enable", "{}").expect("enable");
    assert!(page.drain_events().is_empty());
    let mut transport = browser;
    assert_eq!(
        CdpTransport::call(&mut transport, "Browser.getVersion", "{}"),
        Ok(r#"{"product":"Fake/1"}"#.into())
    );
    let methods: Vec<_> = CdpTransport::drain_events(&mut transport)
        .into_iter()
        .map(|e| e.method)
        .collect();
    assert_eq!(methods, ["Target.attachedToTarget", "Target.targetCreated"]);
    drop(page);
    drop(transport);
    fake.join().unwrap();
}

#[test]
fn an_oversize_frame_is_refused_and_the_pipe_stays_closed() {
    let limits = PipeLimits {
        max_frame_bytes: 1024,
        ..PipeLimits::default()
    };
    let (browser, fake) = fake_with(limits, |fake| {
        fake.attach("S1");
        let request = fake.expect("Page.captureScreenshot");
        let big = format!(
            "{}\0",
            json!({"id": request["id"], "result": {"data": "x".repeat(4096)}})
        );
        fake.raw(big.as_bytes());
        // Nothing else is ever sent: the next request read is EOF.
        assert!(fake.recv().is_none());
    });
    let mut page = browser.page("T1").expect("page");
    let refused = CdpError::FrameTooLarge { limit: 1024 };
    assert_eq!(
        page.call("Page.captureScreenshot", "{}"),
        Err(refused.clone())
    );
    assert_eq!(page.call("Runtime.evaluate", "{}"), Err(refused.clone()));
    assert_eq!(browser.call("Browser.getVersion", "{}"), Err(refused));
    drop(page);
    drop(browser);
    fake.join().unwrap();
}

#[test]
fn eof_before_any_call_is_pipe_closed() {
    let (requests, browser_out) = pipe().expect("pipe");
    let (browser_in, out) = pipe().expect("pipe");
    drop(out);
    let browser = PipeBrowser::new(browser_in, browser_out);
    let err = browser.call("Browser.getVersion", "{}").unwrap_err();
    assert_eq!(
        err,
        CdpError::PipeClosed {
            message: "end of stream".into()
        }
    );
    assert_eq!(browser.call("Browser.getVersion", "{}"), Err(err));
    drop(requests);
}

#[test]
fn a_broken_write_end_is_pipe_closed_for_every_page() {
    let (browser, fake) = fake(|fake| {
        fake.attach("S1");
        // Chrome goes away: both ends close.
    });
    let mut page = browser.page("T1").expect("page");
    fake.join().unwrap();
    let err = page.call("Runtime.evaluate", "{}").unwrap_err();
    assert!(matches!(err, CdpError::PipeClosed { .. }), "{err:?}");
    assert_eq!(page.call("Runtime.evaluate", "{}"), Err(err.clone()));
    assert_eq!(browser.call("Browser.getVersion", "{}"), Err(err));
}

#[test]
fn eof_mid_call_and_mid_frame_is_pipe_closed() {
    let (browser, fake) = fake(|fake| {
        fake.attach("S1");
        let request = fake.expect("Runtime.evaluate");
        let frame = json!({"id": request["id"], "result": {}}).to_string();
        fake.raw(&frame.as_bytes()[..frame.len() / 2]);
    });
    let mut page = browser.page("T1").expect("page");
    assert_eq!(
        page.call("Runtime.evaluate", "{}"),
        Err(CdpError::PipeClosed {
            message: "end of stream".into()
        })
    );
    assert!(matches!(
        browser.call("Browser.getVersion", "{}"),
        Err(CdpError::PipeClosed { .. })
    ));
    fake.join().unwrap();
}

#[test]
fn a_silent_browser_times_out_at_the_deadline_and_the_pipe_stays_usable() {
    let (browser, fake) = fake_with(short_deadline(200), |fake| {
        let _ignored = fake.expect("Never.answered");
        // A late event and reply, then the next call's answer.
        let request = fake.expect("Then.answered");
        fake.send(json!({"id": 1, "result": {"late": true}}));
        fake.reply(&request, json!({"ok": true}));
    });
    let started = Instant::now();
    assert_eq!(
        browser.call("Never.answered", "{}"),
        Err(CdpError::Transport {
            message: "no CDP result for `Never.answered` within 200 ms".into()
        })
    );
    let waited = started.elapsed();
    assert!(waited >= Duration::from_millis(200), "{waited:?}");
    assert!(waited < Duration::from_secs(2), "{waited:?}");
    assert_eq!(
        browser.call("Then.answered", "{}"),
        Ok(r#"{"ok":true}"#.into())
    );
    fake.join().unwrap();
}

#[test]
fn a_dialog_on_the_calling_page_ends_its_wait_and_stays_queued() {
    let (browser, fake) = fake(|fake| {
        fake.attach("S1");
        fake.attach("S2");
        let press = fake.expect("Input.dispatchMouseEvent");
        assert_eq!(press["sessionId"], "S1");
        // Another page's dialog does not end this call's wait.
        fake.send(
            json!({"sessionId": "S2", "method": "Page.javascriptDialogOpening",
                         "params": {"type": "confirm"}}),
        );
        pause();
        fake.send(
            json!({"sessionId": "S1", "method": "Page.javascriptDialogOpening",
                         "params": {"type": "alert", "message": "hi"}}),
        );
        let handle = fake.expect("Page.handleJavaScriptDialog");
        fake.send(json!({"id": press["id"], "sessionId": "S1", "result": {"late": true}}));
        fake.send(
            json!({"sessionId": "S1", "method": "Page.javascriptDialogClosed",
                         "params": {"result": true}}),
        );
        fake.send(json!({"id": handle["id"], "sessionId": "S1", "result": {"handled": true}}));
        let _ = fake.recv();
    });
    let mut one = browser.page("T1").expect("page one");
    let mut two = browser.page("T2").expect("page two");
    let started = Instant::now();
    assert_eq!(
        one.call("Input.dispatchMouseEvent", "{}"),
        Err(CdpError::DialogOpened {
            method: "Input.dispatchMouseEvent".into()
        })
    );
    assert!(started.elapsed() < Duration::from_secs(3));
    let events = one.drain_events();
    assert_eq!(events.len(), 1, "{events:?}");
    assert_eq!(events[0].method, "Page.javascriptDialogOpening");
    assert_eq!(events[0].params["type"], "alert");
    assert_eq!(
        one.call("Page.handleJavaScriptDialog", r#"{"accept":true}"#),
        Ok(r#"{"handled":true}"#.into())
    );
    assert_eq!(one.drain_events()[0].method, "Page.javascriptDialogClosed");
    assert_eq!(two.drain_events()[0].params["type"], "confirm");
    drop((one, two, browser));
    fake.join().unwrap();
}

#[test]
fn a_detached_or_destroyed_target_fails_its_page_instead_of_hanging() {
    let (browser, fake) = fake(|fake| {
        fake.attach("S1");
        fake.attach("S2");
        let _waiting = fake.expect("Runtime.evaluate");
        fake.send(json!({"method": "Target.detachedFromTarget",
                         "params": {"sessionId": "S1", "targetId": "T1"}}));
        let request = fake.expect("Browser.getVersion");
        fake.send(json!({"method": "Target.targetDestroyed", "params": {"targetId": "T2"}}));
        fake.reply(&request, json!({}));
        // Neither gone page detaches on drop; the next read is EOF.
        assert!(fake.recv().is_none());
    });
    let mut one = browser.page("T1").expect("page one");
    let mut two = browser.page("T2").expect("page two");
    let started = Instant::now();
    let gone_one = CdpError::TargetGone {
        target_id: "T1".into(),
    };
    assert_eq!(one.call("Runtime.evaluate", "{}"), Err(gone_one.clone()));
    assert!(started.elapsed() < Duration::from_secs(3));
    assert_eq!(one.call("Runtime.evaluate", "{}"), Err(gone_one));
    // The browser call rides past the destroy event to its own answer.
    assert_eq!(browser.call("Browser.getVersion", "{}"), Ok("{}".into()));
    assert_eq!(
        two.call("Runtime.evaluate", "{}"),
        Err(CdpError::TargetGone {
            target_id: "T2".into()
        })
    );
    let methods: Vec<_> = browser
        .drain_events()
        .into_iter()
        .map(|e| e.method)
        .collect();
    assert_eq!(
        methods,
        ["Target.detachedFromTarget", "Target.targetDestroyed"]
    );
    drop((one, two, browser));
    fake.join().unwrap();
}

#[test]
fn target_helpers_send_the_target_domain_calls() {
    let (browser, fake) = fake(|fake| {
        let request = fake.expect("Target.getTargets");
        fake.reply(
            &request,
            json!({"targetInfos": [
                {"targetId": "SW", "type": "service_worker", "url": "sw.js", "title": ""},
                {"targetId": "T1", "type": "page", "url": "about:blank", "title": "Blank"},
                {"type": "page", "url": "no-id"},
                {"targetId": "T2", "type": "page"}
            ]}),
        );
        let request = fake.expect("Target.createTarget");
        assert_eq!(request["params"]["url"], "https://example.test/");
        fake.reply(&request, json!({"targetId": "T3"}));
        let request = fake.expect("Target.activateTarget");
        assert_eq!(request["params"]["targetId"], "T3");
        fake.reply(&request, json!({}));
        let request = fake.expect("Target.closeTarget");
        assert_eq!(request["params"]["targetId"], "T3");
        fake.reply(&request, json!({"success": true}));
        let request = fake.expect("Target.attachToTarget");
        fake.reply(&request, json!({}));
    });
    assert_eq!(
        browser.targets(),
        Ok(vec![
            PipeTarget {
                target_id: "T1".into(),
                target_type: "page".into(),
                url: "about:blank".into(),
                title: "Blank".into(),
            },
            PipeTarget {
                target_id: "T2".into(),
                target_type: "page".into(),
                url: String::new(),
                title: String::new(),
            },
        ])
    );
    assert_eq!(
        browser.create_target("https://example.test/"),
        Ok("T3".into())
    );
    assert_eq!(browser.activate_target("T3"), Ok(()));
    assert_eq!(browser.close_target("T3"), Ok(true));
    assert!(matches!(
        browser.page("T4"),
        Err(CdpError::Transport { .. })
    ));
    fake.join().unwrap();
}

/// Random chunking of `frames` NUL-joined, as raw byte chunks.
fn chunked(frames: &[Vec<u8>], cuts: &[usize]) -> Vec<Vec<u8>> {
    let stream: Vec<u8> = frames
        .iter()
        .flat_map(|frame| frame.iter().copied().chain([0]))
        .collect();
    let mut points: Vec<usize> = cuts.iter().map(|cut| cut % (stream.len() + 1)).collect();
    points.extend([0, stream.len()]);
    points.sort_unstable();
    points.dedup();
    points
        .windows(2)
        .map(|w| stream[w[0]..w[1]].to_vec())
        .collect()
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(256))]

    #[test]
    fn any_chunking_of_a_frame_stream_yields_the_same_frames(
        frames in prop::collection::vec(prop::collection::vec(1u8..=255, 0..64), 0..16),
        cuts in prop::collection::vec(any::<usize>(), 0..24),
    ) {
        let mut splitter = FrameSplitter::new(64);
        let mut got = Vec::new();
        for chunk in chunked(&frames, &cuts) {
            splitter.push(&chunk);
            while let Some(frame) = splitter.next_frame().expect("within the cap") {
                got.push(frame);
            }
        }
        prop_assert_eq!(got, frames);
        prop_assert_eq!(splitter.buffered(), 0);
    }

    #[test]
    fn a_frame_over_the_cap_fails_closed_under_any_chunking(
        len in 9usize..64,
        cuts in prop::collection::vec(any::<usize>(), 0..8),
    ) {
        let mut splitter = FrameSplitter::new(8);
        let mut failed = false;
        for chunk in chunked(&[vec![b'a'; len], b"ok".to_vec()], &cuts) {
            splitter.push(&chunk);
            loop {
                match splitter.next_frame() {
                    Ok(Some(frame)) => prop_assert!(false, "frame {:?} leaked", frame),
                    Ok(None) => break,
                    Err(err) => {
                        prop_assert_eq!(err, CdpError::FrameTooLarge { limit: 8 });
                        failed = true;
                        break;
                    }
                }
            }
        }
        prop_assert!(failed);
        prop_assert!(splitter.next_frame().is_err());
    }
}
