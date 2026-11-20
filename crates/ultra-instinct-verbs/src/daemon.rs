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
//! `$TMPDIR/aui/<session>.sock` (directory 0700, socket 0600) on unix — a
//! marker file plus a per-user named pipe on Windows. It answers one
//! connection and one line at a time, in a single thread: each line is a
//! JSON-RPC request for one [`ultra_instinct_mcp::Server`], so a session owns
//! one CDP socket. Every press goes through the ticketed `browser_*`
//! tools. The daemon exits after 60 minutes idle
//! (`aui open --idle <secs>`, 1 s to 24 h, sets it for the session) or on
//! `aui close`, and removes its socket.
//!
//! A verb whose socket is missing or stale spawns the daemon (this binary,
//! detached, null stdio) and waits up to 3 s for the socket. A verb that
//! would spawn one must name its browser: `--cdp <url>` attaches there, and
//! with neither `--cdp` nor `--launch` it refuses (`NO_BROWSER`) instead
//! of guessing a port, which could be the user's own Chrome; when nothing
//! answers at `--cdp` the error says to run `aui open --launch` or `aui
//! doctor`. `aui open --attach --allow-domain <host>` drives the user's own
//! Chrome over the browser socket its remote-debugging toggle opens, in a
//! tab the daemon opens; `close` and idle exit close only that tab (never
//! the user's tabs or Chrome). `aui open --launch [--headed] [--profile <dir>]` makes the
//! daemon start its own Chrome once, right after it binds its socket
//! (`--launch` with `--cdp` refuses: pick one); that Chrome lives exactly as
//! long as the daemon (`close`, idle exit and daemon exit all end it); with
//! `--profile` it runs on that profile folder, which stays on disk, and is
//! asked to quit (up to 5 s) before it is killed. `--profile` refuses an
//! empty value, a browser's own everyday profile folder, and a profile a
//! running Chrome holds. A failed launch answers
//! the waiting verb with the launch error, unchanged, and the daemon exits.
//! `--idle`, `--allow-domain` and `--confirm-press` are start-time settings
//! like `--launch`: a running session refuses them.
//! `--allow-domain` / `--confirm-press` on `open` reach the spawned daemon as
//! `ULTRA_INSTINCT_ALLOW_DOMAINS` / `ULTRA_INSTINCT_CONFIRM_PRESS`;
//! `--confirm <label>` on a press verb is the tool's `confirm` argument.
//!
//! A `--launch` session also keeps its login across `close` and reopen:
//! the daemon restores `<state dir>/<session>.json` (the `browser_state_*`
//! format, cookies plus the last origin's storage) when it starts and saves
//! it when it ends, however it ends. `AUI_SESSION_STATE=0` turns that off;
//! `AUI_STATE_DIR` moves the directory. `--cdp` and `--attach` sessions keep
//! no auto state: their browsers are the user's own.
//!
//! `aui open --record <file>` records the session on any browser kind: a
//! `browser_screenshot` frame (secret fields hidden, as `aui shot`) after
//! each answered tool call, throttled to one per 400 ms, into
//! `<file>.frames/`; when the daemon ends, `ffmpeg` assembles them into
//! `<file>` when it is on `PATH`, else the frames and an `index.ffconcat`
//! list stand on their own.
//!
//! The session endpoint is a socket file in a 0700 directory on unix and
//! a named pipe plus a marker file on Windows; elsewhere every verb is a
//! clear error.
#![cfg_attr(not(any(unix, windows)), allow(dead_code))]

use serde_json::{json, Value};

use crate::verbs::parse_verb;
use crate::CliError;

/// The session a verb or daemon names when `--session` is not given.
pub const DEFAULT_SESSION: &str = "default";

/// Longest session name: the endpoint path (socket file or marker) must
/// stay short on every platform.
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
    /// `--profile <dir>` (with `--launch`), absolute: the launched Chrome's
    /// user-data dir, kept on disk.
    pub profile: Option<String>,
    /// `--idle <secs>` (`open` only): the spawned daemon's idle exit.
    pub idle_secs: Option<u64>,
    /// `--attach` (`open` only): the spawned daemon drives the user's own
    /// Chrome through its remote-debugging toggle.
    pub attach: bool,
    /// `--proxy <url>` (with `--launch`): the launched Chrome's proxy.
    pub proxy: Option<String>,
    /// `--record <file>` (`open` only): the spawned daemon records the
    /// session's page into `<file>` (frames in `<file>.frames/`).
    pub record: Option<String>,
    /// `--boundary` (or `AUI_CONTENT_BOUNDARIES`): page content in the
    /// output sits between two marker lines the page cannot forge.
    pub boundary: bool,
}

impl Verb {
    /// The browser a daemon this verb spawns runs on.
    fn browser(&self) -> Result<Browser, CliError> {
        let own = Launch {
            headed: self.headed,
            profile: self.profile.clone(),
            proxy: self.proxy.clone(),
        };
        if self.attach {
            return attach_browser();
        }
        Browser::named(self.cdp.as_deref(), self.launch.then_some(own))
    }

    /// Settings that only take effect when this verb spawns the daemon.
    fn spawn_settings(&self) -> bool {
        !self.allow_domains.is_empty()
            || !self.confirm_press.is_empty()
            || self.launch
            || self.attach
            || self.idle_secs.is_some()
            || self.record.is_some()
    }
}

/// `--attach` drives the user's own Chrome over the browser socket its
/// remote-debugging toggle opens — a session-multiplexing browser that is
/// unix-only for now.
#[cfg(not(unix))]
pub(crate) const ATTACH_NEEDS_UNIX: &str = "--attach drives your own Chrome over its \
remote-debugging socket, which needs macOS or Linux for now; use --launch or --cdp";

/// The refusal of a verb that would start a session with no browser named.
pub(crate) const NO_BROWSER: &str = "no browser named for a new session: \
pass --launch (its own Chrome; --profile <dir> keeps a profile) or --cdp <url> \
(a Chrome you started with --remote-debugging-port and its own --user-data-dir)";

/// How the daemon starts its own Chrome (`--launch`).
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct Launch {
    pub headed: bool,
    /// `--profile <dir>`: the user-data dir, kept on disk.
    pub profile: Option<String>,
    /// `--proxy <url>`: every request of the launched Chrome goes through it.
    pub proxy: Option<String>,
}

/// The one browser a new session runs on.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Browser {
    /// `--cdp <url>`: a Chrome the user started.
    Cdp(String),
    /// `--launch`: the daemon's own Chrome.
    Launch(Launch),
    /// `--attach`: the user's own Chrome, over the browser websocket its
    /// remote-debugging toggle opens. Only the tabs the daemon opens close.
    Attach,
}

impl Browser {
    /// The browser `--cdp` or `--launch` names; neither is [`NO_BROWSER`].
    pub(crate) fn named(cdp: Option<&str>, launch: Option<Launch>) -> Result<Self, CliError> {
        match (cdp, launch) {
            (Some(cdp), None) => Ok(Self::Cdp(cdp.to_owned())),
            (None, Some(own)) => Ok(Self::Launch(own)),
            (Some(_), Some(_)) => Err(CliError::Config(LAUNCH_OR_CDP.into())),
            (None, None) => Err(CliError::Config(NO_BROWSER.into())),
        }
    }
}

const LAUNCH_OR_CDP: &str = "--launch and --cdp: pick one";

/// The `--attach` rules of `open` and the daemon: none of the flags that
/// start or name another browser (`[launch, cdp, profile, headed]` given),
/// and at least one allowed domain.
pub(crate) fn check_attach(
    attach: bool,
    others: [bool; 4],
    allow_domains: bool,
) -> Result<(), CliError> {
    if !attach {
        return Ok(());
    }
    let flags = ["--launch", "--cdp", "--profile", "--headed"];
    if let Some((_, flag)) = others.iter().zip(flags).find(|(given, _)| **given) {
        return Err(CliError::Config(format!("--attach and {flag}: pick one")));
    }
    if !allow_domains {
        return Err(CliError::Config(
            "--attach drives your own logged-in Chrome: name the sites with --allow-domain".into(),
        ));
    }
    Ok(())
}

/// `Browser::Attach` where the session-multiplexing browser exists.
#[cfg(unix)]
fn attach_browser() -> Result<Browser, CliError> {
    Ok(Browser::Attach)
}

/// `Browser::Attach` elsewhere is a clear refusal, before any daemon
/// spawns (the daemon path refuses it too, for a direct `aui daemon`).
#[cfg(not(unix))]
fn attach_browser() -> Result<Browser, CliError> {
    Err(CliError::Config(ATTACH_NEEDS_UNIX.into()))
}

/// The `--launch` / `--cdp` / `--profile` rules of `open`, `run` and the
/// daemon: not both `--launch` and `--cdp`; `--profile` needs `--launch`,
/// a folder name, and not one of Chrome's own profiles. Returns the
/// profile as an absolute path (the daemon does not share this shell's
/// working directory).
pub fn launch_profile(
    launch: bool,
    cdp: bool,
    profile: Option<&str>,
) -> Result<Option<String>, CliError> {
    if launch && cdp {
        return Err(CliError::Config(LAUNCH_OR_CDP.into()));
    }
    let Some(dir) = profile else { return Ok(None) };
    if !launch {
        return Err(CliError::Config(
            "--profile needs --launch: it is the launched Chrome's profile".into(),
        ));
    }
    if dir.trim().is_empty() {
        return Err(CliError::Config("--profile needs a folder".into()));
    }
    let path = std::path::Path::new(dir);
    let path = match path.is_absolute() {
        true => path.to_path_buf(),
        false => std::env::current_dir()
            .map_err(|err| CliError::Io {
                path: dir.to_owned(),
                message: err.to_string(),
            })?
            .join(path),
    };
    if ultra_instinct_browser::launch::is_default_user_data_dir(&path) {
        return Err(CliError::Config(format!(
            "--profile {}: that is a browser's own everyday profile; use a separate folder",
            path.display()
        )));
    }
    Ok(Some(path.display().to_string()))
}

/// A `--proxy` value for a launched Chrome: `http`, `https`, `socks4` or
/// `socks5`, a host and a port, and nothing else. Chrome takes no user name
/// or password here, so one is refused instead of dropped.
pub fn launch_proxy(launch: bool, proxy: Option<&str>) -> Result<Option<String>, CliError> {
    let Some(proxy) = proxy else { return Ok(None) };
    if !launch {
        return Err(CliError::Config(
            "--proxy needs --launch: it is the launched Chrome's proxy".into(),
        ));
    }
    let bad = |why: &str| CliError::Config(format!("--proxy {proxy}: {why}"));
    let (scheme, rest) = proxy
        .split_once("://")
        .ok_or_else(|| bad("give scheme://host:port"))?;
    if !["http", "https", "socks4", "socks5"].contains(&scheme) {
        return Err(bad("the scheme must be http, https, socks4 or socks5"));
    }
    if rest.contains('@') {
        return Err(bad("a user name or password is not supported"));
    }
    let (host, port) = rest
        .rsplit_once(':')
        .ok_or_else(|| bad("give scheme://host:port"))?;
    let plain = |c: char| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '[' | ']' | ':');
    if host.is_empty() || !host.chars().all(plain) || port.parse::<u16>().map_or(true, |p| p == 0) {
        return Err(bad("give scheme://host:port"));
    }
    Ok(Some(proxy.to_owned()))
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
pub fn verb_command(verb: &str, args: &[String]) -> Result<String, CliError> {
    let parsed = parse_verb(verb, args)?;
    imp::run_verb(&parsed)
}

/// `aui daemon --session <name> (--cdp <url> | --launch [--headed] [--profile <dir>])
/// [--idle <secs>] [--record <file>]` (hidden).
pub fn daemon_command(args: &[String]) -> Result<String, CliError> {
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
    let attach = flags.switch("--attach");
    let profile = flags.one("--profile")?;
    let allow = std::env::var(ultra_instinct_mcp::ENV_ALLOW_DOMAINS).unwrap_or_default();
    let others = [launch, cdp.is_some(), profile.is_some(), headed];
    check_attach(attach, others, !allow.trim().is_empty())?;
    let profile = launch_profile(launch, cdp.is_some(), profile.as_deref())?;
    let proxy = launch_proxy(launch, flags.one("--proxy")?.as_deref())?;
    let idle = flags
        .one("--idle")?
        .map(|raw| parse_idle(&raw))
        .transpose()?
        .unwrap_or(DEFAULT_IDLE_SECS);
    let record = flags.one("--record")?;
    if record.as_deref() == Some("") {
        return Err(CliError::Config("--record needs a file path".into()));
    }
    flags.finish()?;
    if let Some(extra) = flags.positional().first() {
        return Err(CliError::UnknownFlag(extra.clone()));
    }
    if headed && !launch {
        return Err(CliError::Config("--headed needs --launch".into()));
    }
    let browser = match attach {
        true => Browser::Attach,
        false => Browser::named(
            cdp.as_deref(),
            launch.then_some(Launch {
                headed,
                profile,
                proxy,
            }),
        )?,
    };
    imp::serve_session(
        &session,
        browser,
        std::time::Duration::from_secs(idle),
        record.map(std::path::PathBuf::from),
    )?;
    Ok(String::new())
}

/// `aui session [--session <name>]` prints the session name;
/// `aui session list` lists the live ones and removes stale sockets.
pub fn session_command(args: &[String]) -> Result<String, CliError> {
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
    if crate::probe::probe_version(cdp).is_ok() {
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

mod session;

#[cfg(all(any(unix, windows), test))]
use imp::{serve_on, Daemon};

#[cfg(unix)]
mod imp {
    use std::os::unix::fs::{DirBuilderExt, PermissionsExt};
    use std::os::unix::net::{UnixListener, UnixStream};
    use std::path::{Path, PathBuf};
    use std::process::Command;
    use std::time::Duration;

    use super::session;
    use super::{Browser, SessionName, Verb};
    use crate::CliError;

    /// The unix session transport: a socket file in a 0700 directory.
    pub struct Transport;

    impl session::SessionStream for UnixStream {
        fn set_nonblocking(&self, on: bool) -> std::io::Result<()> {
            UnixStream::set_nonblocking(self, on)
        }
        fn set_recv_timeout(&self, timeout: Option<Duration>) -> std::io::Result<()> {
            self.set_read_timeout(timeout)
        }
    }

    impl session::SessionListener for UnixListener {
        type Stream = UnixStream;
        fn accept(&self) -> std::io::Result<UnixStream> {
            UnixListener::accept(self).map(|(stream, _)| stream)
        }
    }

    impl session::Transport for Transport {
        type Stream = UnixStream;
        type Listener = UnixListener;

        fn private_dir(dir: &Path) -> Result<(), CliError> {
            std::fs::DirBuilder::new()
                .recursive(true)
                .mode(0o700)
                .create(dir)
                .map_err(|err| session::io_err(dir, err))?;
            std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700))
                .map_err(|err| session::io_err(dir, err))
        }

        /// `$TMPDIR/aui` state files: `~/Library/Application Support/aui`
        /// on macOS, `${XDG_STATE_HOME:-~/.local/state}/aui` elsewhere.
        fn state_dir() -> Option<PathBuf> {
            if let Some(dir) = session::state_dir_override() {
                return Some(dir);
            }
            let home = std::env::var_os("HOME").map(PathBuf::from)?;
            #[cfg(target_os = "macos")]
            let dir = home.join("Library/Application Support/aui");
            #[cfg(not(target_os = "macos"))]
            let dir = std::env::var_os("XDG_STATE_HOME")
                .map(PathBuf::from)
                .filter(|d| d.is_absolute())
                .unwrap_or_else(|| home.join(".local/state"))
                .join("aui");
            Some(dir)
        }

        fn open(path: &Path) -> Result<UnixListener, CliError> {
            if path.exists() {
                if UnixStream::connect(path).is_ok() {
                    return Err(CliError::Config(format!(
                        "a daemon already serves {}",
                        path.display()
                    )));
                }
                std::fs::remove_file(path).map_err(|err| session::io_err(path, err))?;
            }
            let listener = UnixListener::bind(path).map_err(|err| session::io_err(path, err))?;
            std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))
                .map_err(|err| session::io_err(path, err))?;
            listener
                .set_nonblocking(true)
                .map_err(|err| session::io_err(path, err))?;
            Ok(listener)
        }

        fn connect(path: &Path) -> std::io::Result<UnixStream> {
            UnixStream::connect(path)
        }

        fn detach(command: &mut Command) {
            std::os::unix::process::CommandExt::process_group(command, 0);
        }
    }

    /// A bound unix endpoint, for the session tests.
    #[cfg(test)]
    pub type Daemon = session::Daemon<Transport>;

    pub(super) fn serve_session(
        session: &SessionName,
        browser: Browser,
        idle: Duration,
        record: Option<PathBuf>,
    ) -> Result<(), CliError> {
        session::serve_session::<Transport>(session, browser, idle, record)
    }

    pub(super) fn run_verb(verb: &Verb) -> Result<String, CliError> {
        session::run_verb::<Transport>(verb)
    }

    pub(super) fn list_sessions(dir: &Path) -> Result<String, CliError> {
        session::list_sessions::<Transport>(dir)
    }

    #[cfg(test)]
    pub(super) fn send(path: &Path, line: &str) -> Result<String, CliError> {
        session::send::<Transport>(path, line)
    }

    #[cfg(test)]
    pub(super) fn run_verb_at(
        verb: &Verb,
        path: &Path,
        spawn: impl FnOnce() -> Result<(), CliError>,
    ) -> Result<String, CliError> {
        session::run_verb_at::<Transport>(verb, path, spawn)
    }

    pub use session::socket_dir;
    #[cfg(test)]
    pub use session::{reply_ok, serve_failure, serve_on, socket_path, state_value_off};
}

#[cfg(windows)]
mod imp {
    use std::ffi::OsString;
    use std::io::{self, Write};
    use std::os::windows::process::CommandExt;
    use std::path::{Path, PathBuf};
    use std::process::Command;
    use std::time::Duration;

    use interprocess::local_socket::prelude::*;
    use interprocess::local_socket::traits::{Listener as IpListener, Stream as IpStream};
    use interprocess::local_socket::{
        ConnectOptions, GenericNamespaced, ListenerNonblockingMode, ListenerOptions, Name,
    };
    use interprocess::os::windows::local_socket::ListenerOptionsExt;
    use interprocess::os::windows::security_descriptor::SecurityDescriptor;

    use super::session;
    use super::{Browser, SessionName, Verb};
    use crate::CliError;

    /// `CREATE_NO_WINDOW`: the hidden daemon gets its own invisible
    /// console instead of sharing (and outliving) the caller's.
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;

    /// The Windows session transport: a named pipe per session plus a
    /// marker file where the unix socket would be (`<dir>/<session>.sock`).
    /// The marker is created exclusively, so it is the session's lock; the
    /// pipe's name derives from the marker path, whose per-user temp dir
    /// keeps one user's sessions out of another's.
    pub struct Transport;

    /// A named-pipe stream. Windows pipes have no kernel read timeout
    /// (`set_recv_timeout` is unsupported), so the pipe stays nonblocking
    /// and `read` / `write` poll to blocking semantics; the stored timeout
    /// bounds each read exactly like a unix socket's `SO_RCVTIMEO`.
    pub struct PipeStream {
        inner: LocalSocketStream,
        recv_timeout: std::cell::Cell<Option<Duration>>,
    }

    impl PipeStream {
        fn new(inner: LocalSocketStream) -> io::Result<Self> {
            IpStream::set_nonblocking(&inner, true)?;
            Ok(Self {
                inner,
                recv_timeout: std::cell::Cell::new(None),
            })
        }
    }

    impl session::SessionStream for PipeStream {
        fn set_nonblocking(&self, _on: bool) -> io::Result<()> {
            // The pipe is always nonblocking; `read`/`write` block instead.
            Ok(())
        }
        fn set_recv_timeout(&self, timeout: Option<Duration>) -> io::Result<()> {
            self.recv_timeout.set(timeout);
            Ok(())
        }
    }

    impl session::SessionListener for LocalSocketListener {
        type Stream = PipeStream;
        fn accept(&self) -> io::Result<PipeStream> {
            IpListener::accept(self).and_then(PipeStream::new)
        }
    }

    /// `ERROR_PIPE_BUSY` (a full nonblocking pipe buffer) reaches `write`
    /// as an uncategorized OS error, not `WouldBlock`.
    fn pipe_would_block(err: &io::Error) -> bool {
        err.kind() == io::ErrorKind::WouldBlock || err.raw_os_error() == Some(231)
    }

    impl std::io::Read for PipeStream {
        fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
            let deadline = self
                .recv_timeout
                .get()
                .map(|timeout| std::time::Instant::now() + timeout);
            loop {
                match self.inner.read(buf) {
                    Err(err) if err.kind() == io::ErrorKind::WouldBlock => {}
                    // interprocess downgrades "no data yet" and "peer
                    // closed" to the same Ok(0); with a deadline armed both
                    // poll until it (a real close then surfaces as the
                    // timeout, like SO_RCVTIMEO). Without one, Ok(0) is EOF.
                    Ok(0) if deadline.is_some() => {}
                    result => return result,
                }
                if deadline.is_some_and(|at| std::time::Instant::now() >= at) {
                    return Err(io::Error::new(
                        io::ErrorKind::WouldBlock,
                        "session pipe read timed out",
                    ));
                }
                std::thread::sleep(Duration::from_millis(5));
            }
        }
    }

    impl std::io::Write for PipeStream {
        fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
            loop {
                match self.inner.write(buf) {
                    Err(err) if pipe_would_block(&err) => {}
                    // A full nonblocking pipe takes nothing and reports
                    // success; a closed peer is an error, not Ok(0).
                    Ok(0) if !buf.is_empty() => {}
                    result => return result,
                }
                std::thread::sleep(Duration::from_millis(5));
            }
        }
        fn flush(&mut self) -> io::Result<()> {
            self.inner.flush()
        }
    }

    impl session::Transport for Transport {
        type Stream = PipeStream;
        type Listener = LocalSocketListener;

        /// `%TEMP%` is already per-user; the `aui` subdir inherits that.
        fn private_dir(dir: &Path) -> Result<(), CliError> {
            std::fs::create_dir_all(dir).map_err(|err| session::io_err(dir, err))
        }

        /// `%LOCALAPPDATA%\aui`, the Windows `~/.local/state` analog; the
        /// per-user temp dir when `LOCALAPPDATA` is unset.
        fn state_dir() -> Option<PathBuf> {
            if let Some(dir) = session::state_dir_override() {
                return Some(dir);
            }
            if let Some(local) = std::env::var_os("LOCALAPPDATA") {
                return Some(PathBuf::from(local).join("aui"));
            }
            Some(std::env::temp_dir().join("aui-state"))
        }

        fn open(path: &Path) -> Result<LocalSocketListener, CliError> {
            // Claim the marker exclusively; it is the session's lock. A
            // marker whose pipe does not answer is stale and is replaced;
            // one whose pipe answers is a live daemon.
            let name = pipe_name(path);
            for _ in 0..40 {
                match std::fs::OpenOptions::new()
                    .write(true)
                    .create_new(true)
                    .open(path)
                {
                    Ok(mut marker) => {
                        let listener = match bind_pipe(&name) {
                            Ok(listener) => listener,
                            Err(err) => {
                                drop(marker);
                                let _ = std::fs::remove_file(path);
                                return Err(err);
                            }
                        };
                        // For humans (`type default.sock`); connect()
                        // derives the name from the path, not this text.
                        let _ = marker.write_all(name.as_bytes());
                        return Ok(listener);
                    }
                    Err(err) if err.kind() == io::ErrorKind::AlreadyExists => {
                        // The marker precedes the pipe bind by a few ms; a
                        // daemon still starting answers no probe yet. Give
                        // it a moment across several probes before taking
                        // the marker over, or two daemons could share one
                        // pipe name. A live probe writes a blank line so the
                        // daemon closes the connection at once — a named
                        // pipe cannot tell an idle connection from a closed
                        // one, and a bare connect would hold it for the
                        // whole line timeout.
                        let mut live = false;
                        for _ in 0..8 {
                            if let Ok(mut stream) = connect_name(&name) {
                                let _ = stream.write_all(b"\n");
                                live = true;
                                break;
                            }
                            std::thread::sleep(Duration::from_millis(25));
                        }
                        if live {
                            return Err(CliError::Config(format!(
                                "a daemon already serves {}",
                                path.display()
                            )));
                        }
                        if std::fs::remove_file(path).is_ok() || !path.exists() {
                            continue;
                        }
                    }
                    Err(err) => return Err(session::io_err(path, err)),
                }
            }
            Err(CliError::Config(format!(
                "the session endpoint {} stayed busy; try again",
                path.display()
            )))
        }

        fn connect(path: &Path) -> io::Result<PipeStream> {
            connect_name(&pipe_name(path))
        }

        fn detach(command: &mut Command) {
            command.creation_flags(CREATE_NO_WINDOW);
        }
    }

    /// `aui-<fnv-1a of the marker path>`: hashing the path (which lives in
    /// the per-user temp dir) keeps sessions apart across users.
    fn pipe_name(path: &Path) -> String {
        let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
        for byte in path.to_string_lossy().as_bytes() {
            hash ^= u64::from(*byte);
            hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
        }
        format!("aui-{hash:016x}")
    }

    /// `name` as a local-socket name (`\\.\pipe\<name>` on Windows).
    fn socket_name(name: &str) -> io::Result<Name<'static>> {
        OsString::from(name)
            .to_ns_name::<GenericNamespaced>()
            .map(|name| name.into_owned())
    }

    /// Connect to the pipe `name` names.
    fn connect_name(name: &str) -> io::Result<PipeStream> {
        let stream = ConnectOptions::new()
            .name(socket_name(name)?)
            .connect_sync()?;
        PipeStream::new(stream)
    }

    /// Bind the pipe `name` names: nonblocking accept, same-user only —
    /// the named-pipe analog of the unix socket's 0600 in a 0700 dir
    /// (owner, SYSTEM and administrators get full access; nobody else).
    fn bind_pipe(name: &str) -> Result<LocalSocketListener, CliError> {
        let sddl = widestring::U16CString::from_str("D:P(A;;GA;;;SY)(A;;GA;;;BA)(A;;GA;;;OW)")
            .map_err(|err| CliError::Config(format!("session pipe security: {err}")))?;
        let sd = SecurityDescriptor::deserialize(sddl.as_ucstr())
            .map_err(|err| CliError::Config(format!("session pipe security: {err}")))?;
        ListenerOptions::new()
            .name(socket_name(name).map_err(|err| session::io_err(Path::new(name), err))?)
            .nonblocking(ListenerNonblockingMode::Accept)
            .security_descriptor(sd)
            .create_sync()
            .map_err(|err| session::io_err(Path::new(name), err))
    }

    pub(super) fn serve_session(
        session: &SessionName,
        browser: Browser,
        idle: Duration,
        record: Option<PathBuf>,
    ) -> Result<(), CliError> {
        session::serve_session::<Transport>(session, browser, idle, record)
    }

    pub(super) fn run_verb(verb: &Verb) -> Result<String, CliError> {
        session::run_verb::<Transport>(verb)
    }

    pub(super) fn list_sessions(dir: &Path) -> Result<String, CliError> {
        session::list_sessions::<Transport>(dir)
    }

    /// A bound Windows endpoint, for the session tests.
    #[cfg(test)]
    pub type Daemon = session::Daemon<Transport>;

    #[cfg(test)]
    pub(super) fn send(path: &Path, line: &str) -> Result<String, CliError> {
        session::send::<Transport>(path, line)
    }

    #[cfg(test)]
    pub(super) fn run_verb_at(
        verb: &Verb,
        path: &Path,
        spawn: impl FnOnce() -> Result<(), CliError>,
    ) -> Result<String, CliError> {
        session::run_verb_at::<Transport>(verb, path, spawn)
    }

    pub use session::socket_dir;
    #[cfg(test)]
    pub use session::{reply_ok, serve_failure, serve_on, socket_path, state_value_off};
}

#[cfg(not(any(unix, windows)))]
mod imp {
    use super::{Browser, SessionName, Verb};
    use crate::CliError;

    const UNSUPPORTED: &str =
        "aui shell verbs and the session daemon need Unix sockets or Windows named pipes";

    pub(super) fn run_verb(_verb: &Verb) -> Result<String, CliError> {
        Err(CliError::Config(UNSUPPORTED.into()))
    }

    pub(super) fn serve_session(
        _session: &SessionName,
        _browser: Browser,
        _idle: std::time::Duration,
        _record: Option<std::path::PathBuf>,
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

    /// `--launch` / `--cdp` / `--profile` are refused on the hidden daemon
    /// as on `open`, before it binds anything.
    #[test]
    fn the_daemon_takes_exactly_one_browser() {
        let everyday = ultra_instinct_browser::launch::default_user_data_dirs();
        let everyday = everyday.first().map(|dir| dir.display().to_string());
        let mut cases = vec![
            (vec!["--session", "t"], "no browser named"),
            (vec!["--launch", "--cdp", "http://x"], "pick one"),
            (vec!["--profile", "/p"], "--profile needs --launch"),
            (
                vec!["--cdp", "http://x", "--profile", "/p"],
                "needs --launch",
            ),
            (vec!["--launch", "--profile", " "], "needs a folder"),
            (vec!["--launch", "--profile="], "needs a folder"),
            (
                vec!["--cdp", "http://x", "--headed"],
                "--headed needs --launch",
            ),
        ];
        if let Some(dir) = &everyday {
            cases.push((vec!["--launch", "--profile", dir], "separate folder"));
        }
        for (args, says) in cases {
            let err = daemon_command(&a(&args)).unwrap_err().to_string();
            assert!(err.contains(says), "{args:?}: {err}");
        }
        let both = Browser::named(Some("http://x"), Some(Launch::default()));
        assert!(both.unwrap_err().to_string().contains("pick one"));
        // `--attach` multiplexes the user's own Chrome over its
        // remote-debugging socket, which is unix-only for now.
        #[cfg(unix)]
        {
            for other in ["--launch", "--cdp=http://x", "--profile=/p", "--headed"] {
                let err = daemon_command(&a(&["--attach", other]))
                    .unwrap_err()
                    .to_string();
                let flag = other.split('=').next().unwrap();
                assert!(
                    err.ends_with(&format!("--attach and {flag}: pick one")),
                    "{err}"
                );
            }
            let err = check_attach(true, [false; 4], false).unwrap_err();
            assert!(
                err.to_string()
                    .contains("name the sites with --allow-domain"),
                "{err}"
            );
            let open = |args: &[&str]| parse_verb("open", &a(args)).unwrap();
            let attach = open(&[
                "https://m.example/",
                "--attach",
                "--allow-domain",
                "m.example",
            ]);
            assert_eq!(attach.browser().unwrap(), Browser::Attach);
            assert!(attach.spawn_settings());
        }
    }

    #[cfg(any(unix, windows))]
    mod socket {
        use super::super::imp::{list_sessions, run_verb_at as run_verb_with, serve_failure};
        use super::super::*;
        use super::a;
        #[cfg(unix)]
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

        /// A short endpoint path in a per-process temp dir (unix `sun_path`
        /// is 104 bytes; the pipe name is a hash of this path).
        fn path(tag: &str) -> std::path::PathBuf {
            std::env::temp_dir()
                .join(format!("aui-t-{}-{tag}", std::process::id()))
                .join("s.sock")
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
                // The same options as `daemon_server`.
                let mut server = ultra_instinct_mcp::Server::with_connector(move |_url| {
                    let json = once.take().expect("one connect");
                    Ok(Box::new(ReplayTransport::parse(&json)?) as Box<dyn CdpTransport>)
                })
                .require_snapshot(true)
                .bind_to_latest_look(true);
                serve_on(
                    daemon,
                    &mut server,
                    "http://127.0.0.1:9222",
                    Duration::from_secs(20),
                    &mut None,
                    &mut None,
                )
                .unwrap();
            })
        }

        /// Run a daemon on `path` in a thread over a replay of `script`,
        /// recording into `file` the way `serve_session` does.
        fn start_rec(
            path: &std::path::Path,
            script: ScriptBuilder,
            file: &std::path::Path,
        ) -> std::thread::JoinHandle<()> {
            let path = path.to_owned();
            let file = file.to_owned();
            let json = script.to_json();
            let daemon = Daemon::bind(&path).unwrap();
            crate::record::Recorder::prepare(&file).unwrap();
            std::thread::spawn(move || {
                let mut once = Some(json);
                let mut server = ultra_instinct_mcp::Server::with_connector(move |_url| {
                    let json = once.take().expect("one connect");
                    Ok(Box::new(ReplayTransport::parse(&json)?) as Box<dyn CdpTransport>)
                })
                .require_snapshot(true)
                .bind_to_latest_look(true);
                let mut recorder = crate::record::Recorder::open(&file).ok();
                serve_on(
                    daemon,
                    &mut server,
                    "http://127.0.0.1:9222",
                    Duration::from_secs(20),
                    &mut None,
                    &mut recorder,
                )
                .unwrap();
                if let Some(recorder) = recorder {
                    recorder.finish();
                }
            })
        }

        /// The auto-state opt-out values and the reply check are strict.
        #[test]
        fn the_state_opt_out_and_reply_checks_are_strict() {
            use super::super::imp::{reply_ok, state_value_off};
            for off in ["0", "off", "false", "no", " OFF ", "No"] {
                assert!(state_value_off(off), "{off}");
            }
            for on in ["", "1", "yes", "on", "disabled"] {
                assert!(!state_value_off(on), "{on}");
            }
            assert!(reply_ok(
                r#"{"jsonrpc":"2.0","id":1,"result":{"content":[]}}"#
            ));
            assert!(!reply_ok(
                r#"{"jsonrpc":"2.0","id":1,"result":{"isError":true}}"#
            ));
            assert!(!reply_ok(r#"{"jsonrpc":"2.0","id":1,"error":{"code":-1}}"#));
            assert!(!reply_ok("not json"));
        }

        #[test]
        fn verbs_round_trip_through_the_socket_and_close_stops_it() {
            let path = path("rt");
            let script = ScriptBuilder::new()
                .observe_compact(&page())
                .observe_compact(&page())
                .observe_compact(&page())
                .pointer_click()
                .ready_state("complete")
                .observe_compact(&page());
            let handle = start(&path, script);
            #[cfg(unix)]
            {
                let mode = |p: &std::path::Path| {
                    std::fs::metadata(p).unwrap().permissions().mode() & 0o777
                };
                assert_eq!(mode(&path), 0o600);
                assert_eq!(mode(path.parent().unwrap()), 0o700);
            }
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

        /// A ref names what the last snap showed; a moved target refuses unpressed.
        #[test]
        fn a_press_after_the_page_changed_since_snap_refuses_unpressed() {
            let path = path("st");
            let moved = PageSpec::of(
                &[
                    Control::text_field(10, 100, "Email", (20.0, 20.0, 300.0, 28.0)),
                    Control::button(11, 101, "Sign in", (20.0, 300.0, 120.0, 28.0)),
                ],
                "http://127.0.0.1/form",
                "Form",
            );
            // No press scripted: a press would fail the replay.
            let script = ScriptBuilder::new()
                .observe_compact(&page())
                .observe_compact(&moved)
                .observe_compact(&moved);
            let handle = start(&path, script);
            let no_spawn = || panic!("the daemon is running");
            run_verb_with(&parse_verb("snap", &[]).unwrap(), &path, no_spawn).unwrap();
            let click = parse_verb("click", &a(&["@e2"])).unwrap();
            let err = run_verb_with(&click, &path, no_spawn)
                .unwrap_err()
                .to_string();
            assert!(
                err.starts_with("✗ refused: ") && err.ends_with("· run aui snap"),
                "{err}"
            );
            run_verb_with(&parse_verb("close", &[]).unwrap(), &path, no_spawn).unwrap();
            handle.join().unwrap();
            let _ = std::fs::remove_dir(path.parent().unwrap());
        }

        /// A ref press before any snap refuses before any CDP call.
        #[test]
        fn a_press_before_any_snap_refuses() {
            let path = path("ns");
            let handle = start(&path, ScriptBuilder::new());
            let no_spawn = || panic!("the daemon is running");
            let click = parse_verb("click", &a(&["@e2"])).unwrap();
            let err = run_verb_with(&click, &path, no_spawn)
                .unwrap_err()
                .to_string();
            assert_eq!(
                err,
                "✗ refused: no aui snap on this session yet · run aui snap"
            );
            run_verb_with(&parse_verb("close", &[]).unwrap(), &path, no_spawn).unwrap();
            handle.join().unwrap();
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
            // A stale endpoint: a leftover file where the socket or marker
            // would be, with nothing listening on the other side.
            std::fs::write(&path, b"stale").unwrap();
            assert!(path.exists());
            let mut handle = None;
            let snap = parse_verb("snap", &a(&["--cdp", "http://127.0.0.1:9222"])).unwrap();
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
            let snap = parse_verb("snap", &a(&["--cdp", "http://127.0.0.1:9222"])).unwrap();
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
            serve_on(
                daemon,
                &mut server,
                "http://x",
                Duration::from_millis(50),
                &mut None,
                &mut None,
            )
            .unwrap();
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
            // The marker can exist a moment before its pipe takes connects,
            // so retry briefly rather than trusting the file alone.
            let line = request_line(&Request::Tool {
                name: "browser_dialog",
                arguments: json!({"action": "status"}),
            });
            let mut reply = None;
            for _ in 0..100 {
                if let Ok(out) = super::super::imp::send(&socket, &line) {
                    reply = Some(out);
                    break;
                }
                assert!(started.elapsed() < Duration::from_secs(5), "never answered");
                std::thread::sleep(Duration::from_millis(20));
            }
            let reply = reply.expect("the daemon never answered");
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

        /// A directory someone else made first (`aui doctor` makes it
        /// private) is reused, and made private if it was not.
        #[test]
        fn a_socket_dir_made_before_the_daemon_is_reused() {
            let path = path("pre");
            let dir = path.parent().unwrap();
            std::fs::create_dir_all(dir).unwrap();
            #[cfg(unix)]
            {
                std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o755)).unwrap();
            }
            let daemon = Daemon::bind(&path).unwrap();
            #[cfg(unix)]
            {
                let mode = std::fs::metadata(dir).unwrap().permissions().mode() & 0o777;
                assert_eq!(mode, 0o700);
            }
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

        /// A verb that would start a session with no browser named refuses
        /// before any spawn: no default port is tried.
        #[test]
        fn a_new_session_with_no_browser_named_refuses_unspawned() {
            let path = path("nb");
            for (verb, args) in [("open", &["https://a/"][..]), ("snap", &[][..])] {
                let parsed = parse_verb(verb, &a(args)).unwrap();
                let err = run_verb_with(&parsed, &path, || panic!("no spawn without a browser"))
                    .unwrap_err();
                assert!(
                    err.to_string().contains("no browser named")
                        && err.to_string().contains("--launch"),
                    "{verb}: {err}"
                );
            }
            assert!(!path.exists());
        }

        #[test]
        fn session_list_names_live_sockets_and_removes_stale_ones() {
            let live = path("ls");
            let dir = live.parent().unwrap().to_owned();
            let handle = start(&live, ScriptBuilder::new());
            std::fs::write(dir.join("old.sock"), b"stale").unwrap();
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

        /// `--record`: each answered call leaves a frame (the screenshot
        /// tool's own secret hiding applies), throttled; `finish` writes
        /// the concat list and manifest, and `ffmpeg` the file when on PATH.
        #[test]
        fn a_recording_keeps_a_frame_per_answered_call() {
            let path = path("rec");
            let file = path.parent().unwrap().join("demo.mp4");
            // A real 1×1 PNG, so `ffmpeg` can encode it when it is on PATH.
            let png = "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mP8z8BQDwAEhQGAhKmMIQAAAABJRU5ErkJggg==";
            // snap → observe + frame; a snap inside the 400 ms gap → observe
            // only; a snap after it → observe + frame.
            let script = ScriptBuilder::new()
                .observe_compact(&page())
                .call("Page.captureScreenshot", json!({"data": png}))
                .observe_compact(&page())
                .observe_compact(&page())
                .call("Page.captureScreenshot", json!({"data": png}));
            let handle = start_rec(&path, script, &file);
            let no_spawn = || panic!("the daemon is running");
            let snap = || parse_verb("snap", &[]).unwrap();
            run_verb_with(&snap(), &path, no_spawn).unwrap();
            run_verb_with(&snap(), &path, no_spawn).unwrap();
            std::thread::sleep(Duration::from_millis(500));
            run_verb_with(&snap(), &path, no_spawn).unwrap();
            run_verb_with(&parse_verb("close", &[]).unwrap(), &path, no_spawn).unwrap();
            handle.join().unwrap();

            let frames = file.with_file_name(format!(
                "{}.frames",
                file.file_name().unwrap().to_string_lossy()
            ));
            for name in ["f_000001.png", "f_000002.png"] {
                let bytes = std::fs::read(frames.join(name)).unwrap();
                assert!(bytes.starts_with(b"\x89PNG"), "{name}");
            }
            assert!(!frames.join("f_000003.png").exists(), "throttled");
            let list = std::fs::read_to_string(frames.join("index.ffconcat")).unwrap();
            assert!(list.contains("f_000001.png") && list.contains("f_000002.png"));
            let manifest: Value = serde_json::from_str(
                &std::fs::read_to_string(frames.join("manifest.json")).unwrap(),
            )
            .unwrap();
            assert_eq!(manifest["frames"], 2);
            assert_eq!(manifest["frames_dir"], frames.display().to_string());
            assert!(
                matches!(
                    manifest["video"].as_str(),
                    Some("ok" | "ffmpeg-failed" | "no-ffmpeg")
                ),
                "{}",
                manifest["video"]
            );
            if manifest["video"] == "ok" {
                assert!(file.exists(), "ffmpeg wrote {}", file.display());
            }
            #[cfg(unix)]
            {
                let mode = std::fs::metadata(&frames).unwrap().permissions().mode() & 0o777;
                assert_eq!(mode, 0o700);
            }
            let _ = std::fs::remove_dir_all(path.parent().unwrap());
        }
    }
}
