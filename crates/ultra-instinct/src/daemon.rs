//! Shell verbs: a shell agent drives one open browser with short `aui`
//! commands.
//!
//! ```text
//! aui open --launch https://shop.example/join  # spawns the session daemon (and its Chrome)
//! aui snap                                     # the page, one line per control
//! aui type @e2 "kid@example.com"
//! aui click @e5                                # ✓ ticket 41 · navigation
//! aui wait --text "Welcome" --timeout 10
//! aui close                                    # stops the daemon (and its Chrome)
//! ```
//!
//! One session is one daemon (`aui daemon --session <name>`, hidden) serving
//! `$TMPDIR/aui/<session>.sock` (directory 0700, socket 0600). It answers one
//! connection and one line at a time, in a single thread: each line is a
//! JSON-RPC request for one [`ultra_instinct_mcp::Server`], so a session owns
//! one CDP socket. Every press goes through the ticketed `browser_*`
//! tools. The daemon exits after 60 minutes idle
//! (`aui open --idle <secs>`, 1 s to 24 h, sets it for the session) or on
//! `aui close`, and removes its socket.
//!
//! A verb whose socket is missing or stale spawns the daemon (this binary,
//! detached, null stdio) and waits up to 3 s for the socket. Without
//! `--launch` the daemon attaches to `--cdp` (default
//! `http://127.0.0.1:9222`); when nothing answers there the error says to run
//! `aui open --launch` or `aui doctor`. `aui open --launch [--headed]` (and
//! no `--cdp`) makes the daemon start its own Chrome once, right after it
//! binds its socket; that Chrome lives exactly as long as the daemon
//! (`close`, idle exit and daemon exit all end it). A failed launch answers
//! the waiting verb with the launch error, unchanged, and the daemon exits.
//! `--idle`, `--allow-domain` and `--confirm-press` are start-time settings
//! like `--launch`: a running session refuses them.
//! `--allow-domain` / `--confirm-press` on `open` reach the spawned daemon as
//! `ULTRA_INSTINCT_ALLOW_DOMAINS` / `ULTRA_INSTINCT_CONFIRM_PRESS`;
//! `--confirm <label>` on a press verb is the tool's `confirm` argument.
//!
//! Unix only (std `UnixListener` / `UnixStream`); elsewhere every verb is a
//! clear error.
#![cfg_attr(not(unix), allow(dead_code))]

use serde_json::{json, Value};

use crate::verbs::{parse_verb, render};
use crate::CliError;

/// The session a verb or daemon names when `--session` is not given.
pub const DEFAULT_SESSION: &str = "default";

/// Longest session name: the socket path must fit `sun_path`.
const MAX_SESSION_CHARS: usize = 32;

/// Idle exit when `--idle` is not given: 60 minutes.
pub const DEFAULT_IDLE_SECS: u64 = 60 * 60;

/// Longest `--idle`: 24 hours.
pub const MAX_IDLE_SECS: u64 = 24 * 60 * 60;

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
    /// `--cdp`, when given.
    pub cdp: Option<String>,
    pub request: Request,
    /// `--allow-domain` values (`open` only), for the spawned daemon.
    pub allow_domains: Vec<String>,
    /// `--confirm-press` values (`open` only), for the spawned daemon.
    pub confirm_press: Vec<String>,
    /// `--launch` (`open` only): the spawned daemon starts its own Chrome
    /// unless `--cdp` is given.
    pub launch: bool,
    /// `--headed` (with `--launch`).
    pub headed: bool,
    /// `--idle <secs>` (`open` only): the spawned daemon's idle exit.
    pub idle_secs: Option<u64>,
}

impl Verb {
    /// Whether the spawned daemon launches its own Chrome.
    fn launches(&self) -> bool {
        self.launch && self.cdp.is_none()
    }

    /// Settings that only take effect when this verb spawns the daemon.
    fn spawn_settings(&self) -> bool {
        !self.allow_domains.is_empty()
            || !self.confirm_press.is_empty()
            || self.launch
            || self.idle_secs.is_some()
    }
}

/// An `--idle` value: whole seconds, 1 to [`MAX_IDLE_SECS`].
pub(crate) fn parse_idle(raw: &str) -> Result<u64, CliError> {
    let secs: u64 = raw.trim().parse().map_err(|_| CliError::BadNumber {
        flag: "--idle",
        value: raw.to_owned(),
    })?;
    if secs == 0 || secs > MAX_IDLE_SECS {
        return Err(CliError::Config(format!(
            "--idle {secs}: give 1 to {MAX_IDLE_SECS} seconds (24 h)"
        )));
    }
    Ok(secs)
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

/// `aui <verb> …`.
pub(crate) fn verb_command(verb: &str, args: &[String]) -> Result<String, CliError> {
    let parsed = parse_verb(verb, args)?;
    imp::run_verb(&parsed)
}

/// `aui daemon --session <name> [--cdp <url>] [--launch [--headed]] [--idle <secs>]` (hidden).
pub(crate) fn daemon_command(args: &[String]) -> Result<String, CliError> {
    let mut flags = crate::verbs::Flags::scan(args)?;
    let session = SessionName::parse(
        flags
            .one("--session")?
            .as_deref()
            .unwrap_or(DEFAULT_SESSION),
    )?;
    let cdp = flags.one("--cdp")?;
    let launch = flags.switch("--launch");
    let headed = flags.switch("--headed");
    let idle = flags
        .one("--idle")?
        .map(|raw| parse_idle(&raw))
        .transpose()?
        .unwrap_or(DEFAULT_IDLE_SECS);
    flags.finish()?;
    if let Some(extra) = flags.positional().first() {
        return Err(CliError::UnknownFlag(extra.clone()));
    }
    imp::serve_session(
        &session,
        cdp.as_deref(),
        launch,
        headed,
        std::time::Duration::from_secs(idle),
    )?;
    Ok(String::new())
}

/// `aui session [--session <name>]` prints the session name;
/// `aui session list` lists the live ones and removes stale sockets.
pub(crate) fn session_command(args: &[String]) -> Result<String, CliError> {
    let mut flags = crate::verbs::Flags::scan(args)?;
    let session = flags.one("--session")?;
    flags.finish()?;
    match flags.positional().as_slice() {
        [] => {
            let name = SessionName::parse(session.as_deref().unwrap_or(DEFAULT_SESSION))?;
            Ok(format!("{}\n", name.as_str()))
        }
        [list] if list == "list" && session.is_none() => imp::list_sessions(&imp::socket_dir()),
        _ => Err(CliError::Config(
            "usage: aui session [--session <name>] | aui session list".into(),
        )),
    }
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

/// A connect failure on a daemon that did not launch its Chrome: when
/// nothing answers on `cdp`, say how to get a browser.
pub fn connect_hint(
    err: ultra_instinct_browser::CdpError,
    cdp: &str,
) -> ultra_instinct_browser::CdpError {
    if crate::doctor_cmd::probe_version(cdp).is_ok() {
        return err;
    }
    let detail = match err {
        ultra_instinct_browser::CdpError::Transport { message } => message,
        other => other.to_string(),
    };
    ultra_instinct_browser::CdpError::Transport {
        message: format!(
            "{detail} · nothing answers on {cdp}: run `aui open --launch <url>` (aui starts Chrome) or `aui doctor`"
        ),
    }
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
    use ultra_instinct_browser::launch::{self, ChromeProcess, LaunchOptions};
    use ultra_instinct_browser::{CdpTransport, WebSocketTransport};

    use super::{
        connect_hint, render, request_line, with_endpoint, SessionName, Verb, STOP_METHOD,
    };
    use crate::CliError;

    /// How long a verb waits for a spawned daemon's socket.
    const SPAWN_WAIT: Duration = Duration::from_secs(3);
    /// A verb's wait for one reply (`browser_wait` may take 60 s; a launch 20 s).
    const REPLY_WAIT: Duration = Duration::from_secs(120);
    /// How long a daemon whose Chrome failed to start waits to tell a verb.
    const FAILURE_WAIT: Duration = Duration::from_secs(10);
    /// Accept-loop tick while idle.
    const TICK: Duration = Duration::from_millis(25);

    fn io_err(path: &Path, err: io::Error) -> CliError {
        CliError::Io {
            path: path.display().to_string(),
            message: err.to_string(),
        }
    }

    /// `$TMPDIR/aui` (`aui doctor` may have made it first).
    pub fn socket_dir() -> PathBuf {
        std::env::temp_dir().join("aui")
    }

    /// `$TMPDIR/aui/<session>.sock`.
    pub fn socket_path(session: &SessionName) -> PathBuf {
        socket_dir().join(format!("{}.sock", session.as_str()))
    }

    /// The socket directory, mode 0700, whether this process made it or
    /// found it (refuses one it cannot make private).
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

    /// A daemon that could not start: answer the first request that arrives
    /// within `wait` with `message` (a JSON-RPC error), then stop.
    pub fn serve_failure(daemon: Daemon, message: &str, wait: Duration) {
        let started = Instant::now();
        while started.elapsed() < wait {
            let stream = match daemon.listener.accept() {
                Ok((stream, _)) => stream,
                Err(err) if err.kind() == io::ErrorKind::WouldBlock => {
                    std::thread::sleep(TICK);
                    continue;
                }
                Err(_) => return,
            };
            let answered = (|| -> io::Result<()> {
                stream.set_nonblocking(false)?;
                stream.set_read_timeout(Some(Duration::from_secs(10)))?;
                let mut writer = stream.try_clone()?;
                let mut line = String::new();
                BufReader::new(stream).read_line(&mut line)?;
                let id = serde_json::from_str::<Value>(&line)
                    .ok()
                    .and_then(|v| v.get("id").cloned())
                    .unwrap_or(Value::Null);
                let reply = json!({"jsonrpc": "2.0", "id": id,
                                   "error": {"code": -32000, "message": message}});
                writeln!(writer, "{reply}")
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
        ultra_instinct_mcp::Server::with_connector(move |url| {
            WebSocketTransport::connect(url)
                .map(|transport| Box::new(transport) as Box<dyn CdpTransport>)
                .map_err(|err| if hint { connect_hint(err, &cdp) } else { err })
        })
        .with_env_policy()
    }

    /// The hidden `aui daemon` body.
    pub(super) fn serve_session(
        session: &SessionName,
        cdp: Option<&str>,
        launch: bool,
        headed: bool,
        idle: Duration,
    ) -> Result<(), CliError> {
        // Bind first: the spawning verb is already waiting on this socket,
        // and a launch can take longer than its 3 s spawn wait.
        let daemon = Daemon::bind(&socket_path(session))?;
        let chrome = match (launch, cdp) {
            (true, None) => match launch::launch(&LaunchOptions {
                headed,
                ..LaunchOptions::default()
            }) {
                Ok(chrome) => Some(chrome),
                Err(err) => {
                    let message = err.to_string();
                    serve_failure(daemon, &message, FAILURE_WAIT);
                    return Err(CliError::Browser(message));
                }
            },
            _ => None,
        };
        let endpoint = match (&chrome, cdp) {
            (Some(chrome), _) => chrome.endpoint(),
            (None, Some(cdp)) => cdp.to_owned(),
            (None, None) => ultra_instinct_browser::DEFAULT_CDP_HTTP.to_owned(),
        };
        let mut owned = Owned {
            server: daemon_server(&endpoint, chrome.is_none()),
            _chrome: chrome,
        };
        // `daemon` (the socket) is dropped inside; `owned` after, in order.
        serve_on(daemon, &mut owned.server, &endpoint, idle)
    }

    /// `aui session list` over `dir`: each `*.sock` that answers is a live
    /// session; the rest are removed and named.
    pub(super) fn list_sessions(dir: &Path) -> Result<String, CliError> {
        let entries = match std::fs::read_dir(dir) {
            Ok(entries) => entries,
            Err(err) if err.kind() == io::ErrorKind::NotFound => {
                return Ok("no sessions\n".to_owned())
            }
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
            if UnixStream::connect(&path).is_ok() {
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

    /// Spawn `current_exe() daemon --session <name> [--cdp <url>] [--launch
    /// [--headed]] [--idle <secs>]`, detached (own process group, null stdio), with the
    /// policy env for `open`.
    fn spawn_daemon(verb: &Verb) -> Result<(), CliError> {
        let exe = std::env::current_exe().map_err(|err| CliError::Io {
            path: "current_exe".into(),
            message: err.to_string(),
        })?;
        let mut command = std::process::Command::new(&exe);
        command.args(["daemon", "--session", verb.session.as_str()]);
        if let Some(cdp) = &verb.cdp {
            command.args(["--cdp", cdp]);
        }
        if verb.launches() {
            command.arg("--launch");
            if verb.headed {
                command.arg("--headed");
            }
        }
        if let Some(secs) = verb.idle_secs {
            command.args(["--idle", &secs.to_string()]);
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
        if verb.request == super::Request::Stop {
            if UnixStream::connect(path).is_err() {
                return Ok(format!(
                    "✓ no daemon for session {}\n",
                    verb.session.as_str()
                ));
            }
        } else {
            let spawned = connect_or_spawn(path, spawn)?;
            if verb.spawn_settings() && !spawned {
                return Err(CliError::Config(format!(
                    "session {} is already running; --launch, --idle and its policy are set when it starts (aui close first)",
                    verb.session.as_str()
                )));
            }
            let reply = send(path, &request_line(&verb.request))?;
            let out = render(verb, &reply)?;
            if spawned && verb.verb == "open" {
                let started = if verb.launches() {
                    " · daemon started with its own Chrome · opened"
                } else {
                    " · daemon started · opened"
                };
                return Ok(out.replacen(" · opened", started, 1));
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

    pub(super) fn serve_session(
        _session: &SessionName,
        _cdp: Option<&str>,
        _launch: bool,
        _headed: bool,
        _idle: std::time::Duration,
    ) -> Result<(), CliError> {
        Err(CliError::Config(UNSUPPORTED.into()))
    }

    pub fn socket_dir() -> std::path::PathBuf {
        std::env::temp_dir().join("aui")
    }

    pub(super) fn list_sessions(_dir: &std::path::Path) -> Result<String, CliError> {
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
    fn session_prints_its_name() {
        assert_eq!(session_command(&[]).unwrap(), "default\n");
        assert_eq!(
            session_command(&a(&["--session", "shop-2"])).unwrap(),
            "shop-2\n"
        );
        assert!(session_command(&a(&["--session", "a b"])).is_err());
        assert!(session_command(&a(&["kill"])).is_err());
        assert!(session_command(&a(&["list", "--session", "x"])).is_err());
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

    /// Nothing listens on the endpoint: the connect error says how to get a
    /// browser, and keeps the original error text.
    #[test]
    fn a_dead_endpoint_hints_launch_and_doctor() {
        let port = std::net::TcpListener::bind("127.0.0.1:0")
            .unwrap()
            .local_addr()
            .unwrap()
            .port();
        let cdp = format!("http://127.0.0.1:{port}");
        let err = connect_hint(
            ultra_instinct_browser::CdpError::Transport {
                message: "connection refused".into(),
            },
            &cdp,
        );
        let text = err.to_string();
        assert!(text.contains("connection refused"), "{text}");
        assert!(
            text.contains("aui open --launch") && text.contains("aui doctor"),
            "{text}"
        );
    }

    #[test]
    fn daemon_flags_are_checked() {
        for bad in [
            &["--session"][..],
            &["--session", "a", "--session", "b"][..],
            &["--headless"][..],
            &["extra"][..],
        ] {
            assert!(daemon_command(&a(bad)).is_err(), "{bad:?}");
        }
    }

    #[cfg(unix)]
    mod socket {
        use super::super::imp::{list_sessions, run_verb_at as run_verb_with, serve_failure};
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

        /// Read-only verbs of the new tool families round-trip too: `count`
        /// over a fresh observe, `is` over the same observation.
        #[test]
        fn count_and_is_round_trip_through_the_socket() {
            let path = path("ci");
            let script = ScriptBuilder::new()
                .observe_compact(&page())
                .observe_compact(&page());
            let handle = start(&path, script);
            let no_spawn = || panic!("the daemon is running");
            let count = parse_verb("count", &a(&["--role", "button"])).unwrap();
            assert_eq!(run_verb_with(&count, &path, no_spawn).unwrap(), "1  @e2\n");
            let is = parse_verb("is", &a(&["enabled", "@e2"])).unwrap();
            assert_eq!(run_verb_with(&is, &path, no_spawn).unwrap(), "true\n");
            let close = parse_verb("close", &[]).unwrap();
            run_verb_with(&close, &path, no_spawn).unwrap();
            handle.join().unwrap();
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
            // Spawn-time settings on a running session are refused.
            let launch = parse_verb("open", &a(&["https://a/", "--launch"])).unwrap();
            let err = run_verb_with(&launch, &path, || panic!("running")).unwrap_err();
            assert!(err.to_string().contains("already running"), "{err}");
            let idle = parse_verb("open", &a(&["https://a/", "--idle", "5"])).unwrap();
            let err = run_verb_with(&idle, &path, || panic!("running")).unwrap_err();
            assert!(
                err.to_string().contains("already running") && err.to_string().contains("--idle"),
                "{err}"
            );
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

        /// `aui daemon --idle 1` (what `aui open --idle 1` spawns) binds its
        /// session socket, serves, and exits about a second after the last
        /// request, removing the socket.
        #[test]
        fn the_idle_flag_sets_the_daemon_idle_exit() {
            let name = format!("t-idle-{}", std::process::id());
            let session = SessionName::parse(&name).unwrap();
            let socket = super::super::imp::socket_path(&session);
            let started = std::time::Instant::now();
            let daemon = std::thread::spawn(move || {
                // Attaches to an endpoint nobody serves: idle needs no browser.
                daemon_command(&a(&[
                    "--session",
                    &name,
                    "--cdp",
                    "http://127.0.0.1:9",
                    "--idle",
                    "1",
                ]))
            });
            while !socket.exists() {
                assert!(started.elapsed() < Duration::from_secs(5), "never bound");
                std::thread::sleep(Duration::from_millis(10));
            }
            // A request resets the idle clock; the daemon still answers it.
            let line = request_line(&Request::Tool {
                name: "browser_dialog",
                arguments: json!({"action": "status"}),
            });
            let reply = super::super::imp::send(&socket, &line).unwrap();
            assert!(reply.contains("\"jsonrpc\""), "{reply}");
            let asked = std::time::Instant::now();
            assert_eq!(daemon.join().unwrap().unwrap(), "");
            let idle_for = asked.elapsed();
            assert!(
                idle_for >= Duration::from_millis(900) && idle_for < Duration::from_secs(5),
                "exited {idle_for:?} after the last request"
            );
            assert!(!socket.exists(), "socket removed on idle exit");
            for bad in ["0", "86401"] {
                assert!(daemon_command(&a(&["--idle", bad])).is_err(), "{bad}");
            }
        }

        /// A directory someone else made first (`aui doctor` makes it 0700)
        /// is reused, and made private if it was not.
        #[test]
        fn a_socket_dir_made_before_the_daemon_is_reused() {
            let path = path("pre");
            let dir = path.parent().unwrap();
            std::fs::create_dir_all(dir).unwrap();
            std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o755)).unwrap();
            let daemon = Daemon::bind(&path).unwrap();
            let mode = std::fs::metadata(dir).unwrap().permissions().mode() & 0o777;
            assert_eq!(mode, 0o700);
            drop(daemon);
            let _ = std::fs::remove_dir(dir);
        }

        /// A daemon whose Chrome did not start answers the waiting verb with
        /// the launch error text, unchanged, then exits and removes its socket.
        #[test]
        fn a_failed_launch_reaches_the_verb_unchanged() {
            let path = path("lf");
            let message = ultra_instinct_browser::launch::LaunchError::NotFound.to_string();
            let mut failing = None;
            let open = parse_verb("open", &a(&["https://a/", "--launch"])).unwrap();
            let err = run_verb_with(&open, &path, || {
                // The spawned daemon binds, then its launch fails.
                let daemon = Daemon::bind(&path).unwrap();
                let message = message.clone();
                failing = Some(std::thread::spawn(move || {
                    serve_failure(daemon, &message, Duration::from_secs(5))
                }));
                Ok(())
            })
            .unwrap_err();
            assert_eq!(err.to_string(), format!("✗ error: {message}"));
            failing.unwrap().join().unwrap();
            assert!(!path.exists());
            let _ = std::fs::remove_dir(path.parent().unwrap());
        }

        #[test]
        fn session_list_names_live_sockets_and_removes_stale_ones() {
            let live = path("ls");
            let dir = live.parent().unwrap().to_owned();
            let handle = start(&live, ScriptBuilder::new());
            drop(std::os::unix::net::UnixListener::bind(dir.join("old.sock")).unwrap());
            std::fs::write(dir.join("notes.txt"), b"x").unwrap();
            let out = list_sessions(&dir).unwrap();
            assert_eq!(
                out, "s\n1 live session\nremoved stale socket old.sock\n",
                "{out}"
            );
            assert!(!dir.join("old.sock").exists());
            assert!(dir.join("notes.txt").exists(), "only sockets are touched");
            let close = parse_verb("close", &[]).unwrap();
            run_verb_with(&close, &live, || panic!("running")).unwrap();
            handle.join().unwrap();
            assert_eq!(list_sessions(&dir).unwrap(), "no sessions\n");
            assert_eq!(
                list_sessions(&dir.join("missing")).unwrap(),
                "no sessions\n"
            );
            let _ = std::fs::remove_file(dir.join("notes.txt"));
            let _ = std::fs::remove_dir(&dir);
        }
    }
}
