//! Live Chrome checks for the borrowed shell verbs (ADR 0012). Ignored by
//! default (not in CI; CI stays on CDP replay).
//!
//! ```text
//! aui doctor        # finds Chrome (ULTRA_INSTINCT_CHROME picks one)
//! cargo test -p ultra-instinct --test live_borrow -- --ignored --nocapture --test-threads=1
//! ```
//!
//! Each test drives the real `aui` binary: `aui open --launch` with its own
//! `--session`, so every test gets its own headless Chrome and temp profile
//! (never the user's profile), and `aui close` runs at the end, also on
//! failure (a drop guard). Pages come from a tiny HTTP server on
//! 127.0.0.1 inside this file (or `data:` URLs); no other site is visited.
//! Every check reads what the page or the server saw, not only the verb's
//! own reply: page facts go into `document.title`, which `aui snap` prints.
#![cfg(unix)]

use std::collections::BTreeMap;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde_json::Value;

fn aui() -> &'static str {
    env!("CARGO_BIN_EXE_aui")
}

/// `aui <args>`: (exit ok, stdout + stderr).
fn run(args: &[&str], stdin: Option<&str>) -> (bool, String) {
    let mut command = Command::new(aui());
    command
        .args(args)
        .stdin(if stdin.is_some() {
            Stdio::piped()
        } else {
            Stdio::null()
        })
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = command.spawn().expect("spawn aui");
    if let Some(input) = stdin {
        child
            .stdin
            .take()
            .expect("stdin")
            .write_all(input.as_bytes())
            .expect("write stdin");
    }
    let out = child.wait_with_output().expect("aui output");
    let text = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    (out.status.success(), text)
}

/// A test's scratch directory (removed on drop).
struct Scratch(PathBuf);

impl Scratch {
    fn new(tag: &str) -> Self {
        let dir = std::env::temp_dir().join(format!("aui-live-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("scratch dir");
        Self(dir)
    }

    fn path(&self, name: &str) -> PathBuf {
        self.0.join(name)
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// One `aui` session; `aui close` on drop, pass or fail.
struct Session {
    name: String,
}

impl Session {
    fn new(tag: &str) -> Self {
        let name = format!("lb-{tag}-{}", std::process::id());
        assert!(name.len() <= 32, "session name too long: {name}");
        // A leftover daemon from an aborted run would make `open --launch` refuse.
        let _ = run(&["close", "--session", &name], None);
        Self { name }
    }

    /// `aui <verb> <args> --session <name>`.
    fn try_verb(&self, args: &[&str]) -> (bool, String) {
        let mut all = args.to_vec();
        all.extend(["--session", &self.name]);
        run(&all, None)
    }

    /// Like [`Session::try_verb`], panicking on a non-zero exit.
    fn verb(&self, args: &[&str]) -> String {
        let (ok, out) = self.try_verb(args);
        assert!(ok, "aui {args:?} failed: {out}");
        out
    }

    /// `aui open --launch <url> [extra]`.
    fn launch(&self, url: &str, extra: &[&str]) -> String {
        let mut args = vec!["open", "--launch", url];
        args.extend(extra);
        let out = self.verb(&args);
        assert!(out.contains("daemon started with its own Chrome"), "{out}");
        out
    }

    fn socket(&self) -> PathBuf {
        std::env::temp_dir()
            .join("aui")
            .join(format!("{}.sock", self.name))
    }

    /// The page title, from the first `aui snap` line (`title  url`).
    fn title(&self) -> String {
        let snap = self.verb(&["snap"]);
        let first = snap.lines().next().unwrap_or_default();
        match first.rfind("  ") {
            Some(cut) => first[..cut].to_owned(),
            None => first.to_owned(),
        }
    }

    /// Poll the title until `want` holds (10 s), returning it.
    fn title_until(&self, what: &str, want: impl Fn(&str) -> bool) -> String {
        let started = Instant::now();
        loop {
            let title = self.title();
            if want(&title) {
                return title;
            }
            assert!(
                started.elapsed() < Duration::from_secs(10),
                "title never showed {what}; last {title:?}"
            );
            std::thread::sleep(Duration::from_millis(150));
        }
    }

    /// The `@eN` of the control whose label is `label` in a fresh snap.
    fn reference(&self, label: &str) -> String {
        let snap = self.verb(&["snap"]);
        let quoted = format!("\"{label}\"");
        snap.lines()
            .find(|line| line.starts_with("@e") && line.contains(&quoted))
            .and_then(|line| line.split_whitespace().next())
            .unwrap_or_else(|| panic!("no control {quoted} in:\n{snap}"))
            .to_owned()
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        let _ = run(&["close", "--session", &self.name], None);
    }
}

// ---- tiny local HTTP server -------------------------------------------------

#[derive(Clone, Debug)]
struct Seen {
    path: String,
    /// Lowercased header name → value.
    headers: BTreeMap<String, String>,
}

type Route = fn(&str) -> Option<(&'static str, Vec<u8>)>;

/// `127.0.0.1:<port>`, one thread per connection, `Connection: close`.
struct Server {
    base: String,
    seen: Arc<Mutex<Vec<Seen>>>,
}

impl Server {
    fn start(route: Route) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
        let base = format!("http://{}", listener.local_addr().expect("addr"));
        let seen = Arc::new(Mutex::new(Vec::new()));
        let log = Arc::clone(&seen);
        std::thread::spawn(move || {
            for stream in listener.incoming().flatten() {
                let log = Arc::clone(&log);
                std::thread::spawn(move || serve(stream, route, &log));
            }
        });
        Self { base, seen }
    }

    fn url(&self, path: &str) -> String {
        format!("{}{path}", self.base)
    }

    /// Requests whose path starts with `prefix`, in arrival order.
    fn requests(&self, prefix: &str) -> Vec<Seen> {
        self.seen
            .lock()
            .expect("log")
            .iter()
            .filter(|seen| seen.path.starts_with(prefix))
            .cloned()
            .collect()
    }
}

fn serve(stream: TcpStream, route: Route, log: &Mutex<Vec<Seen>>) {
    let _ = stream.set_read_timeout(Some(Duration::from_secs(5)));
    let mut writer = match stream.try_clone() {
        Ok(writer) => writer,
        Err(_) => return,
    };
    let mut reader = BufReader::new(stream);
    let mut line = String::new();
    if reader.read_line(&mut line).unwrap_or(0) == 0 {
        return; // a preconnect that never sent a request
    }
    let path = line.split_whitespace().nth(1).unwrap_or("/").to_owned();
    let mut headers = BTreeMap::new();
    let mut length = 0usize;
    loop {
        let mut header = String::new();
        if reader.read_line(&mut header).unwrap_or(0) == 0 || header.trim().is_empty() {
            break;
        }
        if let Some((name, value)) = header.trim_end().split_once(':') {
            let name = name.trim().to_ascii_lowercase();
            if name == "content-length" {
                length = value.trim().parse().unwrap_or(0);
            }
            headers.insert(name, value.trim().to_owned());
        }
    }
    let mut body = vec![0u8; length.min(1 << 20)];
    let _ = reader.read_exact(&mut body);
    log.lock().expect("log").push(Seen {
        path: path.clone(),
        headers,
    });
    let (status, kind, body) = match route(&path) {
        Some((kind, body)) => ("200 OK", kind, body),
        None => ("404 Not Found", "text/plain", b"not found".to_vec()),
    };
    let mut head = format!(
        "HTTP/1.1 {status}\r\nContent-Type: {kind}\r\nContent-Length: {}\r\nCache-Control: no-store\r\nConnection: close\r\n",
        body.len()
    );
    if kind == "application/octet-stream" {
        head.push_str("Content-Disposition: attachment; filename=\"payload.bin\"\r\n");
    }
    head.push_str("\r\n");
    let _ = writer.write_all(head.as_bytes());
    let _ = writer.write_all(&body);
}

fn html(body: &str) -> Option<(&'static str, Vec<u8>)> {
    Some(("text/html; charset=utf-8", body.as_bytes().to_vec()))
}

/// The bytes `/payload.bin` serves: every byte value, three times.
fn payload() -> Vec<u8> {
    (0..768u32).map(|i| (i % 256) as u8).collect()
}

/// Shared pages. Facts the page sees are written into its title.
fn pages(path: &str) -> Option<(&'static str, Vec<u8>)> {
    let path = path.split('?').next().unwrap_or(path);
    match path {
        "/" | "/blank" => html("<!doctype html><title>blank</title><p>blank</p>"),
        "/state" => html(
            "<!doctype html><title>state</title><script>\
             document.title='cookie='+document.cookie+'|ls='+localStorage.getItem('borrow-key');\
             </script>",
        ),
        "/env" => html(
            "<!doctype html><title>env</title>\
             <meta name=viewport content=\"width=device-width,initial-scale=1\">\
             <button onclick=\"fetch('/ping?'+Date.now()).then(function(r){document.title='ping:'+r.status}).catch(function(){document.title='ping:fail'})\">Ping</button>\
             <script>document.title='w='+innerWidth+'|iphone='+/iPhone/.test(navigator.userAgent)\
             +'|ualen='+navigator.userAgent.length+'|sw='+screen.width+'|dark='+matchMedia('(prefers-color-scheme: dark)').matches;</script>",
        ),
        "/ping" | "/blocked/data" | "/open/data" => Some(("text/plain", b"pong".to_vec())),
        "/net" => html(
            "<!doctype html><title>net</title>\
             <button onclick=\"fetch('/blocked/data?'+Date.now()).then(function(r){document.title='blocked:'+r.status}).catch(function(){document.title='blocked:fail'})\">Fetch blocked</button>\
             <button onclick=\"fetch('/open/data?'+Date.now()).then(function(r){document.title='open:'+r.status}).catch(function(){document.title='open:fail'})\">Fetch open</button>",
        ),
        "/download" => html(
            "<!doctype html><title>download</title>\
             <a href=\"/payload.bin\" download=\"payload.bin\">Get payload</a>",
        ),
        "/payload.bin" => Some(("application/octet-stream", payload())),
        "/batch1" | "/batch3" => html("<!doctype html><title>batch</title>"),
        "/batch2" => html(
            "<!doctype html><title>batch two</title>\
             <button onclick=\"document.title='clicked in batch'\">Mark</button>",
        ),
        _ => None,
    }
}

// ---- process checks ---------------------------------------------------------

/// `ps` command lines, one per process.
fn processes() -> Vec<(u32, String)> {
    let out = Command::new("ps")
        .args(["-axww", "-o", "pid=,command="])
        .output()
        .expect("ps");
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .filter_map(|line| {
            let line = line.trim_start();
            let (pid, command) = line.split_once(' ')?;
            Some((pid.parse().ok()?, command.trim().to_owned()))
        })
        .collect()
}

/// The session's Chrome: (main pid, its command line, its profile dir).
/// The daemon names the temp profile after its own pid.
fn chrome_of(session: &Session) -> (u32, String, PathBuf) {
    let all = processes();
    let daemon_marker = format!("daemon --session {}", session.name);
    let (daemon, _) = all
        .iter()
        .find(|(_, command)| command.contains(&daemon_marker))
        .unwrap_or_else(|| panic!("no daemon process for {}", session.name));
    let profile_marker = format!("ultra-instinct-chrome-{daemon}-");
    let (pid, command) = all
        .iter()
        .find(|(_, command)| {
            command.contains(&profile_marker)
                && command.contains("--remote-debugging-port=0")
                && !command.contains("--type=")
        })
        .unwrap_or_else(|| panic!("no Chrome with {profile_marker}"));
    let profile = command
        .split(" --")
        .find_map(|arg| arg.strip_prefix("user-data-dir="))
        .map(PathBuf::from)
        .expect("--user-data-dir");
    (*pid, command.clone(), profile)
}

/// Processes (main or helper) still using `profile`.
fn using_profile(profile: &Path) -> Vec<u32> {
    let marker = profile.display().to_string();
    processes()
        .into_iter()
        .filter(|(_, command)| command.contains(&marker))
        .map(|(pid, _)| pid)
        .collect()
}

/// Poll `done` for up to `limit`.
fn eventually(limit: Duration, what: &str, done: impl Fn() -> bool) {
    let started = Instant::now();
    while !done() {
        assert!(started.elapsed() < limit, "not within {limit:?}: {what}");
        std::thread::sleep(Duration::from_millis(100));
    }
}

fn mode(path: &Path) -> u32 {
    std::fs::metadata(path)
        .unwrap_or_else(|err| panic!("{}: {err}", path.display()))
        .permissions()
        .mode()
        & 0o777
}

fn field<'a>(title: &'a str, key: &str) -> &'a str {
    title
        .split('|')
        .find_map(|part| part.strip_prefix(&format!("{key}=")))
        .unwrap_or_else(|| panic!("no {key} in title {title:?}"))
}

// ---- tests ------------------------------------------------------------------

#[test]
#[ignore = "needs a local Chrome (aui open --launch)"]
fn state_save_then_load_into_a_fresh_chrome() {
    let server = Server::start(pages);
    let scratch = Scratch::new("state");
    let file = scratch.path("state.json");
    let file_arg = file.display().to_string();
    {
        let first = Session::new("state1");
        first.launch(&server.url("/blank"), &[]);
        first.verb(&["cookies", "set", "borrow_sid", "abc123"]);
        first.verb(&["storage", "local", "set", "borrow-key", "kept-value"]);
        let out = first.verb(&["state", "save", &file_arg]);
        println!("{}", out.trim());
        assert!(
            out.contains("1 cookies") || out.contains("cookies"),
            "{out}"
        );
        assert_eq!(mode(&file), 0o600, "state file is private");
        let text = std::fs::read_to_string(&file).expect("state file");
        assert!(
            text.contains("borrow_sid") && text.contains("kept-value"),
            "{text}"
        );
        first.verb(&["close"]);
    }
    let second = Session::new("state2");
    second.launch(&server.url("/blank"), &[]);
    // A fresh profile: nothing carried over yet.
    second.verb(&["open", &server.url("/state")]);
    assert_eq!(second.title(), "cookie=|ls=null");
    let out = second.verb(&["state", "load", &file_arg]);
    println!("{}", out.trim());
    let before = server.requests("/state").len();
    second.verb(&["open", &server.url("/state")]);
    assert_eq!(second.title(), "cookie=borrow_sid=abc123|ls=kept-value");
    let sent = server.requests("/state");
    assert!(sent.len() > before, "the reload reached the server");
    assert_eq!(
        sent.last()
            .unwrap()
            .headers
            .get("cookie")
            .map(String::as_str),
        Some("borrow_sid=abc123"),
        "the server receives the loaded cookie"
    );
}

#[test]
#[ignore = "needs a local Chrome (aui open --launch)"]
fn cookies_set_list_and_clear() {
    let server = Server::start(pages);
    let session = Session::new("cookies");
    session.launch(&server.url("/blank"), &[]);
    let out = session.verb(&["cookies", "set", "flavor", "oat"]);
    println!("{}", out.trim());
    let listed = session.verb(&["cookies"]);
    assert!(
        listed.contains("flavor=") && !listed.contains("oat"),
        "masked: {listed}"
    );
    let revealed = session.verb(&["cookies", "--reveal"]);
    assert!(revealed.contains("flavor=oat"), "{revealed}");
    session.verb(&["open", &server.url("/state?with")]);
    assert_eq!(field(&session.title(), "cookie"), "flavor=oat");
    let with = server.requests("/state?with");
    assert_eq!(
        with.last()
            .and_then(|s| s.headers.get("cookie"))
            .map(String::as_str),
        Some("flavor=oat")
    );
    let out = session.verb(&["cookies", "clear"]);
    println!("{}", out.trim());
    session.verb(&["open", &server.url("/state?without")]);
    assert_eq!(field(&session.title(), "cookie"), "");
    let without = server.requests("/state?without");
    assert_eq!(
        without.last().and_then(|s| s.headers.get("cookie")),
        None,
        "no Cookie header after clear"
    );
}

#[test]
#[ignore = "needs a local Chrome (aui open --launch)"]
fn set_device_viewport_media_offline_headers_and_reset() {
    let server = Server::start(pages);
    let session = Session::new("set");
    session.launch(&server.url("/env"), &[]);
    let base = session.title();
    println!("baseline {base}");
    assert_eq!(field(&base, "iphone"), "false");
    assert_eq!(field(&base, "dark"), "false");

    session.verb(&["set", "viewport", "500", "700"]);
    session.verb(&["reload"]);
    assert_eq!(field(&session.title(), "w"), "500");

    session.verb(&["set", "device", "iPhone 15"]);
    session.verb(&["reload"]);
    let phone = session.title();
    // A mobile layout viewport is the device width only with a
    // `width=device-width` meta (the page has one); else Chrome uses 980.
    assert_eq!(field(&phone, "w"), "393", "{phone}");
    assert_eq!(field(&phone, "sw"), "393", "{phone}");
    assert_eq!(field(&phone, "iphone"), "true", "{phone}");

    session.verb(&["set", "media", "dark"]);
    session.verb(&["reload"]);
    assert_eq!(field(&session.title(), "dark"), "true");

    session.verb(&["set", "headers", "X-Borrow-Live=yes-1"]);
    session.verb(&["open", &server.url("/env?headers")]);
    let sent = server.requests("/env?headers");
    assert_eq!(
        sent.last()
            .and_then(|s| s.headers.get("x-borrow-live"))
            .map(String::as_str),
        Some("yes-1"),
        "the custom header reaches the server"
    );

    let pings = server.requests("/ping").len();
    session.verb(&["set", "offline", "on"]);
    let ping = session.reference("Ping");
    session.verb(&["click", &ping]);
    assert_eq!(
        session.title_until("ping:*", |t| t.starts_with("ping:")),
        "ping:fail"
    );
    assert_eq!(
        server.requests("/ping").len(),
        pings,
        "offline: nothing sent"
    );
    session.verb(&["set", "offline", "off"]);
    session.verb(&["open", &server.url("/env?online")]);
    let ping = session.reference("Ping");
    session.verb(&["click", &ping]);
    assert_eq!(
        session.title_until("ping:*", |t| t.starts_with("ping:")),
        "ping:200"
    );
    assert_eq!(server.requests("/ping").len(), pings + 1);

    let out = session.verb(&["set", "reset"]);
    println!("{}", out.trim());
    session.verb(&["open", &server.url("/env?reset")]);
    let reset = session.title();
    for key in ["w", "iphone", "ualen", "sw", "dark"] {
        assert_eq!(
            field(&reset, key),
            field(&base, key),
            "{key} after reset: {reset}"
        );
    }
    let sent = server.requests("/env?reset");
    assert_eq!(
        sent.last().and_then(|s| s.headers.get("x-borrow-live")),
        None,
        "reset drops the extra header"
    );
}

#[test]
#[ignore = "needs a local Chrome (aui open --launch)"]
fn download_lands_in_the_target_with_the_served_bytes() {
    let server = Server::start(pages);
    let scratch = Scratch::new("download");
    let target = scratch.path("got.bin");
    let session = Session::new("download");
    session.launch(&server.url("/download"), &[]);
    let link = session.reference("Get payload");
    let out = session.verb(&["download", &link, &target.display().to_string()]);
    println!("{}", out.trim());
    assert!(out.starts_with("✓ saved"), "{out}");
    assert_eq!(std::fs::read(&target).expect("downloaded file"), payload());
    assert_eq!(server.requests("/payload.bin").len(), 1);
}

#[test]
#[ignore = "needs a local Chrome (aui open --launch)"]
fn network_block_unblock_and_a_redacted_har() {
    let server = Server::start(pages);
    let scratch = Scratch::new("network");
    let session = Session::new("network");
    session.launch(&server.url("/net"), &[]);

    let out = session.verb(&["network", "block", "*/blocked/*"]);
    println!("{}", out.trim());
    let blocked = session.reference("Fetch blocked");
    let open = session.reference("Fetch open");
    session.verb(&["click", &blocked]);
    assert_eq!(
        session.title_until("blocked:*", |t| t.starts_with("blocked:")),
        "blocked:fail"
    );
    assert!(
        server.requests("/blocked/").is_empty(),
        "a blocked request never reaches the server"
    );
    session.verb(&["click", &open]);
    assert_eq!(
        session.title_until("open:*", |t| t.starts_with("open:")),
        "open:200"
    );
    session.verb(&["network", "unblock"]);
    session.verb(&["click", &blocked]);
    assert_eq!(
        session.title_until("blocked:200", |t| t == "blocked:200"),
        "blocked:200"
    );
    assert_eq!(server.requests("/blocked/").len(), 1);

    session.verb(&["network", "har", "start"]);
    session.verb(&["cookies", "set", "har_sid", "cookie-secret-1"]);
    session.verb(&["set", "headers", "Authorization=Bearer header-secret-2"]);
    session.verb(&["open", &server.url("/net?har")]);
    let open = session.reference("Fetch open");
    session.verb(&["click", &open]);
    session.title_until("open:200", |t| t == "open:200");
    let har = scratch.path("run.har");
    let out = session.verb(&["network", "har", "stop", &har.display().to_string()]);
    println!("{}", out.trim());
    assert_eq!(mode(&har), 0o600, "HAR file is private");
    let text = std::fs::read_to_string(&har).expect("har");
    assert!(
        !text.contains("cookie-secret-1") && !text.contains("header-secret-2"),
        "secrets never reach the HAR"
    );
    let value: Value = serde_json::from_str(&text).expect("HAR json");
    let entries = value["log"]["entries"].as_array().expect("entries");
    let urls: Vec<&str> = entries
        .iter()
        .filter_map(|e| e["request"]["url"].as_str())
        .collect();
    assert!(
        urls.iter().any(|u| u.ends_with("/net?har"))
            && urls.iter().any(|u| u.contains("/open/data")),
        "{urls:?}"
    );
    for entry in entries {
        assert!(
            entry["response"]["content"].get("text").is_none(),
            "no bodies: {entry}"
        );
        for header in entry["request"]["headers"].as_array().into_iter().flatten() {
            let name = header["name"]
                .as_str()
                .unwrap_or_default()
                .to_ascii_lowercase();
            if name == "cookie" || name == "authorization" {
                assert_eq!(header["value"], "•••", "{header}");
            }
        }
    }
    let page = entries
        .iter()
        .find(|e| {
            e["request"]["url"]
                .as_str()
                .is_some_and(|u| u.ends_with("/net?har"))
        })
        .expect("page entry");
    let names: Vec<String> = page["request"]["headers"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|h| h["name"].as_str().map(str::to_ascii_lowercase))
        .collect();
    // Authorization comes from `requestWillBeSent`; Cookie only from
    // `requestWillBeSentExtraInfo`, which the HAR merges in. Both are
    // listed, and the loop above proves both are masked.
    assert!(
        names.iter().any(|n| n == "authorization"),
        "the authorization header is listed (masked): {names:?}"
    );
    assert!(
        names.iter().any(|n| n == "cookie"),
        "the cookie header is listed (masked): {names:?}"
    );
    // The server did get the real values.
    let sent = server.requests("/net?har");
    let last = sent.last().expect("har page request");
    assert_eq!(
        last.headers.get("cookie").map(String::as_str),
        Some("har_sid=cookie-secret-1")
    );
    assert_eq!(
        last.headers.get("authorization").map(String::as_str),
        Some("Bearer header-secret-2")
    );
}

#[test]
#[ignore = "needs a local Chrome (aui open --launch)"]
fn a_click_that_opens_alert_returns_at_once_with_the_hint() {
    let page = "data:text/html,<title>before</title><button onclick=\"alert('saved');document.title='after alert'\">Save</button>";
    let session = Session::new("dialog");
    session.launch(page, &[]);
    let save = session.reference("Save");
    let started = Instant::now();
    let (ok, out) = session.try_verb(&["click", &save]);
    let took = started.elapsed();
    println!("click took {took:?}: {}", out.trim());
    assert!(!ok, "the press reports the dialog: {out}");
    assert!(
        out.contains("a alert dialog is open; call browser_dialog"),
        "{out}"
    );
    assert!(took < Duration::from_secs(3), "took {took:?}");
    let status = session.verb(&["dialog"]);
    assert!(
        status.contains("alert dialog open") && status.contains("saved"),
        "{status}"
    );
    session.verb(&["dialog", "accept"]);
    assert_eq!(
        session.title_until("after alert", |t| t == "after alert"),
        "after alert"
    );
    assert!(session.verb(&["dialog"]).contains("no dialog open"));
}

#[test]
#[ignore = "needs a local Chrome (aui open --launch)"]
fn close_ends_chrome_and_removes_its_temp_profile() {
    let (_, doctor) = run(&["doctor", "--quick"], None);
    println!("{}", doctor.lines().next().unwrap_or_default());
    let session = Session::new("life");
    session.launch("data:text/html,<title>life</title>", &[]);
    let (pid, command, profile) = chrome_of(&session);
    println!("chrome pid {pid}, profile {}", profile.display());
    assert!(command.contains("--headless=new"), "{command}");
    assert!(profile.is_dir());
    assert!(session.socket().exists());
    assert_eq!(session.verb(&["close"]).trim(), "✓ daemon stopped");
    eventually(Duration::from_secs(10), "Chrome exits", || {
        using_profile(&profile).is_empty()
    });
    eventually(Duration::from_secs(5), "profile removed", || {
        !profile.exists()
    });
    assert!(!session.socket().exists(), "socket removed");
}

#[test]
#[ignore = "needs a local Chrome (aui open --launch)"]
fn idle_exit_ends_the_daemon_chrome_and_profile() {
    let session = Session::new("idle");
    session.launch("data:text/html,<title>idle</title>", &["--idle", "3"]);
    let (pid, _, profile) = chrome_of(&session);
    session.verb(&["snap"]);
    let last = Instant::now();
    println!("chrome pid {pid}, waiting for the 3 s idle exit");
    eventually(Duration::from_secs(15), "the socket goes away", || {
        !session.socket().exists()
    });
    let idle_for = last.elapsed();
    println!("daemon gone {idle_for:?} after the last verb");
    assert!(idle_for >= Duration::from_millis(2500), "{idle_for:?}");
    eventually(Duration::from_secs(10), "Chrome exits", || {
        using_profile(&profile).is_empty()
    });
    eventually(Duration::from_secs(5), "profile removed", || {
        !profile.exists()
    });
    // A running session refuses --idle.
    let again = Session::new("idle2");
    again.launch("data:text/html,<title>x</title>", &[]);
    let (ok, out) = again.try_verb(&["open", "data:text/html,y", "--idle", "5"]);
    assert!(!ok && out.contains("already running"), "{out}");
}

#[test]
#[ignore = "needs a local Chrome (aui open --launch)"]
fn headed_launch_has_no_headless_flag_and_takes_a_shot() {
    let scratch = Scratch::new("headed");
    let session = Session::new("headed");
    session.launch(
        "data:text/html,<title>headed</title><h1>Headed</h1>",
        &["--headed"],
    );
    let (_, command, _) = chrome_of(&session);
    assert!(!command.contains("--headless"), "{command}");
    let shot = scratch.path("headed.png");
    let out = session.verb(&["shot", &shot.display().to_string()]);
    let bytes = std::fs::read(&shot).expect("screenshot file");
    assert!(bytes.starts_with(b"\x89PNG"), "a PNG");
    // Kept for a human to look at; the scratch dir is removed at the end.
    let keep = std::env::temp_dir().join(format!("aui-live-headed-{}.png", std::process::id()));
    std::fs::copy(&shot, &keep).expect("copy shot");
    println!("{} · screenshot copied to {}", out.trim(), keep.display());
}

#[test]
#[ignore = "needs a local Chrome (aui open --launch)"]
fn stdin_batch_runs_commands_in_order() {
    let server = Server::start(pages);
    let session = Session::new("batch");
    session.launch(&server.url("/blank"), &[]);
    let commands = serde_json::json!([
        format!("open {}", server.url("/batch1")),
        format!("open {}", server.url("/batch2")),
        "click @e1",
        "snap",
        format!("open {}", server.url("/batch3")),
    ])
    .to_string();
    let (ok, out) = run(
        &["batch", "--json", "--session", &session.name],
        Some(&commands),
    );
    println!("{out}");
    assert!(ok, "{out}");
    let results: Value = serde_json::from_str(out.trim()).expect("batch json");
    let results = results.as_array().expect("array");
    assert_eq!(results.len(), 5);
    assert!(results.iter().all(|r| r["ok"] == true), "{out}");
    assert!(
        results[3]["output"]
            .as_str()
            .unwrap_or_default()
            .starts_with("clicked in batch"),
        "the snap after the click sees it: {out}"
    );
    let order: Vec<String> = server
        .requests("/batch")
        .into_iter()
        .map(|seen| seen.path)
        .collect();
    assert_eq!(order, ["/batch1", "/batch2", "/batch3"]);
}
