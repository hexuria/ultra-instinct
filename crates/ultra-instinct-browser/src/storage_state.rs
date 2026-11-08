//! Save and reuse a login: cookies plus per-origin Web Storage.
//!
//! File format (version 1, untrusted on load):
//! `{"version":1,"cookies":[{name,value,domain,path,expires?,httpOnly,secure,sameSite?}],
//! "origins":[{"origin":"https://host[:port]","local":{k:v},"session":{k:v}}]}`.
//! Unknown fields, other versions, bad origins, and duplicate origins are errors.
//!
//! [`save_state`] reads every cookie (`Storage.getCookies`, else
//! `Network.getAllCookies`) and the current origin's storage through one fixed
//! `Runtime.evaluate` script. [`load_state`] sets cookies (`Network.setCookies`)
//! and writes the storage of the origin the page is on now through a fixed
//! function whose entries are CDP arguments, never script source. Storage of
//! any other origin is reported as pending: navigate there and call
//! [`apply_origin_storage`] (or [`load_state`] again — cookies are re-set
//! idempotently). Writes merge: keys the file does not name stay.
//!
//! Files are written 0600 through a temp file and rename; on unix, loading a
//! file group or others can access is refused.

use std::collections::BTreeMap;
use std::io::{Read, Write};
use std::path::Path;

use serde_json::{json, Map, Value};
use ultra_instinct_cdp::CdpTransport;

use crate::{BrowserError, BrowserSession};

pub const STATE_VERSION: u64 = 1;
/// Largest state file `read_state_file` accepts.
pub const MAX_STATE_BYTES: u64 = 16 * 1024 * 1024;

const READ_STORAGE_JS: &str = "(()=>{const d=n=>{try{const s=window[n],o={};for(let i=0;i<s.length;i++){const k=s.key(i);o[k]=s.getItem(k);}return o;}catch(e){return null;}};return{origin:location.origin,host:location.hostname,local:d('localStorage'),session:d('sessionStorage')};})()";

/// `this` is `globalThis`. `origin` null skips the origin check; `op` is
/// `set` (payload: object), `remove` (payload: key array), or `clear`.
pub const WRITE_STORAGE_FUNCTION: &str = "function(origin,area,op,payload){if(origin!==null&&location.origin!==origin)throw new Error('page left '+origin);var s=area==='session'?sessionStorage:localStorage;if(op==='clear'){s.clear();return true;}if(op==='remove'){for(var i=0;i<payload.length;i++)s.removeItem(String(payload[i]));return true;}if(op!=='set')throw new Error('unknown op');for(var k in payload){if(Object.prototype.hasOwnProperty.call(payload,k))s.setItem(k,String(payload[k]));}return true;}";

#[derive(Clone, Debug, PartialEq)]
pub struct Cookie {
    pub name: String,
    pub value: String,
    pub domain: String,
    pub path: String,
    /// Unix seconds; `None` is a session cookie.
    pub expires: Option<f64>,
    pub http_only: bool,
    pub secure: bool,
    /// `Strict`, `Lax`, or `None`.
    pub same_site: Option<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct OriginStorage {
    pub origin: String,
    pub local: BTreeMap<String, String>,
    pub session: BTreeMap<String, String>,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct StorageState {
    pub cookies: Vec<Cookie>,
    pub origins: Vec<OriginStorage>,
}

/// What [`load_state`] applied, and what waits for a navigation.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct LoadReport {
    pub cookies: usize,
    pub applied: Option<String>,
    pub pending: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum StateError {
    Io {
        path: String,
        message: String,
    },
    /// The file is not a valid version-1 state.
    Format(String),
    /// The file is readable by group or others (unix mode bits).
    Permissions {
        path: String,
        mode: u32,
    },
    Browser(BrowserError),
}

impl std::fmt::Display for StateError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io { path, message } => write!(f, "state file {path}: {message}"),
            Self::Format(message) => write!(f, "state file: {message}"),
            Self::Permissions { path, mode } => write!(
                f,
                "state file {path} has mode {mode:04o}: group or others can read it; run `chmod 600 {path}`"
            ),
            Self::Browser(err) => write!(f, "{err}"),
        }
    }
}

impl std::error::Error for StateError {}

impl From<BrowserError> for StateError {
    fn from(value: BrowserError) -> Self {
        Self::Browser(value)
    }
}

impl From<ultra_instinct_cdp::CdpError> for StateError {
    fn from(value: ultra_instinct_cdp::CdpError) -> Self {
        Self::Browser(BrowserError::Cdp(value))
    }
}

fn bad(message: impl Into<String>) -> StateError {
    StateError::Format(message.into())
}

/// Lend a transport to a short-lived [`BrowserSession`] without giving it up.
pub struct BorrowedTransport<'a, T: CdpTransport + ?Sized>(pub &'a mut T);

impl<T: CdpTransport + ?Sized> CdpTransport for BorrowedTransport<'_, T> {
    fn call(
        &mut self,
        method: &str,
        params_json: &str,
    ) -> Result<String, ultra_instinct_cdp::CdpError> {
        self.0.call(method, params_json)
    }
    fn drain_events(&mut self) -> Vec<ultra_instinct_cdp::CdpEvent> {
        self.0.drain_events()
    }
}

impl Cookie {
    /// From a CDP `Network.Cookie` (`strict` off: extra fields ignored) or a
    /// state-file cookie (`strict`: unknown fields are errors).
    fn from_json(v: &Value, strict: bool) -> Result<Self, StateError> {
        let obj = v
            .as_object()
            .ok_or_else(|| bad("cookie must be an object"))?;
        const KEYS: [&str; 8] = [
            "name", "value", "domain", "path", "expires", "httpOnly", "secure", "sameSite",
        ];
        if let Some(k) = obj.keys().find(|k| strict && !KEYS.contains(&k.as_str())) {
            return Err(bad(format!("unknown cookie field `{k}`")));
        }
        let text = |k: &str| {
            obj.get(k)
                .and_then(Value::as_str)
                .map(str::to_owned)
                .ok_or_else(|| bad(format!("cookie `{k}` must be a string")))
        };
        let flag = |k: &str| match obj.get(k) {
            None => Ok(false),
            Some(Value::Bool(b)) => Ok(*b),
            Some(_) => Err(bad(format!("cookie `{k}` must be a boolean"))),
        };
        let (name, domain, path) = (text("name")?, text("domain")?, text("path")?);
        if name.is_empty() || domain.is_empty() || !path.starts_with('/') {
            return Err(bad(
                "cookie needs a name, a domain, and a path starting with `/`",
            ));
        }
        let session = obj.get("session").and_then(Value::as_bool).unwrap_or(false);
        let expires = match obj.get("expires") {
            None | Some(Value::Null) => None,
            Some(Value::Number(n)) => n.as_f64().filter(|e| e.is_finite() && *e > 0.0 && !session),
            Some(_) => return Err(bad("cookie `expires` must be a number")),
        };
        let same_site = match obj.get("sameSite") {
            None | Some(Value::Null) => None,
            Some(Value::String(s)) if ["Strict", "Lax", "None"].contains(&s.as_str()) => {
                Some(s.clone())
            }
            Some(other) => {
                return Err(bad(format!(
                    "cookie `sameSite` must be Strict, Lax, or None, got {other}"
                )))
            }
        };
        Ok(Self {
            name,
            value: text("value")?,
            domain,
            path,
            expires,
            http_only: flag("httpOnly")?,
            secure: flag("secure")?,
            same_site,
        })
    }

    /// The state-file cookie, also a valid CDP `Network.CookieParam`.
    pub fn to_json(&self) -> Value {
        let mut v = json!({"name": self.name, "value": self.value, "domain": self.domain, "path": self.path, "httpOnly": self.http_only, "secure": self.secure});
        if let Some(e) = self.expires {
            v["expires"] = json!(e);
        }
        if let Some(s) = &self.same_site {
            v["sameSite"] = json!(s);
        }
        v
    }
}

fn is_origin(origin: &str) -> bool {
    let rest = origin
        .strip_prefix("https://")
        .or_else(|| origin.strip_prefix("http://"));
    rest.is_some_and(|r| {
        !r.is_empty()
            && !r.chars().any(|c| {
                matches!(c, '/' | '?' | '#' | '@' | '\\') || c.is_whitespace() || c.is_control()
            })
    })
}

fn entries(v: Option<&Value>, what: &str) -> Result<BTreeMap<String, String>, StateError> {
    match v {
        None | Some(Value::Null) => Ok(BTreeMap::new()),
        Some(Value::Object(map)) => map
            .iter()
            .map(|(k, v)| match v {
                Value::String(s) => Ok((k.clone(), s.clone())),
                _ => Err(bad(format!("{what} `{k}` must be a string"))),
            })
            .collect(),
        Some(_) => Err(bad(format!("{what} must be an object of strings"))),
    }
}

impl StorageState {
    /// Parse a state file. Untrusted input: never panics.
    pub fn parse(text: &str) -> Result<Self, StateError> {
        let root: Value = serde_json::from_str(text).map_err(|e| bad(format!("not JSON: {e}")))?;
        let obj = root
            .as_object()
            .ok_or_else(|| bad("top level must be an object"))?;
        if let Some(k) = obj
            .keys()
            .find(|k| !["version", "cookies", "origins"].contains(&k.as_str()))
        {
            return Err(bad(format!("unknown field `{k}`")));
        }
        match obj.get("version").and_then(Value::as_u64) {
            Some(STATE_VERSION) => {}
            Some(other) => {
                return Err(bad(format!(
                    "unsupported version {other}; this build reads {STATE_VERSION}"
                )))
            }
            None => return Err(bad("missing integer `version`")),
        }
        let list = |k: &str| match obj.get(k) {
            None => Ok(&[][..]),
            Some(Value::Array(a)) => Ok(a.as_slice()),
            Some(_) => Err(bad(format!("`{k}` must be an array"))),
        };
        let cookies = list("cookies")?
            .iter()
            .map(|c| Cookie::from_json(c, true))
            .collect::<Result<Vec<_>, _>>()?;
        let mut origins: Vec<OriginStorage> = Vec::new();
        for o in list("origins")? {
            let map = o
                .as_object()
                .ok_or_else(|| bad("origin entry must be an object"))?;
            if let Some(k) = map
                .keys()
                .find(|k| !["origin", "local", "session"].contains(&k.as_str()))
            {
                return Err(bad(format!("unknown origin field `{k}`")));
            }
            let origin = map
                .get("origin")
                .and_then(Value::as_str)
                .filter(|o| is_origin(o))
                .ok_or_else(|| bad("`origin` must be http(s)://host[:port]"))?;
            if origins.iter().any(|seen| seen.origin == origin) {
                return Err(bad(format!("duplicate origin {origin}")));
            }
            origins.push(OriginStorage {
                origin: origin.to_owned(),
                local: entries(map.get("local"), "local")?,
                session: entries(map.get("session"), "session")?,
            });
        }
        Ok(Self { cookies, origins })
    }

    pub fn to_json(&self) -> Value {
        let origins: Vec<Value> = self
            .origins
            .iter()
            .map(|o| json!({"origin": o.origin, "local": o.local, "session": o.session}))
            .collect();
        json!({"version": STATE_VERSION, "cookies": self.cookies.iter().map(Cookie::to_json).collect::<Vec<_>>(), "origins": origins})
    }

    pub fn origin(&self, origin: &str) -> Option<&OriginStorage> {
        self.origins.iter().find(|o| o.origin == origin)
    }
}

/// Write `state` to `path` with mode 0600, atomically (temp file + rename).
pub fn write_state_file(path: &Path, state: &StorageState) -> Result<(), StateError> {
    let io = |e: std::io::Error| StateError::Io {
        path: path.display().to_string(),
        message: e.to_string(),
    };
    let name = path
        .file_name()
        .ok_or_else(|| io(std::io::ErrorKind::InvalidInput.into()))?;
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.subsec_nanos())
        .unwrap_or(0);
    let tmp = path.with_file_name(format!(
        ".{}.tmp-{}-{nanos}",
        name.to_string_lossy(),
        std::process::id()
    ));
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);
    let body = serde_json::to_vec_pretty(&state.to_json()).map_err(|e| bad(e.to_string()))?;
    let written = options
        .open(&tmp)
        .and_then(|mut file| {
            file.write_all(&body)?;
            file.write_all(b"\n")?;
            file.sync_all()
        })
        .and_then(|()| std::fs::rename(&tmp, path));
    if written.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    written.map_err(io)
}

/// Read and parse a state file. On unix, a file group or others can access
/// (any of mode `0o077`) is refused with a `chmod 600` hint.
pub fn read_state_file(path: &Path) -> Result<StorageState, StateError> {
    let io = |e: std::io::Error| StateError::Io {
        path: path.display().to_string(),
        message: e.to_string(),
    };
    let file = std::fs::File::open(path).map_err(io)?;
    let meta = file.metadata().map_err(io)?;
    #[cfg(unix)]
    {
        let mode = std::os::unix::fs::PermissionsExt::mode(&meta.permissions()) & 0o777;
        if mode & 0o077 != 0 {
            return Err(StateError::Permissions {
                path: path.display().to_string(),
                mode,
            });
        }
    }
    if meta.len() > MAX_STATE_BYTES {
        return Err(bad(format!("larger than {MAX_STATE_BYTES} bytes")));
    }
    let mut text = String::new();
    file.take(MAX_STATE_BYTES + 1)
        .read_to_string(&mut text)
        .map_err(io)?;
    StorageState::parse(&text)
}

fn call<T: CdpTransport>(
    session: &mut BrowserSession<T>,
    method: &str,
    params: Value,
) -> Result<Value, StateError> {
    let body = session.transport_mut().call(method, &params.to_string())?;
    serde_json::from_str(&body).map_err(|e| {
        StateError::Browser(BrowserError::Cdp(ultra_instinct_cdp::CdpError::BadJson {
            message: e.to_string(),
        }))
    })
}

/// Every cookie the browser holds.
pub fn get_cookies<T: CdpTransport>(
    session: &mut BrowserSession<T>,
) -> Result<Vec<Cookie>, StateError> {
    let body = call(session, "Storage.getCookies", json!({}))
        .or_else(|_| call(session, "Network.getAllCookies", json!({})))?;
    Ok(body
        .get("cookies")
        .and_then(Value::as_array)
        .map(|list| {
            list.iter()
                .filter_map(|c| Cookie::from_json(c, false).ok())
                .collect()
        })
        .unwrap_or_default())
}

pub fn set_cookies<T: CdpTransport>(
    session: &mut BrowserSession<T>,
    cookies: &[Cookie],
) -> Result<(), StateError> {
    if !cookies.is_empty() {
        call(
            session,
            "Network.setCookies",
            json!({"cookies": cookies.iter().map(Cookie::to_json).collect::<Vec<_>>()}),
        )?;
    }
    Ok(())
}

/// `Network.clearBrowserCookies` with no filter; otherwise delete each cookie
/// whose name equals `name` and whose domain (leading dot ignored) is `domain`.
/// Returns how many were deleted (`None` for clear-all).
pub fn clear_cookies<T: CdpTransport>(
    session: &mut BrowserSession<T>,
    name: Option<&str>,
    domain: Option<&str>,
) -> Result<Option<usize>, StateError> {
    if name.is_none() && domain.is_none() {
        call(session, "Network.clearBrowserCookies", json!({}))?;
        return Ok(None);
    }
    let doomed: Vec<Cookie> = get_cookies(session)?
        .into_iter()
        .filter(|c| cookie_matches(c, name, domain))
        .collect();
    for c in &doomed {
        call(
            session,
            "Network.deleteCookies",
            json!({"name": c.name, "domain": c.domain, "path": c.path}),
        )?;
    }
    Ok(Some(doomed.len()))
}

pub fn cookie_matches(c: &Cookie, name: Option<&str>, domain: Option<&str>) -> bool {
    name.is_none_or(|n| c.name == n)
        && domain.is_none_or(|d| c.domain.trim_start_matches('.') == d.trim_start_matches('.'))
}

/// The current page's origin, hostname, and both storage areas (`None` where
/// the page forbids storage, e.g. an opaque origin).
pub fn read_storage<T: CdpTransport>(
    session: &mut BrowserSession<T>,
) -> Result<(OriginStorage, String), StateError> {
    let body = call(
        session,
        "Runtime.evaluate",
        json!({"expression": READ_STORAGE_JS, "returnByValue": true}),
    )?;
    let value = body
        .get("result")
        .and_then(|r| r.get("value"))
        .cloned()
        .unwrap_or(Value::Null);
    let text = |k: &str| {
        value
            .get(k)
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_owned()
    };
    let storage = OriginStorage {
        origin: text("origin"),
        local: entries(value.get("local"), "local")?,
        session: entries(value.get("session"), "session")?,
    };
    Ok((storage, text("host")))
}

/// Run [`WRITE_STORAGE_FUNCTION`] on `globalThis`; arguments are CDP values.
pub fn write_storage<T: CdpTransport>(
    session: &mut BrowserSession<T>,
    origin: Option<&str>,
    area: &str,
    op: &str,
    payload: Value,
) -> Result<(), StateError> {
    let global = call(
        session,
        "Runtime.evaluate",
        json!({"expression": "globalThis"}),
    )?;
    let object_id = global
        .get("result")
        .and_then(|r| r.get("objectId"))
        .and_then(Value::as_str)
        .ok_or(StateError::Browser(BrowserError::MissingObjectId))?
        .to_owned();
    let args = [json!(origin), json!(area), json!(op), payload].map(|v| json!({"value": v}));
    let body = call(
        session,
        "Runtime.callFunctionOn",
        json!({"objectId": object_id, "functionDeclaration": WRITE_STORAGE_FUNCTION, "arguments": args, "returnByValue": true}),
    )?;
    if let Some(details) = body.get("exceptionDetails") {
        let message = details
            .pointer("/exception/description")
            .or_else(|| details.get("text"))
            .and_then(Value::as_str)
            .unwrap_or("page threw");
        return Err(StateError::Browser(BrowserError::InputRejected(
            message.lines().next().unwrap_or(message).to_owned(),
        )));
    }
    Ok(())
}

/// Cookies plus the current origin's storage.
pub fn save_state<T: CdpTransport>(
    session: &mut BrowserSession<T>,
) -> Result<StorageState, StateError> {
    let cookies = get_cookies(session)?;
    let (storage, _) = read_storage(session)?;
    let keep =
        is_origin(&storage.origin) && !(storage.local.is_empty() && storage.session.is_empty());
    Ok(StorageState {
        cookies,
        origins: if keep { vec![storage] } else { Vec::new() },
    })
}

/// Write the storage `state` holds for the page's current origin, if any.
/// Returns that origin when something was written.
pub fn apply_origin_storage<T: CdpTransport>(
    session: &mut BrowserSession<T>,
    state: &StorageState,
) -> Result<Option<String>, StateError> {
    if state.origins.is_empty() {
        return Ok(None);
    }
    let body = call(
        session,
        "Runtime.evaluate",
        json!({"expression": "location.origin", "returnByValue": true}),
    )?;
    let current = body
        .pointer("/result/value")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let Some(entry) = state.origin(current) else {
        return Ok(None);
    };
    for (area, map) in [("local", &entry.local), ("session", &entry.session)] {
        if !map.is_empty() {
            let payload: Map<String, Value> =
                map.iter().map(|(k, v)| (k.clone(), json!(v))).collect();
            write_storage(
                session,
                Some(&entry.origin),
                area,
                "set",
                Value::Object(payload),
            )?;
        }
    }
    Ok(Some(entry.origin.clone()))
}

/// Set every cookie, then the current origin's storage; the other origins
/// are `pending` until the page navigates there (see the module docs).
pub fn load_state<T: CdpTransport>(
    session: &mut BrowserSession<T>,
    state: &StorageState,
) -> Result<LoadReport, StateError> {
    set_cookies(session, &state.cookies)?;
    let applied = apply_origin_storage(session, state)?;
    let pending = state
        .origins
        .iter()
        .map(|o| o.origin.clone())
        .filter(|o| Some(o) != applied.as_ref())
        .collect();
    Ok(LoadReport {
        cookies: state.cookies.len(),
        applied,
        pending,
    })
}
