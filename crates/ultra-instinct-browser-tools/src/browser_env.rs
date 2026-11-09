//! Browser settings, downloads, and network tools (`browser_set`,
//! `browser_download`, `browser_network`), ported from agent-browser's
//! `set` / `download` / `network` verbs.
//!
//! Settings are plain CDP on the session's tab (`Emulation.*`, `Network.*`).
//! Chrome scopes these overrides to the DevTools session, so they last as
//! long as the server keeps that session (an evicted or dropped session
//! clears them on Chrome's side). Observe reads `Page.getLayoutMetrics` on
//! every call, so the next observe after a viewport / device / reset uses
//! the new viewport; nothing here caches it.
//!
//! `browser_download` adds no press path: it sets `Browser.setDownloadBehavior`,
//! then presses through `browser_click` (gate → ticket → `execute_ticketed`,
//! so a stale page refuses), then waits for `Browser.downloadProgress`
//! `completed` among the buffered transport events.
//!
//! HAR recording is a tap on the server's one event buffer
//! ([`EnvState::tee`], called from `BrowserHost::push_events`): the tap copies
//! Network events as they are buffered, and `browser_read_network` drains
//! the buffer itself, so neither steals events from the other.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::time::Duration;

use serde_json::{json, Map, Value};
use ultra_instinct_cdp::{CdpEvent, DEFAULT_CDP_HTTP};

use crate::args::{opt_bool, req_str};
use crate::error::ToolError;
use crate::host::{endpoint_of, keep, session_key, BrowserHost, Downloads, LiveSession};

/// Tool names this module serves.
pub const ENV_TOOLS: [&str; 3] = ["browser_set", "browser_download", "browser_network"];

/// Network-domain bit in `BrowserHost::enabled_domains`. Must equal
/// `browser_tools::NETWORK_BIT`, so `Network.enable` runs once per session
/// whichever of `browser_read_network` / these tools comes first.
const NETWORK_BIT: u8 = 0b10;

/// Interval between transport pumps while a download is in flight.
const DOWNLOAD_POLL_MS: u64 = 100;
const DEFAULT_DOWNLOAD_TIMEOUT_MS: u64 = 30_000;
const MAX_DOWNLOAD_TIMEOUT_MS: u64 = 300_000;

/// Network events one HAR recording keeps; later ones are counted as dropped.
const MAX_HAR_EVENTS: usize = 20_000;

/// Network events a HAR is built from. The `*ExtraInfo` events carry the
/// headers Chrome adds on the wire (`Cookie`, `Set-Cookie`), which the main
/// events leave out; they are merged in and masked like the rest.
const HAR_METHODS: [&str; 6] = [
    "Network.requestWillBeSent",
    "Network.requestWillBeSentExtraInfo",
    "Network.responseReceived",
    "Network.responseReceivedExtraInfo",
    "Network.loadingFinished",
    "Network.loadingFailed",
];

/// Built-in device table for `browser_set {what: "device"}`.
struct Device {
    name: &'static str,
    width: u32,
    height: u32,
    scale: f64,
    mobile: bool,
    touch: bool,
    /// Empty: no user-agent override (the browser's own).
    user_agent: &'static str,
}

const DEVICES: [Device; 4] = [
    Device {
        name: "iPhone 15",
        width: 393,
        height: 852,
        scale: 3.0,
        mobile: true,
        touch: true,
        user_agent: "Mozilla/5.0 (iPhone; CPU iPhone OS 17_0 like Mac OS X) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/17.0 Mobile/15E148 Safari/604.1",
    },
    Device {
        name: "Pixel 8",
        width: 412,
        height: 915,
        scale: 2.625,
        mobile: true,
        touch: true,
        user_agent: "Mozilla/5.0 (Linux; Android 14; Pixel 8) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/120.0.0.0 Mobile Safari/537.36",
    },
    Device {
        name: "iPad Air",
        width: 820,
        height: 1180,
        scale: 2.0,
        mobile: true,
        touch: true,
        user_agent: "Mozilla/5.0 (iPad; CPU OS 17_0 like Mac OS X) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/17.0 Mobile/15E148 Safari/604.1",
    },
    Device {
        name: "Desktop 1440",
        width: 1440,
        height: 900,
        scale: 1.0,
        mobile: false,
        touch: false,
        user_agent: "",
    },
];

fn device(name: &str) -> Option<&'static Device> {
    let wanted = normalize_name(name);
    DEVICES.iter().find(|d| normalize_name(d.name) == wanted)
}

fn normalize_name(name: &str) -> String {
    name.split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase()
}

/// One HAR recording: Network events copied from the shared buffer.
#[derive(Default)]
struct HarTap {
    events: Vec<CdpEvent>,
    /// Hashes of events already copied. Readers that drain the buffer put
    /// the events they do not consume back through `BrowserHost::push_events`
    /// (console / network / download readers); a re-pushed event is the
    /// same event and is copied once.
    seen: BTreeSet<u64>,
    dropped: u64,
}

fn event_hash(event: &CdpEvent) -> u64 {
    use std::hash::{Hash as _, Hasher as _};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    event.method.hash(&mut hasher);
    event.params.to_string().hash(&mut hasher);
    hasher.finish()
}

/// Per-session-key state for these tools. Lives on the host and is dropped
/// with the session key, like ledgers and buffered events.
#[derive(Default)]
pub(crate) struct EnvState {
    har: BTreeMap<String, HarTap>,
    blocked: BTreeMap<String, Vec<String>>,
}

impl EnvState {
    /// Copy Network events into `key`'s HAR recording, if one runs. Called
    /// for every batch the server buffers, before the buffer cap applies.
    pub(crate) fn tee(&mut self, key: &str, events: &[CdpEvent]) {
        let Some(tap) = self.har.get_mut(key) else {
            return;
        };
        for event in events {
            if !HAR_METHODS.contains(&event.method.as_str()) {
                continue;
            }
            if !tap.seen.insert(event_hash(event)) {
                continue;
            }
            if tap.events.len() >= MAX_HAR_EVENTS {
                tap.dropped += 1;
            } else {
                tap.events.push(event.clone());
            }
        }
    }

    /// Forget everything kept for `key`.
    pub(crate) fn remove(&mut self, key: &str) {
        self.har.remove(key);
        self.blocked.remove(key);
    }
}

/// Run one of [`ENV_TOOLS`].
pub(crate) fn call(
    server: &mut dyn BrowserHost,
    name: &str,
    arguments: &Value,
) -> Result<Value, ToolError> {
    match name {
        "browser_set" => set(server, arguments),
        "browser_download" => download(server, arguments),
        "browser_network" => network(server, arguments),
        other => Err(ToolError::UnknownTool(other.to_owned())),
    }
}

// ---- argument helpers -------------------------------------------------------

fn bad(message: impl Into<String>) -> ToolError {
    ToolError::InvalidArguments(message.into())
}

fn opt_u64(arguments: &Value, key: &str) -> Result<Option<u64>, ToolError> {
    match arguments.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::Number(n)) => n
            .as_u64()
            .map(Some)
            .ok_or_else(|| bad(format!("`{key}` must be a non-negative integer"))),
        Some(other) => Err(bad(format!(
            "`{key}` must be a non-negative integer, got {other}"
        ))),
    }
}

fn opt_f64(arguments: &Value, key: &str) -> Result<Option<f64>, ToolError> {
    match arguments.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::Number(n)) => Ok(n.as_f64()),
        Some(other) => Err(bad(format!("`{key}` must be a number, got {other}"))),
    }
}

fn str_list(arguments: &Value, key: &str) -> Result<Option<Vec<String>>, ToolError> {
    match arguments.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::Array(items)) => items
            .iter()
            .map(|item| match item {
                Value::String(s) if !s.is_empty() => Ok(s.clone()),
                other => Err(bad(format!(
                    "`{key}` entries must be non-empty strings, got {other}"
                ))),
            })
            .collect::<Result<Vec<_>, _>>()
            .map(Some),
        Some(other) => Err(bad(format!("`{key}` must be an array, got {other}"))),
    }
}

// ---- session plumbing (same take / keep contract as browser_tools) ---------

/// One CDP command with JSON params (payloads are CDP arguments, never
/// spliced into script source).
fn send(session: &mut LiveSession, method: &str, params: Value) -> Result<String, ToolError> {
    session
        .transport_mut()
        .call(method, &params.to_string())
        .map_err(|err| ToolError::Browser(format!("{method}: {err}")))
}

/// `Network.enable` once per session key (lazily, shared with
/// `browser_read_network`).
fn ensure_network(
    server: &mut dyn BrowserHost,
    key: &str,
    session: &mut LiveSession,
) -> Result<(), ToolError> {
    if server.enabled_domains(key) & NETWORK_BIT != 0 {
        return Ok(());
    }
    send(session, "Network.enable", json!({}))?;
    server.enable_domain(key, NETWORK_BIT);
    Ok(())
}

/// Take the session for `arguments`' endpoint, run `body`, keep it.
fn with_session<R>(
    server: &mut dyn BrowserHost,
    arguments: &Value,
    body: impl FnOnce(&mut dyn BrowserHost, &str, &mut LiveSession) -> Result<R, ToolError>,
) -> Result<R, ToolError> {
    let endpoint = endpoint_of(arguments)?;
    let key = session_key(server, &endpoint);
    let mut session = server.take_session(&key)?;
    let outcome = body(server, &key, &mut session);
    keep(server, &key, session);
    outcome
}

// ---- browser_set ------------------------------------------------------------

fn metrics_params(width: u64, height: u64, scale: f64, mobile: bool) -> Value {
    json!({
        "width": width,
        "height": height,
        "deviceScaleFactor": scale,
        "mobile": mobile,
    })
}

fn dimension(arguments: &Value, key: &str) -> Result<u64, ToolError> {
    let value = opt_u64(arguments, key)?.ok_or_else(|| bad(format!("missing required `{key}`")))?;
    if !(1..=10_000).contains(&value) {
        return Err(bad(format!("`{key}` must be in 1..=10000, got {value}")));
    }
    Ok(value)
}

fn set(server: &mut dyn BrowserHost, arguments: &Value) -> Result<Value, ToolError> {
    let what = req_str(arguments, "what")?;
    match what {
        "viewport" => {
            let width = dimension(arguments, "width")?;
            let height = dimension(arguments, "height")?;
            let scale = opt_f64(arguments, "scale")?.unwrap_or(1.0);
            if !(scale > 0.0 && scale <= 10.0) {
                return Err(bad(format!("`scale` must be in (0, 10], got {scale}")));
            }
            with_session(server, arguments, |_, _, session| {
                send(
                    session,
                    "Emulation.setDeviceMetricsOverride",
                    metrics_params(width, height, scale, false),
                )?;
                Ok(json!({"set": "viewport", "width": width, "height": height, "scale": scale}))
            })
        }
        "device" => {
            let name = req_str(arguments, "name")?;
            let device = device(name).ok_or_else(|| {
                let known: Vec<&str> = DEVICES.iter().map(|d| d.name).collect();
                bad(format!(
                    "unknown device `{name}`; known: {}",
                    known.join(", ")
                ))
            })?;
            with_session(server, arguments, |_, _, session| {
                send(
                    session,
                    "Emulation.setDeviceMetricsOverride",
                    metrics_params(
                        device.width.into(),
                        device.height.into(),
                        device.scale,
                        device.mobile,
                    ),
                )?;
                let touch = if device.touch {
                    json!({"enabled": true, "maxTouchPoints": 5})
                } else {
                    json!({"enabled": false})
                };
                send(session, "Emulation.setTouchEmulationEnabled", touch)?;
                send(
                    session,
                    "Emulation.setUserAgentOverride",
                    json!({"userAgent": device.user_agent}),
                )?;
                Ok(json!({
                    "set": "device",
                    "name": device.name,
                    "width": device.width,
                    "height": device.height,
                    "scale": device.scale,
                    "mobile": device.mobile,
                    "touch": device.touch,
                }))
            })
        }
        "media" => {
            let scheme = req_str(arguments, "color_scheme")?;
            if !matches!(scheme, "dark" | "light" | "no-preference") {
                return Err(bad(format!(
                    "`color_scheme` must be dark|light|no-preference, got `{scheme}`"
                )));
            }
            with_session(server, arguments, |_, _, session| {
                send(session, "Emulation.setEmulatedMedia", media_params(scheme))?;
                Ok(json!({"set": "media", "color_scheme": scheme}))
            })
        }
        "offline" => {
            let on = opt_bool(arguments, "on")?.ok_or_else(|| bad("missing required `on`"))?;
            with_session(server, arguments, |server, key, session| {
                ensure_network(server, key, session)?;
                send(
                    session,
                    "Network.emulateNetworkConditions",
                    network_conditions(on),
                )?;
                Ok(json!({"set": "offline", "on": on}))
            })
        }
        "headers" => {
            let map = match arguments.get("map") {
                Some(Value::Object(map)) => map.clone(),
                Some(other) => return Err(bad(format!("`map` must be an object, got {other}"))),
                None => return Err(bad("missing required `map`")),
            };
            if let Some((name, value)) = map.iter().find(|(_, v)| !v.is_string()) {
                return Err(bad(format!(
                    "header `{name}` must have a string value, got {value}"
                )));
            }
            let names: Vec<String> = map.keys().cloned().collect();
            with_session(server, arguments, |server, key, session| {
                ensure_network(server, key, session)?;
                send(
                    session,
                    "Network.setExtraHTTPHeaders",
                    json!({"headers": Value::Object(map)}),
                )?;
                // Values are not echoed: they may be credentials.
                Ok(json!({"set": "headers", "names": names}))
            })
        }
        "reset" => with_session(server, arguments, |server, key, session| {
            let mut cleared = Vec::new();
            for (method, params) in [
                ("Emulation.clearDeviceMetricsOverride", json!({})),
                (
                    "Emulation.setTouchEmulationEnabled",
                    json!({"enabled": false}),
                ),
                ("Emulation.setUserAgentOverride", json!({"userAgent": ""})),
                ("Emulation.setEmulatedMedia", media_params("")),
            ] {
                send(session, method, params)?;
                cleared.push(method);
            }
            // Offline and extra headers can only have been set after this
            // session enabled Network; without it there is nothing to clear
            // and no reason to start the Network event stream.
            if server.enabled_domains(key) & NETWORK_BIT != 0 {
                send(
                    session,
                    "Network.emulateNetworkConditions",
                    network_conditions(false),
                )?;
                cleared.push("Network.emulateNetworkConditions");
                send(
                    session,
                    "Network.setExtraHTTPHeaders",
                    json!({"headers": {}}),
                )?;
                cleared.push("Network.setExtraHTTPHeaders");
            }
            Ok(json!({"set": "reset", "cleared": cleared}))
        }),
        other => Err(bad(format!(
            "`what` must be viewport|device|media|offline|headers|reset, got `{other}`"
        ))),
    }
}

/// `Emulation.setEmulatedMedia` for one color scheme (`""` clears it).
fn media_params(scheme: &str) -> Value {
    json!({
        "media": "",
        "features": [{"name": "prefers-color-scheme", "value": scheme}],
    })
}

fn network_conditions(offline: bool) -> Value {
    json!({
        "offline": offline,
        "latency": 0,
        "downloadThroughput": -1,
        "uploadThroughput": -1,
    })
}

// ---- browser_download -------------------------------------------------------

/// A destination the download may be moved to: never an existing
/// directory, and an existing file only with `overwrite`.
fn check_destination(path: &Path, overwrite: bool) -> Result<(), ToolError> {
    if path.is_dir() {
        return Err(bad(format!(
            "`path` {} is an existing directory; give a file path",
            path.display()
        )));
    }
    if path.exists() && !overwrite {
        return Err(bad(format!(
            "`path` {} exists; pass overwrite: true to replace it",
            path.display()
        )));
    }
    Ok(())
}

/// A fresh, empty temp directory Chrome writes the download into. Made
/// with `create_dir`, never reused: the clock can repeat (macOS gives
/// microseconds), so a name taken by another download gets the next suffix.
fn download_dir(base: &Path) -> Result<PathBuf, ToolError> {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or_default();
    let mut suffix = 0u32;
    loop {
        let dir = base.join(format!(
            "ultra-instinct-download-{}-{nanos}-{suffix}",
            std::process::id()
        ));
        match std::fs::create_dir(&dir) {
            Ok(()) => return Ok(dir),
            Err(err) if err.kind() == std::io::ErrorKind::AlreadyExists && suffix < 1000 => {
                suffix += 1;
            }
            Err(err) => {
                return Err(ToolError::Io {
                    path: dir.display().to_string(),
                    message: err.to_string(),
                })
            }
        }
    }
}

/// What the buffered download events say so far.
#[derive(Default)]
struct DownloadSeen {
    guid: Option<String>,
    suggested_name: Option<String>,
    /// Completed: (guid, Chrome-reported file path when it sends one).
    completed: Option<(String, Option<String>)>,
    canceled: bool,
}

fn is_download_event(method: &str) -> bool {
    matches!(
        method,
        "Browser.downloadWillBegin"
            | "Browser.downloadProgress"
            | "Page.downloadWillBegin"
            | "Page.downloadProgress"
    )
}

/// Move download events out of `key`'s buffer into `seen`; every other
/// event goes back untouched (console / network readers still get them).
fn scan_downloads(server: &mut dyn BrowserHost, key: &str, seen: &mut DownloadSeen) {
    let mut rest = Vec::new();
    for event in server.take_events(key) {
        if !is_download_event(&event.method) {
            rest.push(event);
            continue;
        }
        let params = &event.params;
        let guid = params
            .get("guid")
            .and_then(Value::as_str)
            .map(str::to_owned);
        if event.method.ends_with("downloadWillBegin") {
            if seen.guid.is_none() {
                seen.guid = guid;
                seen.suggested_name = params
                    .get("suggestedFilename")
                    .and_then(Value::as_str)
                    .map(str::to_owned);
            }
            continue;
        }
        let ours = match (&seen.guid, &guid) {
            (Some(want), Some(got)) => want == got,
            (None, Some(_)) => true,
            _ => false,
        };
        if !ours {
            continue;
        }
        match params.get("state").and_then(Value::as_str) {
            Some("completed") if seen.completed.is_none() => {
                let file = params
                    .get("filePath")
                    .and_then(Value::as_str)
                    .map(str::to_owned);
                seen.completed = guid.map(|g| (g, file));
            }
            Some("canceled") => seen.canceled = true,
            _ => {}
        }
    }
    if !rest.is_empty() {
        server.push_events(key, rest);
    }
}

/// Chrome names an `allowAndName` download by its guid; accept only a
/// plain token so it cannot point outside the download directory.
fn safe_guid(guid: &str) -> bool {
    !guid.is_empty()
        && guid
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

fn move_file(from: &Path, to: &Path) -> std::io::Result<()> {
    if std::fs::rename(from, to).is_ok() {
        return Ok(());
    }
    // Temp dir on another filesystem: copy, then remove the source.
    std::fs::copy(from, to)?;
    std::fs::remove_file(from)
}

fn download(server: &mut dyn BrowserHost, arguments: &Value) -> Result<Value, ToolError> {
    let endpoint = endpoint_of(arguments)?;
    // 5, "5" or "@e5", like every other index tool.
    let index = crate::browser_extra::index_arg(arguments)? as u64;
    let path = crate::host::absolute_path(req_str(arguments, "path")?)?.to_path_buf();
    // Where the host lets downloads go; Chrome writes into a fresh folder
    // inside it (or the temp dir when downloads may go anywhere).
    let staging = match server.downloads() {
        Downloads::Off => return Err(ToolError::DownloadsOff),
        Downloads::Anywhere => std::env::temp_dir(),
        Downloads::Dir(dir) => {
            // Canonical containment, as uploads: `..` and symlinks (the
            // destination itself, or a folder on the way) cannot leave it.
            let real = std::fs::canonicalize(&path).or_else(|_| {
                let parent = path.parent().ok_or(std::io::ErrorKind::NotFound)?;
                std::fs::canonicalize(parent).map(|p| p.join(path.file_name().unwrap_or_default()))
            });
            let inside = matches!((real, std::fs::canonicalize(&dir)),
                (Ok(real), Ok(dir)) if real.parent() == Some(dir.as_path()));
            if !inside {
                return Err(ToolError::PathNotAllowed {
                    path: path.display().to_string(),
                    dir: dir.display().to_string(),
                });
            }
            dir
        }
    };
    let overwrite = opt_bool(arguments, "overwrite")?.unwrap_or(false);
    let mouse = opt_bool(arguments, "mouse")?.unwrap_or(false);
    let timeout_ms = opt_u64(arguments, "timeout_ms")?
        .unwrap_or(DEFAULT_DOWNLOAD_TIMEOUT_MS)
        .clamp(1, MAX_DOWNLOAD_TIMEOUT_MS);
    check_destination(&path, overwrite)?;

    let dir = download_dir(&staging)?;
    // The press takes this call's `snapshot` and `confirm` like browser_click.
    let mut press = json!({"cdp": endpoint, "index": index, "mouse": mouse});
    for key in ["snapshot", "confirm"] {
        if let Some(value) = arguments.get(key).filter(|v| !v.is_null()) {
            press[key] = value.clone();
        }
    }
    let outcome = download_into(server, &endpoint, &press, timeout_ms, &dir);
    // Back to Chrome's default so later clicks do not land in a temp dir.
    // Best effort: the download result (or its own error) stands either way.
    let restore = crate::host::best_effort(server, |server| {
        with_session(server, &json!({"cdp": endpoint}), |_, _, session| {
            send(
                session,
                "Browser.setDownloadBehavior",
                json!({"behavior": "default"}),
            )
        })
    });
    let moved = outcome.and_then(|(source, suggested_name)| {
        check_destination(&path, overwrite)?;
        move_file(&source, &path).map_err(|err| ToolError::Io {
            path: path.display().to_string(),
            message: err.to_string(),
        })?;
        let bytes = std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
        Ok(json!({
            "path": path.display().to_string(),
            "bytes": bytes,
            "suggested_name": suggested_name,
        }))
    });
    let _ = std::fs::remove_dir_all(&dir);
    let mut body = moved?;
    if let Err(err) = restore {
        body["restore_error"] = json!(err.to_string());
    }
    Ok(body)
}

/// Allow downloads into `dir`, press through `browser_click`, wait for
/// `completed`. Returns the downloaded file and its suggested name.
fn download_into(
    server: &mut dyn BrowserHost,
    endpoint: &str,
    press: &Value,
    timeout_ms: u64,
    dir: &Path,
) -> Result<(PathBuf, Option<String>), ToolError> {
    let target = json!({"cdp": endpoint});
    let key = session_key(server, endpoint);
    with_session(server, &target, |_, _, session| {
        send(
            session,
            "Browser.setDownloadBehavior",
            json!({
                "behavior": "allowAndName",
                "downloadPath": dir.display().to_string(),
                "eventsEnabled": true,
            }),
        )
    })?;
    // Download events from before this press belong to someone else.
    scan_downloads(server, &key, &mut DownloadSeen::default());

    // The press: the exact `browser_click` path (gate → ticket →
    // `execute_ticketed`). A page that drifted refuses here.
    crate::browser_tools::call(server, "browser_click", press)?;

    let mut seen = DownloadSeen::default();
    scan_downloads(server, &key, &mut seen);
    let polls = timeout_ms.div_ceil(DOWNLOAD_POLL_MS).max(1);
    let mut poll = 0;
    while seen.completed.is_none() && !seen.canceled && poll < polls {
        poll += 1;
        std::thread::sleep(Duration::from_millis(DOWNLOAD_POLL_MS.min(timeout_ms)));
        // The blocking socket reads frames only during a call: a cheap
        // command pumps the events that arrived since the last one.
        with_session(server, &target, |_, _, session| {
            send(session, "Browser.getVersion", json!({})).map(|_| ())
        })?;
        scan_downloads(server, &key, &mut seen);
    }
    if seen.canceled {
        return Err(ToolError::Browser("download was canceled".into()));
    }
    let Some((guid, file_path)) = seen.completed else {
        return Err(ToolError::Browser(format!(
            "download did not complete within {timeout_ms} ms"
        )));
    };
    if !safe_guid(&guid) {
        return Err(ToolError::Browser(format!(
            "download guid `{guid}` is not a plain token"
        )));
    }
    let named = dir.join(&guid);
    let source = match file_path.map(PathBuf::from) {
        Some(reported) if !named.exists() && reported.starts_with(dir) => reported,
        _ => named,
    };
    if !source.is_file() {
        return Err(ToolError::Browser(format!(
            "download completed but {} is missing",
            source.display()
        )));
    }
    Ok((source, seen.suggested_name))
}

// ---- browser_network --------------------------------------------------------

/// Blocking uses `Network.setBlockedURLs`, never `Fetch.enable` +
/// `Fetch.requestPaused`: this server is a blocking request/response loop
/// over one CDP socket, idle between tool calls, so it cannot answer a paused
/// request until the next call arrives. Every matching request would hang
/// the page (and any wait on it) until then. `setBlockedURLs` makes Chrome
/// fail matching requests itself, with nothing for us to answer.
fn network(server: &mut dyn BrowserHost, arguments: &Value) -> Result<Value, ToolError> {
    let action = req_str(arguments, "action")?;
    match action {
        "block" => {
            let patterns = str_list(arguments, "patterns")?
                .filter(|p| !p.is_empty())
                .ok_or_else(|| bad("missing required `patterns`"))?;
            with_session(server, arguments, |server, key, session| {
                let mut list = server
                    .tool_state()
                    .env
                    .blocked
                    .get(key)
                    .cloned()
                    .unwrap_or_default();
                for pattern in patterns {
                    if !list.contains(&pattern) {
                        list.push(pattern);
                    }
                }
                apply_blocked(server, key, session, list)
            })
        }
        "unblock" => {
            let patterns = str_list(arguments, "patterns")?;
            with_session(server, arguments, |server, key, session| {
                let list = match patterns {
                    // No patterns: lift every block.
                    None => Vec::new(),
                    Some(drop) => server
                        .tool_state()
                        .env
                        .blocked
                        .get(key)
                        .cloned()
                        .unwrap_or_default()
                        .into_iter()
                        .filter(|p| !drop.contains(p))
                        .collect(),
                };
                apply_blocked(server, key, session, list)
            })
        }
        "har_start" => with_session(server, arguments, |server, key, session| {
            ensure_network(server, key, session)?;
            if server.tool_state().env.har.contains_key(key) {
                return Ok(json!({"recording": true, "already": true}));
            }
            // Events already buffered predate the recording. Mark them seen
            // so a reader that puts them back does not copy them in later.
            let earlier = session.transport_mut().drain_events();
            if !earlier.is_empty() {
                server.push_events(key, earlier);
            }
            let buffered = server.take_events(key);
            let tap = HarTap {
                seen: buffered.iter().map(event_hash).collect(),
                ..HarTap::default()
            };
            server.tool_state_mut().env.har.insert(key.to_owned(), tap);
            if !buffered.is_empty() {
                server.push_events(key, buffered);
            }
            Ok(json!({"recording": true}))
        }),
        "har_stop" => {
            let path = crate::host::absolute_path(req_str(arguments, "path")?)?.to_path_buf();
            if path.is_dir() {
                return Err(bad(format!(
                    "`path` {} is an existing directory; give a file path",
                    path.display()
                )));
            }
            let endpoint = endpoint_of(arguments)?;
            let key = session_key(server, &endpoint);
            if !server.tool_state().env.har.contains_key(&key) {
                return Err(bad("no HAR recording on this tab; call har_start first"));
            }
            // Flush the transport so the tap sees everything up to now.
            with_session(server, arguments, |_, _, _| Ok(()))?;
            let tap = server
                .tool_state_mut()
                .env
                .har
                .remove(&key)
                .unwrap_or_default();
            let mut har = ultra_instinct_cdp::har::build_har(&tap.events);
            // A typed secret a page sent in a URL never reaches the file.
            server.tool_state().secrets.scrub_value(&mut har);
            let entries = har["log"]["entries"].as_array().map_or(0, Vec::len);
            let text = serde_json::to_vec_pretty(&har)
                .map_err(|err| ToolError::Browser(err.to_string()))?;
            write_private(&path, &text).map_err(|err| ToolError::Io {
                path: path.display().to_string(),
                message: err.to_string(),
            })?;
            Ok(json!({
                "path": path.display().to_string(),
                "entries": entries,
                "bytes": text.len(),
                "dropped": tap.dropped,
            }))
        }
        other => Err(bad(format!(
            "`action` must be block|unblock|har_start|har_stop, got `{other}`"
        ))),
    }
}

fn apply_blocked(
    server: &mut dyn BrowserHost,
    key: &str,
    session: &mut LiveSession,
    list: Vec<String>,
) -> Result<Value, ToolError> {
    ensure_network(server, key, session)?;
    send(session, "Network.setBlockedURLs", json!({"urls": list}))?;
    let body = json!({"blocked": list});
    if list.is_empty() {
        server.tool_state_mut().env.blocked.remove(key);
    } else {
        server
            .tool_state_mut()
            .env
            .blocked
            .insert(key.to_owned(), list);
    }
    Ok(body)
}

/// Write `bytes` to `path` readable by the owner only (0600 on unix).
fn write_private(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    use std::io::Write as _;
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt as _;
        options.mode(0o600);
    }
    let mut file = options.open(path)?;
    #[cfg(unix)]
    {
        // `mode` applies only on create; an existing file is narrowed too.
        use std::os::unix::fs::PermissionsExt as _;
        file.set_permissions(std::fs::Permissions::from_mode(0o600))?;
    }
    file.write_all(bytes)
}

// ---- tool specs -------------------------------------------------------------

/// `(description, properties, required)` for one of [`ENV_TOOLS`].
pub(crate) fn spec(name: &str) -> (String, Map<String, Value>, Vec<&'static str>) {
    let mut props = Map::new();
    props.insert(
        "cdp".into(),
        json!({"type": "string", "description": format!("CDP debugging endpoint; default {DEFAULT_CDP_HTTP}")}),
    );
    let mut required = Vec::new();
    let description = match name {
        "browser_set" => {
            props.insert(
                "what".into(),
                json!({"type": "string", "enum": ["viewport", "device", "media", "offline", "headers", "reset"]}),
            );
            props.insert("width".into(), json!({"type": "integer"}));
            props.insert("height".into(), json!({"type": "integer"}));
            props.insert("scale".into(), json!({"type": "number"}));
            props.insert(
                "name".into(),
                json!({"type": "string", "enum": DEVICES.iter().map(|d| d.name).collect::<Vec<_>>()}),
            );
            props.insert(
                "color_scheme".into(),
                json!({"type": "string", "enum": ["dark", "light", "no-preference"]}),
            );
            props.insert("on".into(), json!({"type": "boolean"}));
            props.insert(
                "map".into(),
                json!({"type": "object", "additionalProperties": {"type": "string"}}),
            );
            required.push("what");
            "Browser settings for the current tab: viewport {width, height, scale?}, device {name} (viewport + mobile + touch + user agent), media {color_scheme}, offline {on}, headers {map} (extra HTTP headers), reset (clears all of these). The next observe uses the new viewport."
        }
        "browser_download" => {
            props.insert("index".into(), json!({"type": ["integer", "string"], "description": "1-based index from browser_get_state: 5, \"5\" or \"@e5\""}));
            props.insert("snapshot".into(), crate::bound::snapshot_prop());
            props.insert("confirm".into(), json!({"type": "string", "description": "The label of a press the user confirmed, this call only."}));
            props.insert(
                "path".into(),
                json!({"type": "string", "description": "Absolute file path."}),
            );
            props.insert("overwrite".into(), json!({"type": "boolean"}));
            props.insert("mouse".into(), json!({"type": "boolean"}));
            props.insert("timeout_ms".into(), json!({"type": "integer"}));
            required.extend(["index", "path"]);
            "Click the element at a browser_get_state index through the same ticketed path as browser_click, wait (timeout_ms, default 30000) for the download to complete, and move the file to path. Refuses a directory path, and an existing file unless overwrite. Returns path, bytes, suggested_name."
        }
        "browser_network" => {
            props.insert(
                "action".into(),
                json!({"type": "string", "enum": ["block", "unblock", "har_start", "har_stop"]}),
            );
            props.insert(
                "patterns".into(),
                json!({"type": "array", "items": {"type": "string"}}),
            );
            props.insert("path".into(), json!({"type": "string"}));
            required.push("action");
            "Network control: block {patterns} / unblock {patterns?} (Network.setBlockedURLs wildcards; unblock without patterns lifts all), har_start, har_stop {path} (HAR 1.2 without bodies, cookie/authorization values redacted, written 0600). browser_read_network keeps working while a HAR records."
        }
        other => return (other.to_owned(), props, required),
    };
    (description.to_owned(), props, required)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn devices_resolve_case_and_space_insensitively() {
        assert_eq!(device("iphone  15").map(|d| d.width), Some(393));
        assert_eq!(device("Desktop 1440").map(|d| d.mobile), Some(false));
        assert!(device("Nokia 3310").is_none());
    }

    #[test]
    fn guid_must_be_a_plain_token() {
        assert!(safe_guid("5f0c-AB_9"));
        assert!(!safe_guid("../etc/passwd"));
        assert!(!safe_guid(""));
    }
}
