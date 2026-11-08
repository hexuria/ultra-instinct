//! `browser_state_*`, `browser_cookies`, `browser_storage`: save and reuse a
//! login (ported from agent-browser `state` / `cookies` / `storage`).
//!
//! Cookie and storage values are masked (`"•••"` plus `value_length`) in list
//! output unless the call passes `reveal: true`, so session tokens stay out
//! of a model's context by default. `browser_state_save` never prints values:
//! it returns the path and counts. Storage writes go through
//! `ultra_instinct_browser::storage_state`'s fixed function with CDP
//! arguments; nothing here splices a value into script source.

use std::collections::BTreeMap;
use std::path::Path;

use serde_json::{json, Map, Value};
use ultra_instinct_browser::storage_state::{
    clear_cookies, cookie_matches, get_cookies, load_state, read_state_file, read_storage,
    save_state, set_cookies, write_state_file, write_storage, Cookie, StateError,
};
use ultra_instinct_cdp::DEFAULT_CDP_HTTP;

use crate::error::ToolError;
use crate::server::{LiveSession, Server};

/// Tool names this module serves.
pub const STATE_TOOLS: [&str; 4] = [
    "browser_state_save",
    "browser_state_load",
    "browser_cookies",
    "browser_storage",
];

/// What a masked value reads as.
pub const MASK: &str = "•••";

/// `None` when `name` is not one of [`STATE_TOOLS`].
pub(crate) fn call(
    server: &mut Server,
    name: &str,
    arguments: &Value,
) -> Option<Result<Value, ToolError>> {
    if !STATE_TOOLS.contains(&name) {
        return None;
    }
    Some(with_session(server, arguments, |session| match name {
        "browser_state_save" => state_save(session, arguments),
        "browser_state_load" => state_load(session, arguments),
        "browser_cookies" => cookies(session, arguments),
        _ => storage(session, arguments),
    }))
}

/// Run `f` on the session the `cdp` endpoint (or its current tab) drives.
fn with_session(
    server: &mut Server,
    arguments: &Value,
    f: impl FnOnce(&mut LiveSession) -> Result<Value, ToolError>,
) -> Result<Value, ToolError> {
    let endpoint = opt_str(arguments, "cdp")?.unwrap_or(DEFAULT_CDP_HTTP);
    let key = server
        .current_tab(endpoint)
        .map_or_else(|| endpoint.to_owned(), str::to_owned);
    let mut session = server.take_session(&key)?;
    let outcome = f(&mut session);
    let events = session.transport_mut().drain_events();
    if !events.is_empty() {
        server.push_events(&key, events);
    }
    server.keep_session(&key, session);
    outcome
}

fn opt_str<'a>(arguments: &'a Value, key: &str) -> Result<Option<&'a str>, ToolError> {
    match arguments.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(s)) => Ok(Some(s.as_str())),
        Some(other) => Err(invalid(format!("`{key}` must be a string, got {other}"))),
    }
}

fn req_str<'a>(arguments: &'a Value, key: &str) -> Result<&'a str, ToolError> {
    opt_str(arguments, key)?.ok_or_else(|| invalid(format!("missing required `{key}`")))
}

fn opt_bool(arguments: &Value, key: &str) -> Result<Option<bool>, ToolError> {
    match arguments.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::Bool(b)) => Ok(Some(*b)),
        Some(other) => Err(invalid(format!("`{key}` must be a boolean, got {other}"))),
    }
}

fn invalid(message: String) -> ToolError {
    ToolError::InvalidArguments(message)
}

fn state_err(err: StateError) -> ToolError {
    match err {
        StateError::Browser(err) => ToolError::Browser(err.to_string()),
        other => invalid(other.to_string()),
    }
}

/// `{"value": "•••", "value_length": n}` unless revealed.
fn masked(value: &str, reveal: bool) -> Value {
    if reveal {
        json!({"value": value})
    } else {
        json!({"value": MASK, "value_length": value.chars().count()})
    }
}

fn merge(mut base: Value, extra: Value) -> Value {
    if let (Some(base), Value::Object(extra)) = (base.as_object_mut(), extra) {
        base.extend(extra);
    }
    base
}

fn state_save(session: &mut LiveSession, arguments: &Value) -> Result<Value, ToolError> {
    let path = req_str(arguments, "path")?;
    let state = save_state(session).map_err(state_err)?;
    write_state_file(Path::new(path), &state).map_err(state_err)?;
    let origins: Vec<Value> = state
        .origins
        .iter()
        .map(|o| json!({"origin": o.origin, "local": o.local.len(), "session": o.session.len()}))
        .collect();
    Ok(json!({"path": path, "cookies": state.cookies.len(), "origins": origins}))
}

fn state_load(session: &mut LiveSession, arguments: &Value) -> Result<Value, ToolError> {
    let path = req_str(arguments, "path")?;
    let state = read_state_file(Path::new(path)).map_err(state_err)?;
    let report = load_state(session, &state).map_err(state_err)?;
    let mut body = json!({
        "path": path,
        "cookies": report.cookies,
        "applied_origin": report.applied,
        "pending_origins": report.pending,
    });
    if !report.pending.is_empty() {
        body["hint"] = json!(
            "storage for pending origins is written when the tab is on that origin: browser_navigate there, then call browser_state_load again"
        );
    }
    Ok(body)
}

fn cookie_json(cookie: &Cookie, reveal: bool) -> Value {
    merge(
        json!({
            "name": cookie.name,
            "domain": cookie.domain,
            "path": cookie.path,
            "expires": cookie.expires,
            "http_only": cookie.http_only,
            "secure": cookie.secure,
            "same_site": cookie.same_site,
        }),
        masked(&cookie.value, reveal),
    )
}

fn cookies(session: &mut LiveSession, arguments: &Value) -> Result<Value, ToolError> {
    let action = opt_str(arguments, "action")?.unwrap_or("list");
    let name = opt_str(arguments, "name")?;
    let domain = opt_str(arguments, "domain")?;
    match action {
        "list" => {
            let reveal = opt_bool(arguments, "reveal")?.unwrap_or(false);
            let list: Vec<Value> = get_cookies(session)
                .map_err(state_err)?
                .iter()
                .filter(|c| cookie_matches(c, name, domain))
                .map(|c| cookie_json(c, reveal))
                .collect();
            Ok(json!({"count": list.len(), "cookies": list}))
        }
        "set" => {
            let name = name.ok_or_else(|| invalid("`set` needs `name`".to_owned()))?;
            let value = req_str(arguments, "value")?;
            let domain = match domain {
                Some(d) => d.to_owned(),
                // Host-only cookie for the page the tab is on.
                None => read_storage(session).map_err(state_err)?.1,
            };
            if domain.is_empty() {
                return Err(invalid(
                    "the page has no host; pass `domain` (or navigate first)".to_owned(),
                ));
            }
            let same_site = opt_str(arguments, "same_site")?
                .map(|s| match s.to_ascii_lowercase().as_str() {
                    "strict" => Ok("Strict"),
                    "lax" => Ok("Lax"),
                    "none" => Ok("None"),
                    _ => Err(invalid(format!(
                        "`same_site` must be Strict, Lax, or None, got {s:?}"
                    ))),
                })
                .transpose()?;
            let expires = match arguments.get("expires") {
                None | Some(Value::Null) => None,
                Some(v) => Some(
                    v.as_f64()
                        .filter(|e| e.is_finite() && *e > 0.0)
                        .ok_or_else(|| {
                            invalid("`expires` must be positive unix seconds".to_owned())
                        })?,
                ),
            };
            let cookie = Cookie {
                name: name.to_owned(),
                value: value.to_owned(),
                domain,
                path: opt_str(arguments, "path")?.unwrap_or("/").to_owned(),
                expires,
                http_only: opt_bool(arguments, "http_only")?.unwrap_or(false),
                secure: opt_bool(arguments, "secure")?.unwrap_or(false),
                same_site: same_site.map(str::to_owned),
            };
            if !cookie.path.starts_with('/') {
                return Err(invalid("`path` must start with `/`".to_owned()));
            }
            set_cookies(session, std::slice::from_ref(&cookie)).map_err(state_err)?;
            Ok(json!({"set": cookie_json(&cookie, false)}))
        }
        "clear" => match clear_cookies(session, name, domain).map_err(state_err)? {
            None => Ok(json!({"cleared": "all"})),
            Some(n) => Ok(json!({"cleared": n})),
        },
        other => Err(invalid(format!(
            "`action` must be list, set, or clear, got {other:?}"
        ))),
    }
}

fn storage(session: &mut LiveSession, arguments: &Value) -> Result<Value, ToolError> {
    let area = match req_str(arguments, "area")? {
        a @ ("local" | "session") => a,
        other => {
            return Err(invalid(format!(
                "`area` must be local or session, got {other:?}"
            )))
        }
    };
    let action = opt_str(arguments, "action")?.unwrap_or("list");
    let key = opt_str(arguments, "key")?;
    let pick =
        |session: &mut LiveSession| -> Result<(String, BTreeMap<String, String>), ToolError> {
            let (storage, _) = read_storage(session).map_err(state_err)?;
            let map = if area == "local" {
                storage.local
            } else {
                storage.session
            };
            Ok((storage.origin, map))
        };
    match action {
        "list" => {
            let reveal = opt_bool(arguments, "reveal")?.unwrap_or(false);
            let (origin, map) = pick(session)?;
            let entries: Map<String, Value> = map
                .iter()
                .map(|(k, v)| (k.clone(), masked(v, reveal)))
                .collect();
            Ok(json!({"origin": origin, "area": area, "count": entries.len(), "entries": entries}))
        }
        // An explicit single-key read is the reveal.
        "get" => {
            let key = key.ok_or_else(|| invalid("`get` needs `key`".to_owned()))?;
            let (origin, map) = pick(session)?;
            Ok(json!({"origin": origin, "area": area, "key": key, "value": map.get(key)}))
        }
        "set" => {
            let key = key.ok_or_else(|| invalid("`set` needs `key`".to_owned()))?;
            let value = req_str(arguments, "value")?;
            write_storage(session, None, area, "set", json!({ key: value })).map_err(state_err)?;
            Ok(json!({"area": area, "set": key, "value_length": value.chars().count()}))
        }
        "clear" => {
            match key {
                Some(key) => write_storage(session, None, area, "remove", json!([key])),
                None => write_storage(session, None, area, "clear", Value::Null),
            }
            .map_err(state_err)?;
            Ok(json!({"area": area, "cleared": key.map_or(json!("all"), |k| json!(k))}))
        }
        other => Err(invalid(format!(
            "`action` must be list, get, set, or clear, got {other:?}"
        ))),
    }
}

/// Tool spec for a [`STATE_TOOLS`] name: description, properties, required.
pub(crate) fn spec(name: &str) -> Option<(String, Map<String, Value>, Vec<&'static str>)> {
    let mut props = Map::new();
    props.insert(
        "cdp".into(),
        json!({"type": "string", "description": format!("CDP debugging endpoint; default {DEFAULT_CDP_HTTP}")}),
    );
    let mut required = Vec::new();
    let string = || json!({"type": "string"});
    let boolean = || json!({"type": "boolean"});
    let description = match name {
        "browser_state_save" => {
            props.insert("path".into(), string());
            required.push("path");
            "Save every cookie plus the current origin's localStorage and sessionStorage to a version-1 JSON file (mode 0600, atomic). Returns counts only, never values."
        }
        "browser_state_load" => {
            props.insert("path".into(), string());
            required.push("path");
            "Load a saved state: sets its cookies, and writes the storage of the origin the tab is on now. Other origins come back as pending_origins: navigate there and load again. Refuses files group or others can read."
        }
        "browser_cookies" => {
            props.insert(
                "action".into(),
                json!({"type": "string", "enum": ["list", "set", "clear"]}),
            );
            for key in ["name", "value", "domain", "path"] {
                props.insert(key.into(), string());
            }
            props.insert("expires".into(), json!({"type": "number"}));
            props.insert("http_only".into(), boolean());
            props.insert("secure".into(), boolean());
            props.insert(
                "same_site".into(),
                json!({"type": "string", "enum": ["Strict", "Lax", "None"]}),
            );
            props.insert("reveal".into(), boolean());
            "Cookies: list (filter by name/domain; values masked unless reveal: true), set (name + value; domain defaults to the page's host), clear (all, or those matching name/domain)."
        }
        "browser_storage" => {
            props.insert(
                "area".into(),
                json!({"type": "string", "enum": ["local", "session"]}),
            );
            props.insert(
                "action".into(),
                json!({"type": "string", "enum": ["list", "get", "set", "clear"]}),
            );
            props.insert("key".into(), string());
            props.insert("value".into(), string());
            props.insert("reveal".into(), boolean());
            required.push("area");
            "Current origin's localStorage or sessionStorage: list (values masked unless reveal: true), get key, set key value, clear (one key, or the whole area)."
        }
        _ => return None,
    };
    Some((description.to_owned(), props, required))
}
