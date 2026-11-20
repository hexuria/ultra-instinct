//! The session's request/response loop and lifecycle, shared by every
//! platform's [`Transport`]: unix socket files, Windows named pipes.
//!
//! One session is one daemon answering one connection and one line at a
//! time, in a single thread: each line is a JSON-RPC request for one
//! [`ultra_instinct_mcp::Server`], so a session owns one CDP socket. The
//! daemon exits after its idle time or on `aui close`, and drops its
//! endpoint (the transport removes the socket file or marker).

use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};

use serde_json::{json, Value};
use ultra_instinct_browser::launch::{self, ChromeProcess, LaunchOptions};
use ultra_instinct_browser::{CdpTransport, WebSocketTransport};

use super::{connect_hint, request_line, with_endpoint, Browser, SessionName, Verb, STOP_METHOD};
use crate::verbs::render;
use crate::CliError;

/// How long a verb waits for a spawned daemon's endpoint.
const SPAWN_WAIT: Duration = Duration::from_secs(3);
/// A verb's wait for one reply (`browser_wait` may take 60 s; a launch 20 s).
const REPLY_WAIT: Duration = Duration::from_secs(120);
/// How long a daemon whose Chrome failed to start waits to tell a verb.
const FAILURE_WAIT: Duration = Duration::from_secs(10);
/// Accept-loop tick while idle.
const TICK: Duration = Duration::from_millis(25);
/// A connection's read timeout between lines.
const LINE_WAIT: Duration = Duration::from_secs(10);

/// `AUI_SESSION_STATE=0|off|false|no` turns auto session state off.
const AUTO_STATE_ENV: &str = "AUI_SESSION_STATE";
/// `AUI_STATE_DIR` overrides the per-user state directory.
pub const STATE_DIR_ENV: &str = "AUI_STATE_DIR";

/// How a platform carries the session protocol: where the endpoint lives,
/// how to bind and connect it, and how a spawned daemon detaches.
///
/// Unix binds a socket file in a 0700 directory (the file mode is the
/// access control). Windows binds a named pipe — its per-user ACL is the
/// same boundary — and keeps a marker file where the socket path would be,
/// so endpoint discovery and stale cleanup read the same directory.
pub trait Transport {
    /// One client connection.
    type Stream: SessionStream + Read + Write;
    /// The bound endpoint's accept side.
    type Listener: SessionListener<Stream = Self::Stream>;

    /// Make `dir` usable only by this user (0700 on unix; the per-user
    /// temp/profile ACLs already do it on Windows).
    fn private_dir(dir: &Path) -> Result<(), CliError>;
    /// The per-user directory session state files live in, if any.
    fn state_dir() -> Option<PathBuf>;
    /// Bind the endpoint at `path` (the directory is already private): a
    /// live endpoint on it is an error, a stale one is replaced.
    fn open(path: &Path) -> Result<Self::Listener, CliError>;
    /// Open a client connection to the endpoint `path` names.
    fn connect(path: &Path) -> io::Result<Self::Stream>;
    /// Platform flags that detach a spawned daemon's `Command`.
    fn detach(command: &mut Command);
}

/// The accept side of a bound session endpoint.
pub trait SessionListener {
    type Stream;
    /// The next client connection; `WouldBlock` in nonblocking mode.
    fn accept(&self) -> io::Result<Self::Stream>;
}

/// Per-connection knobs the serve loop sets.
pub trait SessionStream {
    fn set_nonblocking(&self, on: bool) -> io::Result<()>;
    fn set_recv_timeout(&self, timeout: Option<Duration>) -> io::Result<()>;
}

/// `$TMPDIR/aui` (`aui doctor` may have made it first).
pub fn socket_dir() -> PathBuf {
    std::env::temp_dir().join("aui")
}

/// `AUI_STATE_DIR` when set and non-empty.
pub fn state_dir_override() -> Option<PathBuf> {
    std::env::var_os(STATE_DIR_ENV)
        .map(PathBuf::from)
        .filter(|dir| !dir.as_os_str().is_empty())
}

/// The session's endpoint path: `<dir>/<session>.sock` — a socket file on
/// unix, a marker file naming the pipe on Windows.
pub fn socket_path(session: &SessionName) -> PathBuf {
    socket_dir().join(format!("{}.sock", session.as_str()))
}

/// `path: message` as a [`CliError::Io`].
pub fn io_err(path: &Path, err: io::Error) -> CliError {
    CliError::Io {
        path: path.display().to_string(),
        message: err.to_string(),
    }
}

/// A bound daemon endpoint; its path is removed on drop.
pub struct Daemon<T: Transport> {
    listener: T::Listener,
    path: PathBuf,
}

impl<T: Transport> Daemon<T> {
    /// Bind `path` in a private directory. A live endpoint on it is an
    /// error; a stale one is replaced.
    pub fn bind(path: &Path) -> Result<Self, CliError> {
        if let Some(dir) = path.parent() {
            T::private_dir(dir)?;
        }
        let listener = T::open(path)?;
        Ok(Self {
            listener,
            path: path.to_owned(),
        })
    }
}

impl<T: Transport> Drop for Daemon<T> {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
}

/// One connection's request/reply pump over `&mut S`: `read_line` waits
/// for a `\n`-terminated line (`None` at EOF), `write_line` appends `\n`
/// and flushes. The single mutable borrow lets reads and writes interleave
/// without cloning the stream (unix sockets needed `try_clone`; named pipes
/// have no `shutdown`).
struct Conn<'a, S> {
    io: &'a mut S,
    buf: Vec<u8>,
}

impl<'a, S> Conn<'a, S>
where
    S: Read + Write,
{
    fn new(io: &'a mut S) -> Self {
        Self {
            io,
            buf: Vec::new(),
        }
    }

    /// The next line without its terminator; `None` at EOF. A final line
    /// without `\n` is still yielded, like `BufRead::lines`.
    fn read_line(&mut self) -> io::Result<Option<String>> {
        loop {
            if let Some(at) = self.buf.iter().position(|b| *b == b'\n') {
                let mut line = self.buf.drain(..=at).collect::<Vec<u8>>();
                line.pop();
                if line.last() == Some(&b'\r') {
                    line.pop();
                }
                return String::from_utf8(line)
                    .map(Some)
                    .map_err(|err| io::Error::new(io::ErrorKind::InvalidData, err));
            }
            let mut chunk = [0u8; 8192];
            match self.io.read(&mut chunk)? {
                0 if self.buf.is_empty() => return Ok(None),
                0 => {
                    let line = std::mem::take(&mut self.buf);
                    return String::from_utf8(line)
                        .map(Some)
                        .map_err(|err| io::Error::new(io::ErrorKind::InvalidData, err));
                }
                n => self.buf.extend_from_slice(&chunk[..n]),
            }
        }
    }

    fn write_line(&mut self, line: &str) -> io::Result<()> {
        self.io.write_all(line.as_bytes())?;
        self.io.write_all(b"\n")?;
        self.io.flush()
    }
}

fn auto_state_off() -> bool {
    std::env::var(AUTO_STATE_ENV).is_ok_and(|v| state_value_off(&v))
}

pub fn state_value_off(value: &str) -> bool {
    matches!(
        value.trim().to_ascii_lowercase().as_str(),
        "0" | "off" | "false" | "no"
    )
}

/// A `--launch` session's saved login (`<state dir>/<session>.json`), in
/// the `browser_state_*` format. Restored when the daemon starts and
/// saved when it ends, however it ends. `pending` holds the origins whose
/// storage still waits for a matching navigation; empty means the restore
/// is done (or there was nothing to restore).
pub(crate) struct AutoState {
    path: PathBuf,
    /// The daemon's CDP endpoint, handed to the state tools as `cdp`.
    cdp: String,
    pending: Vec<String>,
    /// Storage was applied after a navigation this session.
    applied: bool,
}

impl AutoState {
    /// Auto state for a `--launch` session, unless `AUI_SESSION_STATE`
    /// turns it off or `dir` (the transport's state directory, already
    /// made private) is `None`. `--cdp` / `--attach` sessions pass
    /// `enabled: false`: their browsers are the user's own.
    fn new(session: &SessionName, cdp: &str, enabled: bool, dir: Option<PathBuf>) -> Option<Self> {
        if !enabled || auto_state_off() {
            return None;
        }
        let dir = dir?;
        Some(Self {
            path: dir.join(format!("{}.json", session.as_str())),
            cdp: cdp.to_owned(),
            pending: Vec::new(),
            applied: false,
        })
    }

    /// Load the saved state into the fresh browser, if a file exists.
    /// Best-effort: a missing, corrupt, or policy-refused state changes
    /// nothing and never stops the session.
    fn restore(&mut self, server: &mut ultra_instinct_mcp::Server) {
        if self.path.exists() {
            self.load(server);
        }
    }

    /// `browser_state_load` on the session's endpoint; `pending` becomes
    /// the origins still waiting for their page.
    fn load(&mut self, server: &mut ultra_instinct_mcp::Server) {
        let Ok(reply) = server.call_tool(
            "browser_state_load",
            &json!({"path": self.path.display().to_string(), "cdp": self.cdp}),
        ) else {
            return;
        };
        self.pending = reply["pending_origins"]
            .as_array()
            .map(|list| {
                list.iter()
                    .filter_map(Value::as_str)
                    .map(str::to_owned)
                    .collect()
            })
            .unwrap_or_default();
        self.applied = reply["applied_origin"].is_string();
    }

    /// After a request the daemon answered: while storage origins are
    /// still pending, a successful `browser_navigate` / `browser_new_tab`
    /// re-loads the state (cookies re-set idempotently; the origin's
    /// storage is written once the page is on it). The first application
    /// reloads the page once, so it reads what was restored.
    fn after_reply(&mut self, server: &mut ultra_instinct_mcp::Server, request: &Value, ok: bool) {
        if self.pending.is_empty() || !ok {
            return;
        }
        let name = request["params"]["name"].as_str().unwrap_or_default();
        if !matches!(name, "browser_navigate" | "browser_new_tab") {
            return;
        }
        self.load(server);
        if self.applied {
            // The page loaded before its storage landed; load it once
            // more so it reads the restored values. Best-effort.
            let _ = server.call_tool("browser_reload", &json!({"cdp": self.cdp}));
        }
    }

    /// `browser_state_save` on exit; best-effort (a policy refusal, a
    /// dead browser, or an unwritable directory never fails shutdown).
    fn save(&self, server: &mut ultra_instinct_mcp::Server) {
        let _ = server.call_tool(
            "browser_state_save",
            &json!({"path": self.path.display().to_string(), "cdp": self.cdp}),
        );
    }
}

/// Whether a JSON-RPC reply carried a successful tool result.
pub fn reply_ok(reply: &str) -> bool {
    serde_json::from_str::<Value>(reply)
        .is_ok_and(|v| v.get("error").is_none() && v["result"]["isError"] != true)
}

/// Serve `daemon` with `server` until a stop request or `idle` passes
/// with no request. One connection and one line at a time.
pub fn serve_on<T: Transport>(
    daemon: Daemon<T>,
    server: &mut ultra_instinct_mcp::Server,
    cdp: &str,
    idle: Duration,
    auto: &mut Option<AutoState>,
    recorder: &mut Option<crate::record::Recorder>,
) -> Result<(), CliError> {
    let mut last = Instant::now();
    // The text of the session's latest `aui snap`, for `aui diff`.
    let mut snap = None;
    loop {
        let stream = match daemon.listener.accept() {
            Ok(stream) => stream,
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
        if let Ok(true) = serve_connection(stream, server, cdp, &mut snap, auto, recorder) {
            return Ok(());
        }
        last = Instant::now();
    }
}

/// Answer each line of one connection. `Ok(true)`: stop requested.
fn serve_connection<S>(
    mut stream: S,
    server: &mut ultra_instinct_mcp::Server,
    cdp: &str,
    snap: &mut Option<String>,
    auto: &mut Option<AutoState>,
    recorder: &mut Option<crate::record::Recorder>,
) -> io::Result<bool>
where
    S: SessionStream + Read + Write,
{
    stream.set_nonblocking(false)?;
    stream.set_recv_timeout(Some(LINE_WAIT))?;
    let mut conn = Conn::new(&mut stream);
    while let Some(line) = conn.read_line()? {
        let Some(value) = with_endpoint(&line, cdp) else {
            // Not JSON: the server answers the parse error.
            if let Some(reply) = server.handle_line(&line) {
                conn.write_line(&reply)?;
            }
            continue;
        };
        if value.get("method").and_then(Value::as_str) == Some(STOP_METHOD) {
            let id = value.get("id").cloned().unwrap_or(Value::Null);
            let reply = json!({"jsonrpc": "2.0", "id": id, "result": {"stopped": true}});
            conn.write_line(&reply.to_string())?;
            return Ok(true);
        }
        let (value, diff, cut) = take_diff(value);
        if let Some(reply) = server.handle_line(&value.to_string()) {
            let out = note_snap(&value, reply, diff, cut, snap);
            conn.write_line(&out)?;
            if let Some(state) = auto.as_mut() {
                state.after_reply(server, &value, reply_ok(&out));
            }
            // After the auto-state reload, so a frame shows the page as
            // the session left it.
            if let Some(rec) = recorder.as_mut() {
                rec.after_reply(server, &value, reply_ok(&out));
            }
        }
    }
    Ok(false)
}

/// `aui diff` is a text `browser_get_state` with `diff: true`: the
/// request without that flag, whether it was set, and its `max_chars`
/// (`--max-output`), taken off so the look that is compared is whole.
fn take_diff(mut request: Value) -> (Value, bool, Option<usize>) {
    let Some(arguments) = request["params"]["arguments"].as_object_mut() else {
        return (request, false, None);
    };
    if arguments.remove("diff") != Some(Value::Bool(true)) {
        return (request, false, None);
    }
    let cut = arguments.remove("max_chars").and_then(|max| max.as_u64());
    (request, true, cut.map(|max| max as usize))
}

/// Remember the text of a `browser_get_state` reply as the session's
/// latest snap. For `aui diff`, the reply's text becomes the changes
/// since the snap before it (an error when there was none).
fn note_snap(
    request: &Value,
    reply: String,
    diff: bool,
    cut: Option<usize>,
    snap: &mut Option<String>,
) -> String {
    let params = &request["params"];
    if params["name"] != "browser_get_state" || params["arguments"]["format"] != "text" {
        return reply;
    }
    let Ok(mut body) = serde_json::from_str::<Value>(&reply) else {
        return reply;
    };
    let result = &mut body["result"];
    let Some(text) = result["content"][0]["text"].as_str().map(str::to_owned) else {
        return reply;
    };
    if result["isError"] == true {
        return reply;
    }
    // A snap cut by `--max-output` is no baseline: its missing controls
    // would read as gone.
    if params["arguments"].get("max_chars").is_some() {
        *snap = None;
        return reply;
    }
    let before = snap.replace(text.clone());
    if !diff {
        return reply;
    }
    match before {
        Some(before) => {
            let changes = crate::diff::diff_snaps(&before, &text);
            let mut shown = changes.trim_end().to_owned();
            if let Some((at, _)) = cut.and_then(|max| shown.char_indices().nth(max)) {
                shown.truncate(at);
                shown.push_str("\n… cut by --max-output; run aui snap for the refs");
            }
            result["content"][0]["text"] = json!(shown);
        }
        None => {
            result["content"][0]["text"] = json!(json!({
                "code": "snapshot_required",
                "message": "no earlier aui snap to compare with",
            })
            .to_string());
            result["isError"] = json!(true);
        }
    }
    body.to_string()
}

/// A daemon that could not start: answer the first request that arrives
/// within `wait` with `message` (a JSON-RPC error), then stop.
pub fn serve_failure<T: Transport>(daemon: Daemon<T>, message: &str, wait: Duration) {
    let started = Instant::now();
    while started.elapsed() < wait {
        let mut stream = match daemon.listener.accept() {
            Ok(stream) => stream,
            Err(err) if err.kind() == io::ErrorKind::WouldBlock => {
                std::thread::sleep(TICK);
                continue;
            }
            Err(_) => return,
        };
        let answered = (|| -> io::Result<()> {
            stream.set_nonblocking(false)?;
            stream.set_recv_timeout(Some(LINE_WAIT))?;
            let mut conn = Conn::new(&mut stream);
            let Some(line) = conn.read_line()? else {
                return Ok(());
            };
            let id = serde_json::from_str::<Value>(&line)
                .ok()
                .and_then(|v| v.get("id").cloned())
                .unwrap_or(Value::Null);
            let reply = json!({"jsonrpc": "2.0", "id": id,
                               "error": {"code": -32000, "message": message}});
            conn.write_line(&reply.to_string())
        })();
        if answered.is_ok() {
            return;
        }
    }
}

/// What one daemon owns. Fields drop in order: the MCP server (and with
/// it every CDP socket) first, then Chrome is killed and its temp
/// profile removed.
struct Owned {
    server: ultra_instinct_mcp::Server,
    _chrome: Option<ChromeProcess>,
}

/// The daemon's server: live websockets under the env policy. When the
/// daemon did not launch Chrome, a connect failure carries the hint.
fn daemon_server(cdp: &str, hint: bool) -> ultra_instinct_mcp::Server {
    let cdp = cdp.to_owned();
    file_policy(hardened(ultra_instinct_mcp::Server::with_connector(
        move |url| {
            WebSocketTransport::connect(url)
                .map(|transport| Box::new(transport) as Box<dyn CdpTransport>)
                .map_err(|err| if hint { connect_hint(err, &cdp) } else { err })
        },
    )))
}

/// `server` under the action policy file the spawning verb named
/// (`ENV_POLICY_FILE`), when it named one. The verb already read it; if
/// it no longer reads, nothing runs.
fn file_policy(server: ultra_instinct_mcp::Server) -> ultra_instinct_mcp::Server {
    match std::env::var_os(ultra_instinct_mcp::ENV_POLICY_FILE) {
        Some(file) if !file.is_empty() => {
            match ultra_instinct_mcp::ToolPolicy::read(Path::new(&file)) {
                Ok(policy) => server.with_tool_policy(&policy),
                Err(_) => server.with_tool_allowlist(Vec::<String>::new()),
            }
        }
        _ => server,
    }
}

/// The daemon's settings on any server: the env policy, and a ref press
/// binds to the latest `aui snap` of its tab, and needs one.
fn hardened(server: ultra_instinct_mcp::Server) -> ultra_instinct_mcp::Server {
    server
        .with_env_policy()
        .require_snapshot(true)
        .bind_to_latest_look(true)
}

/// The user's own Chrome: find its browser socket, connect (Chrome may
/// ask the user first), and open the daemon's own tab, so no verb ever
/// lands on one of the user's tabs unasked. Unix only: it rides the
/// session-multiplexing browser (`CdpBrowser`), which is `cfg(unix)`.
#[cfg(unix)]
fn attached_server() -> Result<ultra_instinct_mcp::Server, String> {
    let url = launch::find_attach_endpoint().map_err(|err| err.to_string())?;
    let browser =
        ultra_instinct_mcp::CdpBrowser::connect_websocket(&url).map_err(|e| e.to_string())?;
    let mut server = hardened(ultra_instinct_mcp::Server::with_attach(browser));
    server
        .call_tool("browser_new_tab", &json!({"url": "about:blank"}))
        .map_err(|err| err.to_string())?;
    // After the daemon's own blank tab: a policy file that takes
    // `browser_new_tab` off limits the caller, not this.
    Ok(file_policy(server))
}

/// The hidden `aui daemon` body.
pub fn serve_session<T: Transport>(
    session: &SessionName,
    browser: Browser,
    idle: Duration,
    record: Option<PathBuf>,
) -> Result<(), CliError> {
    // Bind first: the spawning verb is already waiting on this endpoint,
    // and a launch can take longer than its 3 s spawn wait.
    let daemon = Daemon::<T>::bind(&socket_path(session))?;
    // `--record` works on every browser kind: it only watches the page.
    // The verb already made the frames dir; a failure here just records
    // nothing rather than stopping the session.
    let mut recorder = record
        .as_deref()
        .and_then(|file| crate::record::Recorder::open(file).ok());
    // Only a `--launch` session keeps auto state: a `--cdp` browser is the
    // user's own and an `--attach` one cannot serve state tools at all.
    let auto_enabled = matches!(&browser, Browser::Launch(_));
    let (chrome, endpoint) = match browser {
        // The server drops after the socket: it closes only its own tabs.
        Browser::Attach => return attach_session(daemon, idle, &mut recorder),
        Browser::Cdp(cdp) => (None, cdp),
        Browser::Launch(own) => match launch::launch(&LaunchOptions {
            headed: own.headed,
            profile_dir: own.profile.map(std::path::PathBuf::from),
            proxy: own.proxy,
            ..LaunchOptions::default()
        }) {
            Ok(chrome) => {
                let endpoint = chrome.endpoint();
                (Some(chrome), endpoint)
            }
            Err(err) => {
                let message = err.to_string();
                serve_failure(daemon, &message, FAILURE_WAIT);
                finish_recording(&mut recorder);
                return Err(CliError::Browser(message));
            }
        },
    };
    let mut owned = Owned {
        server: daemon_server(&endpoint, chrome.is_none()),
        _chrome: chrome,
    };
    // Saved login back into the fresh browser before the first request;
    // the file is rewritten with the session's end state on the way out.
    let state_dir = T::state_dir().filter(|dir| T::private_dir(dir).is_ok());
    let mut auto = AutoState::new(session, &endpoint, auto_enabled, state_dir);
    if let Some(state) = auto.as_mut() {
        state.restore(&mut owned.server);
    }
    // `daemon` (the socket) is dropped inside; `owned` after, in order.
    let result = serve_on(
        daemon,
        &mut owned.server,
        &endpoint,
        idle,
        &mut auto,
        &mut recorder,
    );
    finish_recording(&mut recorder);
    if let Some(state) = &auto {
        state.save(&mut owned.server);
    }
    result
}

/// `--attach` where the session-multiplexing browser exists.
#[cfg(unix)]
fn attach_session<T: Transport>(
    daemon: Daemon<T>,
    idle: Duration,
    recorder: &mut Option<crate::record::Recorder>,
) -> Result<(), CliError> {
    match attached_server() {
        Ok(mut server) => {
            let result = serve_on(
                daemon,
                &mut server,
                ultra_instinct_mcp::ATTACH_ENDPOINT,
                idle,
                &mut None,
                recorder,
            );
            finish_recording(recorder);
            result
        }
        Err(message) => {
            serve_failure(daemon, &message, FAILURE_WAIT);
            finish_recording(recorder);
            Err(CliError::Browser(message))
        }
    }
}

/// `--attach` elsewhere: refused with the same message the verb prints.
#[cfg(not(unix))]
fn attach_session<T: Transport>(
    daemon: Daemon<T>,
    _idle: Duration,
    recorder: &mut Option<crate::record::Recorder>,
) -> Result<(), CliError> {
    serve_failure(daemon, super::ATTACH_NEEDS_UNIX, FAILURE_WAIT);
    finish_recording(recorder);
    Err(CliError::Config(super::ATTACH_NEEDS_UNIX.into()))
}

/// Assemble the recording however the session ends; never fails.
fn finish_recording(recorder: &mut Option<crate::record::Recorder>) {
    if let Some(recorder) = recorder.take() {
        recorder.finish();
    }
}

/// `aui session list` over `dir`: each `*.sock` that answers is a live
/// session; the rest are removed and named.
pub fn list_sessions<T: Transport>(dir: &Path) -> Result<String, CliError> {
    let entries = match std::fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok("no sessions\n".to_owned()),
        Err(err) => return Err(io_err(dir, err)),
    };
    let mut sockets: Vec<PathBuf> = entries
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| path.extension().is_some_and(|ext| ext == "sock"))
        .collect();
    sockets.sort();
    let (mut live, mut removed) = (Vec::new(), Vec::new());
    for path in sockets {
        let name = path
            .file_stem()
            .map(|stem| stem.to_string_lossy().into_owned())
            .unwrap_or_default();
        if T::connect(&path).is_ok() {
            live.push(name);
        } else if std::fs::remove_file(&path).is_ok() {
            removed.push(name);
        }
    }
    let mut out = String::new();
    for name in &live {
        out.push_str(&format!("{name}\n"));
    }
    out.push_str(&match live.len() {
        0 => "no sessions\n".to_owned(),
        1 => "1 live session\n".to_owned(),
        n => format!("{n} live sessions\n"),
    });
    for name in &removed {
        out.push_str(&format!("removed stale socket {name}.sock\n"));
    }
    Ok(out)
}

/// Send one line on a fresh connection and read one reply line.
pub fn send<T: Transport>(path: &Path, line: &str) -> Result<String, CliError> {
    let mut stream = T::connect(path).map_err(|err| io_err(path, err))?;
    stream
        .set_recv_timeout(Some(REPLY_WAIT))
        .map_err(|err| io_err(path, err))?;
    let mut conn = Conn::new(&mut stream);
    conn.write_line(line).map_err(|err| io_err(path, err))?;
    match conn.read_line().map_err(|err| io_err(path, err))? {
        Some(reply) if !reply.trim().is_empty() => Ok(reply),
        _ => Err(CliError::Browser(
            "the daemon closed without a reply".into(),
        )),
    }
}

/// Connect to the session's endpoint; when it is missing or stale run
/// `spawn` and wait up to 3 s for it. `Ok(true)`: it was spawned.
pub fn connect_or_spawn<T: Transport>(
    path: &Path,
    spawn: impl FnOnce() -> Result<(), CliError>,
) -> Result<bool, CliError> {
    if T::connect(path).is_ok() {
        return Ok(false);
    }
    spawn()?;
    let started = Instant::now();
    while started.elapsed() < SPAWN_WAIT {
        if T::connect(path).is_ok() {
            return Ok(true);
        }
        std::thread::sleep(Duration::from_millis(30));
    }
    Err(CliError::Browser(format!(
        "the daemon did not open {} within 3 s",
        path.display()
    )))
}

/// Spawn `current_exe() daemon --session <name> [--cdp <url> | --attach |
/// --launch [--headed] [--profile <dir>]] [--idle <secs>]`, detached (own process
/// group, null stdio), with the policy env for `open`.
fn spawn_daemon<T: Transport>(verb: &Verb) -> Result<(), CliError> {
    let exe = std::env::current_exe().map_err(|err| CliError::Io {
        path: "current_exe".into(),
        message: err.to_string(),
    })?;
    let mut command = std::process::Command::new(&exe);
    command.args(["daemon", "--session", verb.session.as_str()]);
    match verb.browser()? {
        Browser::Cdp(cdp) => {
            command.args(["--cdp", &cdp]);
        }
        Browser::Attach => {
            command.arg("--attach");
        }
        Browser::Launch(own) => {
            command.arg("--launch");
            if own.headed {
                command.arg("--headed");
            }
            if let Some(profile) = &own.profile {
                command.args(["--profile", profile]);
            }
            if let Some(proxy) = &own.proxy {
                command.args(["--proxy", proxy]);
            }
        }
    }
    if let Some(secs) = verb.idle_secs {
        command.args(["--idle", &secs.to_string()]);
    }
    if let Some(file) = &verb.record {
        // Absolute, like `--profile`: the daemon outlives this shell's
        // cwd. The frames dir is made now so a bad path fails this verb,
        // not the daemon halfway through a session.
        let file = match std::path::Path::new(file).is_absolute() {
            true => std::path::PathBuf::from(file),
            false => std::env::current_dir()
                .map_err(|err| CliError::Io {
                    path: file.clone(),
                    message: err.to_string(),
                })?
                .join(file),
        };
        let file = file.display().to_string();
        crate::record::Recorder::prepare(std::path::Path::new(&file)).map_err(|err| {
            CliError::Io {
                path: file.clone(),
                message: err.to_string(),
            }
        })?;
        command.args(["--record", &file]);
    }
    command
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
    // The project's action policy file, checked here so a bad file is
    // an error now, not a daemon that refuses everything.
    let dir = std::env::current_dir().map_err(|err| CliError::Io {
        path: ".".into(),
        message: err.to_string(),
    })?;
    if let Some(file) = ultra_instinct_mcp::find_policy_file(&dir) {
        ultra_instinct_mcp::ToolPolicy::read(&file).map_err(CliError::Config)?;
        command.env(ultra_instinct_mcp::ENV_POLICY_FILE, file);
    }
    T::detach(&mut command);
    command.spawn().map(|_| ()).map_err(|err| CliError::Io {
        path: exe.display().to_string(),
        message: err.to_string(),
    })
}

/// [`run_verb_at`] for the default session endpoint.
pub fn run_verb<T: Transport>(verb: &Verb) -> Result<String, CliError> {
    run_verb_at::<T>(verb, &socket_path(&verb.session), || {
        spawn_daemon::<T>(verb)
    })
}

/// [`run_verb`] against `path`, spawning with `spawn`.
pub fn run_verb_at<T: Transport>(
    verb: &Verb,
    path: &Path,
    spawn: impl FnOnce() -> Result<(), CliError>,
) -> Result<String, CliError> {
    if verb.request == super::Request::Stop {
        if T::connect(path).is_err() {
            return Ok(format!(
                "✓ no daemon for session {}\n",
                verb.session.as_str()
            ));
        }
    } else {
        // A new session must name its browser; nothing is spawned else.
        let spawned = connect_or_spawn::<T>(path, || verb.browser().and_then(|_| spawn()))?;
        if verb.spawn_settings() && !spawned {
            return Err(CliError::Config(format!(
                "session {} is already running; --launch, --attach, --idle and its policy are set when it starts (aui close first)",
                verb.session.as_str()
            )));
        }
        let reply = send::<T>(path, &request_line(&verb.request))?;
        let out = render(verb, &reply)?;
        if spawned && verb.verb == "open" {
            let started = match (verb.launch, verb.attach) {
                (true, _) => " · daemon started with its own Chrome · opened",
                (_, true) => " · daemon attached to your Chrome · opened",
                _ => " · daemon started · opened",
            };
            return Ok(out.replacen(" · opened", started, 1));
        }
        return Ok(out);
    }
    let reply = send::<T>(path, &request_line(&verb.request))?;
    render(verb, &reply)
}
