//! Shell verbs: a shell agent drives one open browser with short `aui`
//! commands.
//!
//! ```text
//! aui open https://shop.example/join      # spawns the session daemon if needed
//! aui snap                                # the page, one line per control
//! aui type @e2 "kid@example.com"
//! aui click @e5                           # ✓ ticket 41 · navigation
//! aui wait --text "Welcome" --timeout 10
//! aui close                               # stops the daemon
//! ```
//!
//! One session is one daemon (`aui daemon --session <name>`, hidden) serving
//! `$TMPDIR/aui/<session>.sock` (directory 0700, socket 0600). It answers one
//! connection and one line at a time, in a single thread: each line is a
//! JSON-RPC request for one [`ultra_instinct_mcp::Server`], so a session owns
//! one CDP socket (anti-drift #10). Every press goes through the existing
//! ticketed `browser_*` tools. The daemon exits after 60 minutes idle or on
//! `aui close`, and removes its socket.
//!
//! A verb whose socket is missing or stale spawns the daemon (this binary,
//! detached, null stdio) and waits up to 3 s for the socket. `aui open` does
//! not launch Chrome: the daemon attaches to `--cdp` (default
//! `http://127.0.0.1:9222`). `--allow-domain` / `--confirm-press` on `open`
//! reach the spawned daemon as `ULTRA_INSTINCT_ALLOW_DOMAINS` /
//! `ULTRA_INSTINCT_CONFIRM_PRESS`; `--confirm <label>` on a press verb is
//! the tool's `confirm` argument.
//!
//! Unix only (std `UnixListener` / `UnixStream`); elsewhere every verb is a
//! clear error.
#![cfg_attr(not(unix), allow(dead_code))]

use serde_json::{json, Value};

use crate::CliError;

/// Shell verbs, in usage order.
pub const VERBS: [&str; 11] = [
    "open", "snap", "click", "type", "select", "key", "scroll", "wait", "shot", "tabs", "close",
];

/// The session a verb or daemon names when `--session` is not given.
pub const DEFAULT_SESSION: &str = "default";

/// Longest session name: the socket path must fit `sun_path`.
const MAX_SESSION_CHARS: usize = 32;

/// JSON-RPC method that stops the daemon (answered by the daemon itself).
const STOP_METHOD: &str = "aui/stop";

/// `--session` value: ASCII letters, digits and `-`, at most 32 chars.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SessionName(String);

impl SessionName {
    pub fn parse(raw: &str) -> Result<Self, CliError> {
        let ok = !raw.is_empty()
            && raw.len() <= MAX_SESSION_CHARS
            && raw.chars().all(|c| c.is_ascii_alphanumeric() || c == '-');
        if !ok {
            return Err(CliError::Config(format!(
                "session `{raw}`: use 1-{MAX_SESSION_CHARS} ASCII letters, digits or `-`"
            )));
        }
        Ok(Self(raw.to_owned()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// What one verb asks the daemon for.
#[derive(Clone, Debug, PartialEq)]
pub enum Request {
    /// `tools/call` of one `browser_*` tool.
    Tool {
        name: &'static str,
        arguments: Value,
    },
    /// Stop the daemon.
    Stop,
}

/// One parsed verb: the session, its request, and the spawn settings.
#[derive(Clone, Debug, PartialEq)]
pub struct Verb {
    pub verb: &'static str,
    pub session: SessionName,
    pub cdp: String,
    pub request: Request,
    /// `--allow-domain` values (`open` only), for the spawned daemon.
    pub allow_domains: Vec<String>,
    /// `--confirm-press` values (`open` only), for the spawned daemon.
    pub confirm_press: Vec<String>,
}

/// Parse `aui <verb> …` (`args` excludes the verb).
pub fn parse_verb(verb: &str, args: &[String]) -> Result<Verb, CliError> {
    let verb: &'static str = VERBS
        .iter()
        .copied()
        .find(|v| *v == verb)
        .ok_or_else(|| CliError::UnknownCommand(verb.to_owned()))?;
    let mut session = None;
    let mut cdp = None;
    let mut allow_domains = Vec::new();
    let mut confirm_press = Vec::new();
    let mut confirm = None;
    let mut json_out = false;
    let mut mouse = false;
    let mut timeout = None;
    let mut wait = serde_json::Map::new();
    let mut positional = Vec::new();
    let mut i = 0;
    while i < args.len() {
        let arg = args[i].as_str();
        let (name, inline) = match arg.split_once('=') {
            Some((n, v)) if n.starts_with("--") => (n, Some(v.to_owned())),
            _ => (arg, None),
        };
        let mut value = |flag: &'static str| -> Result<String, CliError> {
            if let Some(v) = inline.clone() {
                return Ok(v);
            }
            i += 1;
            args.get(i).cloned().ok_or(CliError::MissingValue(flag))
        };
        match name {
            "--session" => set(&mut session, "--session", value("--session")?)?,
            "--cdp" => set(&mut cdp, "--cdp", value("--cdp")?)?,
            "--allow-domain" if verb == "open" => allow_domains.push(value("--allow-domain")?),
            "--confirm-press" if verb == "open" => confirm_press.push(value("--confirm-press")?),
            "--confirm" if verb == "click" => set(&mut confirm, "--confirm", value("--confirm")?)?,
            "--mouse" if verb == "click" && inline.is_none() => mouse = true,
            "--json" if verb == "snap" && inline.is_none() => json_out = true,
            "--text" | "--url" | "--control" | "--load" | "--gone" if verb == "wait" => {
                let flag = match name {
                    "--text" => "--text",
                    "--url" => "--url",
                    "--load" => "--load",
                    "--gone" => "--gone",
                    _ => "--control",
                };
                let v = value(flag)?;
                if wait.insert(flag[2..].to_owned(), json!(v)).is_some() {
                    return Err(CliError::DuplicateFlag(flag));
                }
            }
            "--timeout" if verb == "wait" => {
                let raw = value("--timeout")?;
                let secs: f64 = raw
                    .parse()
                    .ok()
                    .filter(|s: &f64| s.is_finite() && *s >= 0.0)
                    .ok_or(CliError::BadNumber {
                        flag: "--timeout",
                        value: raw.clone(),
                    })?;
                set(&mut timeout, "--timeout", secs)?;
            }
            _ if arg.starts_with("--") => return Err(CliError::UnknownFlag(arg.to_owned())),
            _ => positional.push(arg.to_owned()),
        }
        i += 1;
    }
    let session = SessionName::parse(session.as_deref().unwrap_or(DEFAULT_SESSION))?;
    for host in &allow_domains {
        ultra_instinct_guard::HostPattern::parse(host)
            .map_err(|err| CliError::Config(err.to_string()))?;
    }
    for glob in &confirm_press {
        ultra_instinct_guard::LabelGlob::parse(glob)
            .map_err(|err| CliError::Config(err.to_string()))?;
    }
    let arity = |want: std::ops::RangeInclusive<usize>, usage: &str| {
        if want.contains(&positional.len()) {
            Ok(())
        } else {
            Err(CliError::Config(format!("usage: aui {verb} {usage}")))
        }
    };
    let tool = |name: &'static str, arguments: Value| Request::Tool { name, arguments };
    let request = match verb {
        "open" => {
            arity(1..=1, "<url>")?;
            tool("browser_navigate", json!({"url": positional[0]}))
        }
        "snap" => {
            arity(0..=0, "[--json]")?;
            let format = if json_out { "json" } else { "text" };
            tool("browser_get_state", json!({"format": format}))
        }
        "click" => {
            arity(1..=1, "<ref> [--confirm <label>] [--mouse]")?;
            let mut arguments = json!({"index": positional[0], "verify": true});
            if let Some(label) = confirm {
                arguments["confirm"] = json!(label);
            }
            if mouse {
                arguments["mouse"] = json!(true);
            }
            tool("browser_click", arguments)
        }
        "type" => {
            if positional.len() < 2 {
                return Err(CliError::Config("usage: aui type <ref> <text>".into()));
            }
            let text = positional[1..].join(" ");
            tool(
                "browser_type",
                json!({"index": positional[0], "text": text, "verify": true}),
            )
        }
        "select" => {
            if positional.len() < 2 {
                return Err(CliError::Config("usage: aui select <ref> <option>".into()));
            }
            let option = positional[1..].join(" ");
            tool(
                "browser_select_dropdown",
                json!({"index": positional[0], "text": option, "verify": true}),
            )
        }
        "key" => {
            arity(2..=2, "<ref> <key>")?;
            tool(
                "browser_send_key",
                json!({"index": positional[0], "key": positional[1], "verify": true}),
            )
        }
        "scroll" => {
            arity(0..=1, "[up|down]")?;
            let down = match positional.first().map(String::as_str) {
                None | Some("down") => true,
                Some("up") => false,
                Some(other) => {
                    return Err(CliError::Config(format!(
                        "usage: aui scroll [up|down], got `{other}`"
                    )))
                }
            };
            tool("browser_scroll", json!({"down": down}))
        }
        "wait" => {
            const USAGE: &str =
                "--text|--url|--control|--gone <v> | --load domcontentloaded|load [--timeout s]";
            arity(0..=0, USAGE)?;
            if wait.is_empty() {
                return Err(CliError::Config(format!("usage: aui wait {USAGE}")));
            }
            if let Some(load) = wait.get("load").and_then(Value::as_str) {
                if !matches!(load, "domcontentloaded" | "load") {
                    return Err(CliError::Config(format!(
                        "--load is domcontentloaded or load, got `{load}`"
                    )));
                }
            }
            if let Some(secs) = timeout {
                wait.insert("seconds".into(), json!(secs));
            }
            tool("browser_wait", Value::Object(wait))
        }
        "shot" => {
            arity(0..=1, "[path]")?;
            let path = match positional.first() {
                Some(path) => std::path::PathBuf::from(path),
                None => std::path::PathBuf::from(format!(
                    "aui-shot-{}.png",
                    std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .map(|d| d.as_secs())
                        .unwrap_or(0)
                )),
            };
            // The daemon writes the file: resolve against this shell's cwd.
            let path = std::env::current_dir()
                .map(|cwd| cwd.join(&path))
                .unwrap_or(path);
            tool(
                "browser_screenshot",
                json!({"path": path.to_string_lossy()}),
            )
        }
        "tabs" => {
            arity(0..=0, "")?;
            tool("browser_list_tabs", json!({}))
        }
        _ => {
            arity(0..=0, "")?;
            Request::Stop
        }
    };
    Ok(Verb {
        verb,
        session,
        cdp: cdp.unwrap_or_else(|| ultra_instinct_browser::DEFAULT_CDP_HTTP.to_owned()),
        request,
        allow_domains,
        confirm_press,
    })
}

fn set<T>(slot: &mut Option<T>, flag: &'static str, value: T) -> Result<(), CliError> {
    if slot.is_some() {
        return Err(CliError::DuplicateFlag(flag));
    }
    *slot = Some(value);
    Ok(())
}

/// The JSON-RPC line for `request`.
pub fn request_line(request: &Request) -> String {
    match request {
        Request::Tool { name, arguments } => json!({
            "jsonrpc": "2.0", "id": 1, "method": "tools/call",
            "params": {"name": name, "arguments": arguments},
        }),
        Request::Stop => json!({"jsonrpc": "2.0", "id": 1, "method": STOP_METHOD}),
    }
    .to_string()
}

/// Short human text for one daemon reply. A refusal or an error is `Err`
/// (non-zero exit).
pub fn render(verb: &Verb, reply: &str) -> Result<String, CliError> {
    let reply: Value = serde_json::from_str(reply)
        .map_err(|err| CliError::Browser(format!("bad daemon reply: {err}")))?;
    if let Some(error) = reply.get("error") {
        let message = error.get("message").and_then(Value::as_str).unwrap_or("");
        return Err(CliError::Refused(format!("✗ error: {message}")));
    }
    let result = &reply["result"];
    if verb.request == Request::Stop {
        return Ok("✓ daemon stopped\n".to_owned());
    }
    let text = result["content"][0]["text"].as_str().unwrap_or_default();
    if result["isError"] == true {
        return Err(CliError::Refused(render_error(text)));
    }
    if verb.verb == "snap" {
        return Ok(format!("{text}\n"));
    }
    let body: Value = serde_json::from_str(text).unwrap_or(Value::Null);
    let line = match verb.verb {
        "open" => format!(
            "✓ session {} · opened {}",
            verb.session.as_str(),
            body["navigated"].as_str().unwrap_or_default()
        ),
        "click" | "type" | "select" | "key" => {
            let mut line = format!("✓ ticket {}", body["ticket"]);
            if let Some(kind) = body["verification"].as_str() {
                line.push_str(&format!(" · {kind}"));
            }
            line
        }
        "scroll" => format!(
            "✓ scrolled {}",
            body["scrolled"].as_str().unwrap_or_default()
        ),
        "wait" => {
            let secs = body["after_ms"].as_u64().unwrap_or(0) as f64 / 1000.0;
            let looks = body["looks"].as_u64().unwrap_or(0);
            if body["seen"] == true {
                format!("✓ seen after {secs:.1} s · {looks} looks")
            } else {
                return Err(CliError::Refused(format!(
                    "✗ not seen in {secs:.1} s · last url {}",
                    body["url"].as_str().unwrap_or("unknown")
                )));
            }
        }
        "shot" => format!(
            "✓ saved {} ({} bytes)",
            body["path"].as_str().unwrap_or_default(),
            body["bytes"]
        ),
        "tabs" => {
            let tabs = body["tabs"].as_array().cloned().unwrap_or_default();
            let mut out = String::new();
            for tab in &tabs {
                let id = tab["tab_id"].as_str().unwrap_or_default();
                out.push_str(&format!(
                    "{}  {}  {}\n",
                    id.get(..8).unwrap_or(id),
                    tab["url"].as_str().unwrap_or_default(),
                    tab["title"].as_str().unwrap_or_default()
                ));
            }
            out.push_str(&format!("{} tabs", tabs.len()));
            out
        }
        _ => text.to_owned(),
    };
    Ok(format!("{line}\n"))
}

/// `✗ refused: <Reason> · run aui snap` for a ticket / gate / policy
/// refusal, `✗ error: …` otherwise.
fn render_error(text: &str) -> String {
    let error: Value = serde_json::from_str(text).unwrap_or(Value::Null);
    let message = error["message"].as_str().unwrap_or(text);
    if error["variant"] != "TicketInvalid" {
        return format!("✗ error: {message}");
    }
    let reason = ["ticket invalid: ", "gate refused: ", "ticket "]
        .iter()
        .fold(message, |m, prefix| m.strip_prefix(prefix).unwrap_or(m));
    let (token, detail) = match reason.split_once(": ") {
        Some((token, detail)) => (token, Some(detail)),
        None => (reason, None),
    };
    let name: String = token
        .split('-')
        .map(|word| {
            let mut chars = word.chars();
            chars
                .next()
                .map(|c| c.to_ascii_uppercase().to_string() + chars.as_str())
                .unwrap_or_default()
        })
        .collect();
    match (token, detail) {
        ("needs-confirm", Some(label)) => {
            let label: String = serde_json::from_str(label).unwrap_or_else(|_| label.to_owned());
            format!(
                "✗ refused: NeedsConfirm {label:?} · rerun with --confirm {}",
                crate::run_cmd::shell_quote(&label)
            )
        }
        ("domain-not-allowed", Some(host)) => {
            format!("✗ refused: DomainNotAllowed {host} · not in --allow-domain")
        }
        _ => format!("✗ refused: {name} · run aui snap"),
    }
}

/// `aui <verb> …`.
pub(crate) fn verb_command(verb: &str, args: &[String]) -> Result<String, CliError> {
    let parsed = parse_verb(verb, args)?;
    imp::run_verb(&parsed)
}

/// `aui daemon --session <name> [--cdp <url>]` (hidden).
pub(crate) fn daemon_command(args: &[String]) -> Result<String, CliError> {
    let (mut session, mut cdp) = (None, None);
    for pair in args.chunks(2) {
        let (flag, slot): (&'static str, _) = match pair[0].as_str() {
            "--session" => ("--session", &mut session),
            "--cdp" => ("--cdp", &mut cdp),
            other => return Err(CliError::UnknownFlag(other.to_owned())),
        };
        let value = pair.get(1).cloned().ok_or(CliError::MissingValue(flag))?;
        set(slot, flag, value)?;
    }
    let session = SessionName::parse(session.as_deref().unwrap_or(DEFAULT_SESSION))?;
    let cdp = cdp.unwrap_or_else(|| ultra_instinct_browser::DEFAULT_CDP_HTTP.to_owned());
    imp::serve_session(&session, &cdp)?;
    Ok(String::new())
}

/// Give a `tools/call` line the daemon's `cdp` endpoint when it names none.
pub fn with_endpoint(line: &str, cdp: &str) -> Option<Value> {
    let mut value: Value = serde_json::from_str(line).ok()?;
    if value.get("method").and_then(Value::as_str) == Some("tools/call") {
        if let Some(params) = value.get_mut("params").and_then(Value::as_object_mut) {
            let arguments = params.entry("arguments").or_insert_with(|| json!({}));
            if let Some(map) = arguments.as_object_mut() {
                map.entry("cdp").or_insert_with(|| json!(cdp));
            }
        }
    }
    Some(value)
}

#[cfg(all(unix, test))]
use imp::{serve_on, Daemon};

#[cfg(unix)]
mod imp {
    use std::io::{self, BufRead, BufReader, Write};
    use std::os::unix::fs::{DirBuilderExt, PermissionsExt};
    use std::os::unix::net::{UnixListener, UnixStream};
    use std::path::{Path, PathBuf};
    use std::time::{Duration, Instant};

    use serde_json::{json, Value};

    use super::{render, request_line, with_endpoint, SessionName, Verb, STOP_METHOD};
    use crate::CliError;

    /// The daemon exits after this long without a request.
    const IDLE: Duration = Duration::from_secs(60 * 60);
    /// How long a verb waits for a spawned daemon's socket.
    const SPAWN_WAIT: Duration = Duration::from_secs(3);
    /// A verb's wait for one reply (`browser_wait` may take 60 s).
    const REPLY_WAIT: Duration = Duration::from_secs(120);
    /// Accept-loop tick while idle.
    const TICK: Duration = Duration::from_millis(25);

    fn io_err(path: &Path, err: io::Error) -> CliError {
        CliError::Io {
            path: path.display().to_string(),
            message: err.to_string(),
        }
    }

    /// `$TMPDIR/aui/<session>.sock`.
    pub fn socket_path(session: &SessionName) -> PathBuf {
        std::env::temp_dir()
            .join("aui")
            .join(format!("{}.sock", session.as_str()))
    }

    /// The socket directory, mode 0700 (refuses one it cannot make private).
    fn private_dir(dir: &Path) -> Result<(), CliError> {
        std::fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(dir)
            .map_err(|err| io_err(dir, err))?;
        std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700))
            .map_err(|err| io_err(dir, err))
    }

    /// A bound daemon socket; removed on drop.
    pub struct Daemon {
        listener: UnixListener,
        path: PathBuf,
    }

    impl Daemon {
        /// Bind `path` (0600) in a 0700 directory. A live daemon on it is an
        /// error; a stale socket file is replaced.
        pub fn bind(path: &Path) -> Result<Self, CliError> {
            if let Some(dir) = path.parent() {
                private_dir(dir)?;
            }
            if path.exists() {
                if UnixStream::connect(path).is_ok() {
                    return Err(CliError::Config(format!(
                        "a daemon already serves {}",
                        path.display()
                    )));
                }
                std::fs::remove_file(path).map_err(|err| io_err(path, err))?;
            }
            let listener = UnixListener::bind(path).map_err(|err| io_err(path, err))?;
            std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))
                .map_err(|err| io_err(path, err))?;
            listener
                .set_nonblocking(true)
                .map_err(|err| io_err(path, err))?;
            Ok(Self {
                listener,
                path: path.to_owned(),
            })
        }
    }

    impl Drop for Daemon {
        fn drop(&mut self) {
            let _ = std::fs::remove_file(&self.path);
        }
    }

    /// Serve `daemon` with `server` until a stop request or `idle` passes
    /// with no request. One connection and one line at a time.
    pub fn serve_on(
        daemon: Daemon,
        server: &mut ultra_instinct_mcp::Server,
        cdp: &str,
        idle: Duration,
    ) -> Result<(), CliError> {
        let mut last = Instant::now();
        loop {
            let stream = match daemon.listener.accept() {
                Ok((stream, _)) => stream,
                Err(err) if err.kind() == io::ErrorKind::WouldBlock => {
                    if last.elapsed() >= idle {
                        return Ok(());
                    }
                    std::thread::sleep(TICK);
                    continue;
                }
                Err(err) => return Err(io_err(&daemon.path, err)),
            };
            // A misbehaving client gets dropped; the daemon keeps serving.
            if let Ok(true) = serve_connection(stream, server, cdp) {
                return Ok(());
            }
            last = Instant::now();
        }
    }

    /// Answer each line of one connection. `Ok(true)`: stop requested.
    fn serve_connection(
        stream: UnixStream,
        server: &mut ultra_instinct_mcp::Server,
        cdp: &str,
    ) -> io::Result<bool> {
        stream.set_nonblocking(false)?;
        stream.set_read_timeout(Some(Duration::from_secs(10)))?;
        let mut writer = stream.try_clone()?;
        for line in BufReader::new(stream).lines() {
            let line = line?;
            let Some(value) = with_endpoint(&line, cdp) else {
                // Not JSON: the server answers the parse error.
                if let Some(reply) = server.handle_line(&line) {
                    writeln!(writer, "{reply}")?;
                }
                continue;
            };
            if value.get("method").and_then(Value::as_str) == Some(STOP_METHOD) {
                let id = value.get("id").cloned().unwrap_or(Value::Null);
                let reply = json!({"jsonrpc": "2.0", "id": id, "result": {"stopped": true}});
                writeln!(writer, "{reply}")?;
                return Ok(true);
            }
            if let Some(reply) = server.handle_line(&value.to_string()) {
                writeln!(writer, "{reply}")?;
            }
        }
        Ok(false)
    }

    /// The hidden `aui daemon` body.
    pub(super) fn serve_session(session: &SessionName, cdp: &str) -> Result<(), CliError> {
        let daemon = Daemon::bind(&socket_path(session))?;
        let mut server = ultra_instinct_mcp::Server::new();
        serve_on(daemon, &mut server, cdp, IDLE)
    }

    /// Send one line on a fresh connection and read one reply line.
    pub(super) fn send(path: &Path, line: &str) -> Result<String, CliError> {
        let mut stream = UnixStream::connect(path).map_err(|err| io_err(path, err))?;
        stream
            .set_read_timeout(Some(REPLY_WAIT))
            .map_err(|err| io_err(path, err))?;
        writeln!(stream, "{line}").map_err(|err| io_err(path, err))?;
        stream
            .shutdown(std::net::Shutdown::Write)
            .map_err(|err| io_err(path, err))?;
        let mut reply = String::new();
        BufReader::new(stream)
            .read_line(&mut reply)
            .map_err(|err| io_err(path, err))?;
        if reply.trim().is_empty() {
            return Err(CliError::Browser(
                "the daemon closed without a reply".into(),
            ));
        }
        Ok(reply)
    }

    /// Connect to the session's daemon; when its socket is missing or stale
    /// run `spawn` and wait up to 3 s for it. `Ok(true)`: it was spawned.
    pub(super) fn connect_or_spawn(
        path: &Path,
        spawn: impl FnOnce() -> Result<(), CliError>,
    ) -> Result<bool, CliError> {
        if UnixStream::connect(path).is_ok() {
            return Ok(false);
        }
        spawn()?;
        let started = Instant::now();
        while started.elapsed() < SPAWN_WAIT {
            if UnixStream::connect(path).is_ok() {
                return Ok(true);
            }
            std::thread::sleep(Duration::from_millis(30));
        }
        Err(CliError::Browser(format!(
            "the daemon did not open {} within 3 s",
            path.display()
        )))
    }

    /// Spawn `current_exe() daemon --session <name> --cdp <url>`, detached
    /// (own process group, null stdio), with the policy env for `open`.
    fn spawn_daemon(verb: &Verb) -> Result<(), CliError> {
        let exe = std::env::current_exe().map_err(|err| CliError::Io {
            path: "current_exe".into(),
            message: err.to_string(),
        })?;
        let mut command = std::process::Command::new(&exe);
        command
            .args([
                "daemon",
                "--session",
                verb.session.as_str(),
                "--cdp",
                &verb.cdp,
            ])
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null());
        if !verb.allow_domains.is_empty() {
            command.env(
                ultra_instinct_mcp::ENV_ALLOW_DOMAINS,
                verb.allow_domains.join(","),
            );
        }
        if !verb.confirm_press.is_empty() {
            command.env(
                ultra_instinct_mcp::ENV_CONFIRM_PRESS,
                verb.confirm_press.join(","),
            );
        }
        std::os::unix::process::CommandExt::process_group(&mut command, 0);
        command.spawn().map(|_| ()).map_err(|err| CliError::Io {
            path: exe.display().to_string(),
            message: err.to_string(),
        })
    }

    pub(super) fn run_verb(verb: &Verb) -> Result<String, CliError> {
        run_verb_at(verb, &socket_path(&verb.session), || spawn_daemon(verb))
    }

    /// [`run_verb`] against `path`, spawning with `spawn`.
    pub(super) fn run_verb_at(
        verb: &Verb,
        path: &Path,
        spawn: impl FnOnce() -> Result<(), CliError>,
    ) -> Result<String, CliError> {
        let policy_flags = !verb.allow_domains.is_empty() || !verb.confirm_press.is_empty();
        if verb.request == super::Request::Stop {
            if UnixStream::connect(path).is_err() {
                return Ok(format!(
                    "✓ no daemon for session {}\n",
                    verb.session.as_str()
                ));
            }
        } else {
            let spawned = connect_or_spawn(path, spawn)?;
            if policy_flags && !spawned {
                return Err(CliError::Config(format!(
                    "session {} is already running; its policy is set when it starts (aui close first)",
                    verb.session.as_str()
                )));
            }
            let reply = send(path, &request_line(&verb.request))?;
            let out = render(verb, &reply)?;
            if spawned && verb.verb == "open" {
                return Ok(out.replacen(" · opened", " · daemon started · opened", 1));
            }
            return Ok(out);
        }
        let reply = send(path, &request_line(&verb.request))?;
        render(verb, &reply)
    }
}

#[cfg(not(unix))]
mod imp {
    use super::{SessionName, Verb};
    use crate::CliError;

    const UNSUPPORTED: &str =
        "aui shell verbs and the session daemon need Unix sockets (macOS or Linux)";

    pub(super) fn run_verb(_verb: &Verb) -> Result<String, CliError> {
        Err(CliError::Config(UNSUPPORTED.into()))
    }

    pub(super) fn serve_session(_session: &SessionName, _cdp: &str) -> Result<(), CliError> {
        Err(CliError::Config(UNSUPPORTED.into()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn a(raw: &[&str]) -> Vec<String> {
        raw.iter().map(|s| (*s).to_owned()).collect()
    }

    #[test]
    fn session_names_are_letters_digits_and_dashes() {
        for ok in ["default", "a", "shop-2", "ABC-def-123"] {
            assert!(SessionName::parse(ok).is_ok(), "{ok}");
        }
        let long = "x".repeat(33);
        for bad in ["", "a b", "../x", "a/b", "a.b", "a_b", "é", long.as_str()] {
            assert!(SessionName::parse(bad).is_err(), "{bad:?}");
        }
    }

    #[test]
    fn verbs_map_to_browser_tools() {
        let tool = |verb: &str, args: &[&str]| match parse_verb(verb, &a(args)).unwrap().request {
            Request::Tool { name, arguments } => (name, arguments),
            Request::Stop => ("stop", Value::Null),
        };
        assert_eq!(
            tool("open", &["https://shop.example/"]),
            ("browser_navigate", json!({"url": "https://shop.example/"}))
        );
        assert_eq!(
            tool("snap", &[]),
            ("browser_get_state", json!({"format": "text"}))
        );
        assert_eq!(
            tool("snap", &["--json"]),
            ("browser_get_state", json!({"format": "json"}))
        );
        assert_eq!(
            tool("click", &["@e5", "--confirm", "Pay $18"]),
            (
                "browser_click",
                json!({"index": "@e5", "verify": true, "confirm": "Pay $18"})
            )
        );
        assert_eq!(
            tool("type", &["@e2", "kid@example.com"]),
            (
                "browser_type",
                json!({"index": "@e2", "text": "kid@example.com", "verify": true})
            )
        );
        assert_eq!(
            tool("select", &["3", "New", "Zealand"]),
            (
                "browser_select_dropdown",
                json!({"index": "3", "text": "New Zealand", "verify": true})
            )
        );
        assert_eq!(
            tool("key", &["@e2", "Enter"]),
            (
                "browser_send_key",
                json!({"index": "@e2", "key": "Enter", "verify": true})
            )
        );
        assert_eq!(
            tool("scroll", &["up"]),
            ("browser_scroll", json!({"down": false}))
        );
        assert_eq!(
            tool("scroll", &[]),
            ("browser_scroll", json!({"down": true}))
        );
        assert_eq!(
            tool("wait", &["--text", "Welcome", "--timeout", "2.5"]),
            ("browser_wait", json!({"text": "Welcome", "seconds": 2.5}))
        );
        assert_eq!(
            tool("wait", &["--load", "domcontentloaded"]),
            ("browser_wait", json!({"load": "domcontentloaded"}))
        );
        assert_eq!(
            tool("wait", &["--gone", "Loading…", "--load=load"]),
            ("browser_wait", json!({"gone": "Loading…", "load": "load"}))
        );
        assert_eq!(tool("tabs", &[]), ("browser_list_tabs", json!({})));
        assert_eq!(tool("close", &[]), ("stop", Value::Null));
        let (name, arguments) = tool("shot", &["out.png"]);
        assert_eq!(name, "browser_screenshot");
        let path = arguments["path"].as_str().unwrap();
        assert!(std::path::Path::new(path).is_absolute() && path.ends_with("out.png"));
    }

    #[test]
    fn verb_flags_and_arity_are_checked() {
        let open = parse_verb(
            "open",
            &a(&[
                "https://shop.example/",
                "--session",
                "s1",
                "--cdp=http://127.0.0.1:9333",
                "--allow-domain",
                "shop.example",
                "--confirm-press",
                "Pay*",
            ]),
        )
        .unwrap();
        assert_eq!(open.session.as_str(), "s1");
        assert_eq!(open.cdp, "http://127.0.0.1:9333");
        assert_eq!(open.allow_domains, ["shop.example"]);
        assert_eq!(open.confirm_press, ["Pay*"]);
        assert_eq!(
            parse_verb("snap", &[]).unwrap().cdp,
            "http://127.0.0.1:9222"
        );
        for (verb, args) in [
            ("open", &[][..]),
            ("click", &[][..]),
            ("type", &["@e1"][..]),
            ("key", &["Enter"][..]),
            ("scroll", &["left"][..]),
            ("wait", &[][..]),
            ("wait", &["--timeout", "x", "--url", "/a"][..]),
            ("wait", &["--load", "networkidle"][..]),
            ("wait", &["--gone"][..]),
            ("snap", &["--confirm", "x"][..]),
            ("click", &["@e1", "--allow-domain", "x"][..]),
            ("open", &["https://a/", "--allow-domain", "https://a/"][..]),
            ("snap", &["--session", "bad name"][..]),
            ("tabs", &["extra"][..]),
        ] {
            assert!(parse_verb(verb, &a(args)).is_err(), "{verb} {args:?}");
        }
        assert!(parse_verb("fly", &[]).is_err());
    }

    #[test]
    fn refusals_render_short_and_fail() {
        let click = parse_verb("click", &a(&["@e5"])).unwrap();
        let reply = |text: &str, is_error: bool| {
            json!({"jsonrpc": "2.0", "id": 1, "result": {
                "content": [{"type": "text", "text": text}], "isError": is_error}})
            .to_string()
        };
        let ok = render(
            &click,
            &reply(
                r#"{"acted":"click","ticket":41,"verification":"navigation"}"#,
                false,
            ),
        )
        .unwrap();
        assert_eq!(ok, "✓ ticket 41 · navigation\n");
        let stale = render(
            &click,
            &reply(
                r#"{"variant":"TicketInvalid","message":"ticket target-gone"}"#,
                true,
            ),
        )
        .unwrap_err();
        assert_eq!(stale.to_string(), "✗ refused: TargetGone · run aui snap");
        let confirm = render(
            &click,
            &reply(
                r#"{"variant":"TicketInvalid","message":"needs-confirm: \"Pay $18\""}"#,
                true,
            ),
        )
        .unwrap_err();
        assert_eq!(
            confirm.to_string(),
            "✗ refused: NeedsConfirm \"Pay $18\" · rerun with --confirm \"Pay \\$18\""
        );
        let gate = render(
            &click,
            &reply(r#"{"variant":"TicketInvalid","message":"occluded"}"#, true),
        )
        .unwrap_err();
        assert_eq!(gate.to_string(), "✗ refused: Occluded · run aui snap");
        let other = render(
            &click,
            &reply(r#"{"variant":"Browser","message":"socket closed"}"#, true),
        )
        .unwrap_err();
        assert_eq!(other.to_string(), "✗ error: socket closed");
    }

    #[test]
    fn the_daemon_names_its_endpoint_unless_the_call_does() {
        let line = request_line(&Request::Tool {
            name: "browser_get_state",
            arguments: json!({}),
        });
        let value = with_endpoint(&line, "http://127.0.0.1:9333").unwrap();
        assert_eq!(value["params"]["arguments"]["cdp"], "http://127.0.0.1:9333");
        let own = line.replace("{}", r#"{"cdp":"http://h:1"}"#);
        let value = with_endpoint(&own, "http://127.0.0.1:9333").unwrap();
        assert_eq!(value["params"]["arguments"]["cdp"], "http://h:1");
    }

    #[cfg(unix)]
    mod socket {
        use super::super::imp::run_verb_at as run_verb_with;
        use super::super::*;
        use super::a;
        use std::os::unix::fs::PermissionsExt;
        use std::time::Duration;
        use ultra_instinct_browser::script::{Control, PageSpec, ScriptBuilder};
        use ultra_instinct_browser::{CdpTransport, ReplayTransport};

        fn page() -> PageSpec {
            PageSpec::of(
                &[
                    Control::text_field(10, 100, "Email", (20.0, 20.0, 300.0, 28.0)),
                    Control::button(11, 101, "Sign in", (20.0, 80.0, 120.0, 28.0)),
                ],
                "http://127.0.0.1/form",
                "Form",
            )
        }

        /// A short socket path (macOS `sun_path` is 104 bytes).
        fn path(tag: &str) -> std::path::PathBuf {
            std::path::PathBuf::from(format!("/tmp/aui-t-{}-{tag}/s.sock", std::process::id()))
        }

        /// Run a daemon on `path` in a thread over a replay of `script`.
        fn start(path: &std::path::Path, script: ScriptBuilder) -> std::thread::JoinHandle<()> {
            let path = path.to_owned();
            let json = script.to_json();
            let daemon = Daemon::bind(&path).unwrap();
            // The listener moves into the thread; the server is built there
            // (it is not Send).
            std::thread::spawn(move || {
                let mut once = Some(json);
                let mut server = ultra_instinct_mcp::Server::with_connector(move |_url| {
                    let json = once.take().expect("one connect");
                    Ok(Box::new(ReplayTransport::parse(&json)?) as Box<dyn CdpTransport>)
                });
                serve_on(
                    daemon,
                    &mut server,
                    "http://127.0.0.1:9222",
                    Duration::from_secs(20),
                )
                .unwrap();
            })
        }

        #[test]
        fn verbs_round_trip_through_the_socket_and_close_stops_it() {
            let path = path("rt");
            let script = ScriptBuilder::new()
                .observe_compact(&page())
                .observe_compact(&page())
                .observe_compact(&page())
                .dom_click(11)
                .ready_state("complete")
                .observe_compact(&page());
            let handle = start(&path, script);
            let mode =
                |p: &std::path::Path| std::fs::metadata(p).unwrap().permissions().mode() & 0o777;
            assert_eq!(mode(&path), 0o600);
            assert_eq!(mode(path.parent().unwrap()), 0o700);
            let no_spawn = || panic!("the daemon is running");
            let snap = parse_verb("snap", &[]).unwrap();
            let out = run_verb_with(&snap, &path, no_spawn).unwrap();
            assert!(
                out.starts_with("Form  http://127.0.0.1/form\n@e1 textbox \"Email\""),
                "{out}"
            );
            assert!(out.contains("@e2 button \"Sign in\""), "{out}");
            let click = parse_verb("click", &a(&["@e2"])).unwrap();
            let out = run_verb_with(&click, &path, no_spawn).unwrap();
            assert!(out.starts_with("✓ ticket ") && out.contains(" · "), "{out}");
            let close = parse_verb("close", &[]).unwrap();
            assert_eq!(
                run_verb_with(&close, &path, no_spawn).unwrap(),
                "✓ daemon stopped\n"
            );
            handle.join().unwrap();
            assert!(!path.exists(), "socket removed on exit");
            let _ = std::fs::remove_dir(path.parent().unwrap());
        }

        #[test]
        fn a_missing_socket_spawns_the_daemon_and_a_stale_one_is_replaced() {
            let path = path("sp");
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            // A stale socket file: bound once, nobody listening now.
            drop(std::os::unix::net::UnixListener::bind(&path).unwrap());
            assert!(path.exists());
            let mut handle = None;
            let snap = parse_verb("snap", &[]).unwrap();
            let out = run_verb_with(&snap, &path, || {
                handle = Some(start(&path, ScriptBuilder::new().observe_compact(&page())));
                Ok(())
            })
            .unwrap();
            assert!(out.contains("@e2 button \"Sign in\""), "{out}");
            // A second daemon on a live socket refuses to start.
            assert!(Daemon::bind(&path).is_err());
            let close = parse_verb("close", &[]).unwrap();
            run_verb_with(&close, &path, || panic!("running")).unwrap();
            handle.unwrap().join().unwrap();
            // With no daemon, close does not spawn one.
            let out = run_verb_with(&close, &path, || panic!("close never spawns")).unwrap();
            assert!(out.starts_with("✓ no daemon"), "{out}");
            let _ = std::fs::remove_dir(path.parent().unwrap());
        }

        #[test]
        fn a_spawn_that_never_binds_times_out() {
            let path = path("to");
            let snap = parse_verb("snap", &[]).unwrap();
            let started = std::time::Instant::now();
            let err = run_verb_with(&snap, &path, || Ok(())).unwrap_err();
            assert!(err.to_string().contains("did not open"), "{err}");
            assert!(started.elapsed() >= Duration::from_secs(3));
        }

        #[test]
        fn the_daemon_exits_when_idle() {
            let path = path("idle");
            let daemon = Daemon::bind(&path).unwrap();
            let mut server = ultra_instinct_mcp::Server::with_connector(|_url| {
                Err(ultra_instinct_browser::CdpError::Transport {
                    message: "no browser".into(),
                })
            });
            serve_on(daemon, &mut server, "http://x", Duration::from_millis(50)).unwrap();
            assert!(!path.exists());
            let _ = std::fs::remove_dir(path.parent().unwrap());
        }
    }
}
