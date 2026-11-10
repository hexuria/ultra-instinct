//! Blocking CDP websocket. Plain `ws://` only. `wss://` is refused: this
//! crate does not pull a TLS stack. Chrome's local debugging port is `ws`.

use std::collections::{BTreeMap, VecDeque};
use std::io::{ErrorKind, Read, Write};
use std::net::{TcpStream, ToSocketAddrs};
use std::time::{Duration, Instant};

use serde_json::value::RawValue;
use serde_json::{json, Value};
use tungstenite::{Message, WebSocket};

use crate::{CdpError, CdpEvent, CdpTransport};

/// Buffered unsolicited events kept per transport; oldest drop past this.
pub(crate) const MAX_BUFFERED_EVENTS: usize = 1024;

/// Documented default. Pass `--cdp` with no value to use it. A live Chrome
/// must already be listening; ultra-instinct does not launch a browser.
pub const DEFAULT_CDP_HTTP: &str = "http://127.0.0.1:9222";

/// The event that ends a call's wait early: see [`CdpError::DialogOpened`].
pub(crate) const DIALOG_OPENING: &str = "Page.javascriptDialogOpening";

/// Maximum time to wait for a CDP response after sending a call.
pub(crate) const CALL_DEADLINE: Duration = Duration::from_secs(30);

/// TCP connect deadline and per-read/write timeout. The ws handshake runs on
/// a socket that already carries these, so a silent peer fails fast.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);
const IO_TIMEOUT: Duration = Duration::from_secs(5);

type Socket = WebSocket<TcpStream>;

#[derive(Clone)]
struct OwnedTarget {
    http: String,
    target_id: String,
}

pub struct WebSocketTransport {
    socket: Socket,
    next_id: i64,
    owned: Option<OwnedTarget>,
    events: VecDeque<CdpEvent>,
}

impl WebSocketTransport {
    pub fn connect(endpoint: &str) -> Result<Self, CdpError> {
        let ws_url = resolve_websocket_url(endpoint)?;
        let mut transport = Self {
            socket: connect_socket(&ws_url)?,
            next_id: 1,
            owned: None,
            events: VecDeque::new(),
        };
        // Match `open_tab`: trusted key/mouse input needs emulated focus or
        // it lands on whatever Chrome happens to have focused.
        transport.call(
            "Emulation.setFocusEmulationEnabled",
            &json!({"enabled": true}).to_string(),
        )?;
        Ok(transport)
    }

    fn open_tab_inner(endpoint: &str) -> Result<Self, CdpError> {
        let http = http_endpoint(endpoint)?.to_owned();
        let result = browser_ws_call(
            &http,
            "Target.createTarget",
            &json!({"url": "about:blank", "background": true}).to_string(),
        )?;
        let target_id = created_target_id(&result)?;

        let owned = OwnedTarget {
            http: http.clone(),
            target_id: target_id.clone(),
        };
        let result: Result<Self, CdpError> = (|| {
            let page_url = page_socket_url(&http, &target_id)?;
            let mut transport = Self {
                socket: connect_socket(&page_url)?,
                next_id: 1,
                owned: None,
                events: VecDeque::new(),
            };
            transport.call(
                "Emulation.setFocusEmulationEnabled",
                &json!({"enabled": true}).to_string(),
            )?;
            transport.owned = Some(owned.clone());
            Ok(transport)
        })();
        if result.is_err() {
            close_owned_target(&owned);
        }
        result
    }

    pub fn target_id(&self) -> Option<&str> {
        self.owned.as_ref().map(|target| target.target_id.as_str())
    }
}

pub fn open_tab(endpoint: &str) -> Result<WebSocketTransport, CdpError> {
    WebSocketTransport::open_tab_inner(endpoint)
}

/// One `"type": "page"` entry from `/json/list` — a tab.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PageTarget {
    /// Chrome target id (also the last path segment of `ws_url`).
    pub target_id: String,
    /// Document URL the tab currently shows.
    pub url: String,
    /// Tab title.
    pub title: String,
    /// Page-level websocket URL for driving this tab.
    pub ws_url: String,
}

/// Page targets at an `http://` debugging endpoint, in `/json/list` order.
pub fn page_targets(endpoint: &str) -> Result<Vec<PageTarget>, CdpError> {
    let http = http_endpoint(endpoint)?;
    let body = http_get(&format!("{http}/json/list"))?;
    let value: Value = serde_json::from_str(&body).map_err(|err| CdpError::BadJson {
        message: err.to_string(),
    })?;
    Ok(value
        .as_array()
        .into_iter()
        .flatten()
        .filter(|target| target.get("type").and_then(Value::as_str) == Some("page"))
        .filter_map(|target| {
            Some(PageTarget {
                target_id: target.get("id").and_then(Value::as_str)?.to_owned(),
                url: target
                    .get("url")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_owned(),
                title: target
                    .get("title")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_owned(),
                ws_url: target
                    .get("webSocketDebuggerUrl")
                    .and_then(Value::as_str)?
                    .to_owned(),
            })
        })
        .collect())
}

/// One CDP call on the browser target (`Target.*`, `Browser.*` domains).
fn browser_ws_call(endpoint: &str, method: &str, params: &str) -> Result<String, CdpError> {
    let http = http_endpoint(endpoint)?;
    let version = http_get(&format!("{http}/json/version"))?;
    let browser_url = version_websocket_url(&version)?;
    let mut browser = WebSocketTransport {
        socket: connect_socket(&browser_url)?,
        next_id: 1,
        owned: None,
        events: VecDeque::new(),
    };
    browser.call(method, params)
}

/// Bring a page target to the front (`Target.activateTarget`).
pub fn activate_target(endpoint: &str, target_id: &str) -> Result<(), CdpError> {
    browser_ws_call(
        endpoint,
        "Target.activateTarget",
        &json!({"targetId": target_id}).to_string(),
    )?;
    Ok(())
}

/// Close a page target (`Target.closeTarget`). Returns the target's
/// reported closure flag, false when the browser refused.
pub fn close_target(endpoint: &str, target_id: &str) -> Result<bool, CdpError> {
    let body = browser_ws_call(
        endpoint,
        "Target.closeTarget",
        &json!({"targetId": target_id}).to_string(),
    )?;
    let value: Value = serde_json::from_str(&body).map_err(|err| CdpError::BadJson {
        message: err.to_string(),
    })?;
    value
        .get("success")
        .and_then(Value::as_bool)
        .ok_or_else(|| CdpError::BadJson {
            message: "closeTarget result missing `success`".to_owned(),
        })
}

/// Create a page target showing `url`; returns its id and ws url.
/// `activate` brings it to the front (`Target.createTarget`).
pub fn create_target(endpoint: &str, url: &str, activate: bool) -> Result<PageTarget, CdpError> {
    let body = browser_ws_call(
        endpoint,
        "Target.createTarget",
        &json!({"url": url, "background": !activate}).to_string(),
    )?;
    let target_id = created_target_id(&body)?;
    Ok(PageTarget {
        ws_url: page_socket_url(endpoint, &target_id)?,
        target_id,
        url: url.to_owned(),
        title: String::new(),
    })
}

impl Drop for WebSocketTransport {
    fn drop(&mut self) {
        if let Some(target) = self.owned.take() {
            close_owned_target(&target);
        }
    }
}

impl CdpTransport for WebSocketTransport {
    fn call(&mut self, method: &str, params_json: &str) -> Result<String, CdpError> {
        let params: Value = serde_json::from_str(params_json).map_err(|err| CdpError::BadJson {
            message: err.to_string(),
        })?;
        let id = self.next_id;
        self.next_id += 1;
        let envelope = json!({"id": id, "method": method, "params": params});
        self.socket
            .send(Message::Text(envelope.to_string().into()))
            .map_err(|err| CdpError::Transport {
                message: err.to_string(),
            })?;
        let deadline = Instant::now() + CALL_DEADLINE;
        loop {
            if Instant::now() >= deadline {
                return Err(call_deadline_error(method));
            }
            let message = match self.socket.read() {
                Ok(message) => message,
                Err(tungstenite::Error::Io(error))
                    if matches!(error.kind(), ErrorKind::TimedOut | ErrorKind::WouldBlock) =>
                {
                    if Instant::now() >= deadline {
                        return Err(call_deadline_error(method));
                    }
                    continue;
                }
                Err(err) => {
                    return Err(CdpError::Transport {
                        message: err.to_string(),
                    });
                }
            };
            let Message::Text(text) = message else {
                continue;
            };
            let Some(frame) = frame_fields(text.as_ref())? else {
                continue;
            };
            if frame
                .get("id")
                .and_then(|raw| raw.get().parse::<i64>().ok())
                != Some(id)
            {
                if let Some(event) = frame.get("method").and_then(|raw| raw_string(raw)) {
                    if let Some(params) = event_params(&frame) {
                        let dialog = event == DIALOG_OPENING;
                        if self.events.len() >= MAX_BUFFERED_EVENTS {
                            self.events.pop_front();
                        }
                        self.events.push_back(CdpEvent {
                            method: event,
                            params,
                        });
                        // The page answers nothing while a dialog is open:
                        // stop waiting. The event stays buffered. A late
                        // response to this id is ignored like any stray one
                        // (ids only grow).
                        if dialog {
                            return Err(CdpError::DialogOpened {
                                method: method.to_owned(),
                            });
                        }
                    }
                }
                continue;
            }
            return response_result(&frame);
        }
    }

    fn drain_events(&mut self) -> Vec<CdpEvent> {
        self.events.drain(..).collect()
    }
}

/// One websocket text frame as a field map of raw values. Payload fields
/// stay `RawValue` and are materialized only by the leaf parsers that know
/// whether they need depth: a `DOM.getDocument` `result` nests ~2 JSON
/// levels per DOM level, which dies at `serde_json`'s 128-level `Value` cap
/// on real pages (~60-deep DOMs). Callers already choose bounded vs deep
/// parsing (observe uses `parse_json_deep`), so the envelope decode only
/// locates the fields.
///
/// `Ok(None)` is a valid JSON frame that is not an object — not a CDP
/// message, ignored like any foreign frame. `Err(BadJson)` is malformed
/// JSON, same mapping `Value` parsing had.
pub(crate) fn frame_fields(
    text: &str,
) -> Result<Option<BTreeMap<String, Box<RawValue>>>, CdpError> {
    match serde_json::from_str(text) {
        Ok(fields) => Ok(Some(fields)),
        Err(err) if err.is_data() => Ok(None),
        Err(err) => Err(CdpError::BadJson {
            message: err.to_string(),
        }),
    }
}

/// A frame field's text when it is a JSON string, `None` otherwise.
pub(crate) fn raw_string(raw: &RawValue) -> Option<String> {
    serde_json::from_str(raw.get()).ok()
}

/// An event frame's `params`, `Null` when the field is absent. A frame
/// whose params overflow `Value`'s depth cap is undeliverable: the event is
/// dropped rather than failing the in-flight call it rode in with.
pub(crate) fn event_params(frame: &BTreeMap<String, Box<RawValue>>) -> Option<Value> {
    match frame.get("params") {
        Some(raw) => serde_json::from_str(raw.get()).ok(),
        None => Some(Value::Null),
    }
}

/// A response frame's `result` text (`null` when absent), or its `error`
/// object as [`CdpError::Protocol`].
pub(crate) fn response_result(frame: &BTreeMap<String, Box<RawValue>>) -> Result<String, CdpError> {
    if let Some(raw) = frame.get("error") {
        let error: Value = serde_json::from_str(raw.get()).map_err(|err| CdpError::BadJson {
            message: err.to_string(),
        })?;
        let message = error
            .get("message")
            .and_then(Value::as_str)
            .unwrap_or("CDP error")
            .to_owned();
        return Err(CdpError::Protocol { message });
    }
    Ok(frame
        .get("result")
        .map(|raw| raw.get().to_owned())
        .unwrap_or_else(|| "null".to_owned()))
}

fn call_deadline_error(method: &str) -> CdpError {
    deadline_error(method, CALL_DEADLINE)
}

/// The timeout a call reports when `deadline` passed without its result.
/// Its text keeps the transport wording: `CDP transport: no CDP result for
/// `X.y` within 30 s`.
pub(crate) fn deadline_error(method: &str, deadline: Duration) -> CdpError {
    CdpError::Timeout {
        method: method.to_owned(),
        after: deadline,
    }
}

fn connect_socket(ws_url: &str) -> Result<Socket, CdpError> {
    connect_plain(ws_url, IO_TIMEOUT, None)
}

/// A plain `ws://` socket to `ws_url`: TCP within the connect deadline, then
/// the blocking handshake on a socket whose reads and writes time out after
/// `handshake_wait` (the socket keeps those timeouts). `wss://` refuses.
pub(crate) fn connect_plain(
    ws_url: &str,
    handshake_wait: Duration,
    config: Option<tungstenite::protocol::WebSocketConfig>,
) -> Result<Socket, CdpError> {
    if ws_url.starts_with("wss://") {
        return Err(CdpError::Transport {
            message: "wss is not supported; use a local ws:// debugging port".into(),
        });
    }
    if !ws_url.starts_with("ws://") {
        return Err(CdpError::Transport {
            message: format!("websocket URL `{ws_url}` must use ws://"),
        });
    }
    // `tungstenite::connect` applies no deadline until it returns — a host
    // that accepts TCP but never completes the upgrade would stall forever.
    // Connect with a timeout, then run the blocking handshake over a socket
    // that already carries read/write timeouts.
    let authority = ws_url["ws://".len()..]
        .split('/')
        .next()
        .unwrap_or_default();
    let host_port = match authority.contains(':') {
        true => authority.to_owned(),
        false => format!("{authority}:80"),
    };
    let stream = connect_tcp(&host_port)?;
    stream
        .set_read_timeout(Some(handshake_wait))
        .and_then(|()| stream.set_write_timeout(Some(handshake_wait)))
        .map_err(|err| CdpError::Transport {
            message: err.to_string(),
        })?;
    let (socket, _response) = tungstenite::client::client_with_config(ws_url, stream, config)
        .map_err(|err| CdpError::Transport {
            message: err.to_string(),
        })?;
    Ok(socket)
}

fn connect_tcp(host_port: &str) -> Result<TcpStream, CdpError> {
    let mut last_err = String::from("no addresses resolved");
    for addr in host_port
        .to_socket_addrs()
        .map_err(|err| CdpError::Transport {
            message: format!("resolve `{host_port}`: {err}"),
        })?
    {
        match TcpStream::connect_timeout(&addr, CONNECT_TIMEOUT) {
            Ok(stream) => {
                stream.set_nodelay(true).ok();
                return Ok(stream);
            }
            Err(err) => last_err = format!("{addr}: {err}"),
        }
    }
    Err(CdpError::Transport {
        message: format!("connect `{host_port}`: {last_err}"),
    })
}

fn resolve_websocket_url(endpoint: &str) -> Result<String, CdpError> {
    if endpoint.starts_with("ws://") || endpoint.starts_with("wss://") {
        return Ok(endpoint.to_owned());
    }
    let http = endpoint.trim_end_matches('/');
    if !http.starts_with("http://") {
        return Err(CdpError::Transport {
            message: format!("CDP endpoint `{endpoint}` must be http:// or ws://"),
        });
    }
    // `/json/version` names the browser target, which has no `Page` domain.
    // The tools need a page, so pick the first page target from `/json/list`.
    first_page_websocket_url(&http_get(&format!("{http}/json/list"))?)
}

/// First `"type": "page"` target's websocket URL in a `/json/list` body.
fn first_page_websocket_url(body: &str) -> Result<String, CdpError> {
    let value: Value = serde_json::from_str(body).map_err(|err| CdpError::BadJson {
        message: err.to_string(),
    })?;
    value
        .as_array()
        .into_iter()
        .flatten()
        .filter(|target| target.get("type").and_then(Value::as_str) == Some("page"))
        .find_map(|target| target.get("webSocketDebuggerUrl").and_then(Value::as_str))
        .map(str::to_owned)
        .ok_or_else(|| CdpError::Transport {
            message: "json/list has no page target with a webSocketDebuggerUrl".into(),
        })
}

fn http_endpoint(endpoint: &str) -> Result<&str, CdpError> {
    let http = endpoint.trim_end_matches('/');
    if !http.starts_with("http://") || http.len() == "http://".len() {
        return Err(CdpError::Transport {
            message: "open_tab needs the http:// debugging endpoint".into(),
        });
    }
    Ok(http)
}

fn version_websocket_url(body: &str) -> Result<String, CdpError> {
    let value: Value = serde_json::from_str(body).map_err(|err| CdpError::BadJson {
        message: err.to_string(),
    })?;
    value
        .get("webSocketDebuggerUrl")
        .and_then(Value::as_str)
        .map(str::to_owned)
        .ok_or_else(|| CdpError::Transport {
            message: "json/version has no webSocketDebuggerUrl".into(),
        })
}

pub(crate) fn created_target_id(body: &str) -> Result<String, CdpError> {
    let value: Value = serde_json::from_str(body).map_err(|err| CdpError::BadJson {
        message: err.to_string(),
    })?;
    value
        .get("targetId")
        .and_then(Value::as_str)
        .map(str::to_owned)
        .ok_or_else(|| CdpError::Transport {
            message: "Target.createTarget result has no targetId".into(),
        })
}

fn page_socket_url(endpoint: &str, target_id: &str) -> Result<String, CdpError> {
    let http = http_endpoint(endpoint)?;
    let host = http
        .strip_prefix("http://")
        .unwrap_or_default()
        .split('/')
        .next()
        .unwrap_or_default();
    if host.is_empty() {
        return Err(CdpError::Transport {
            message: "open_tab needs an http:// debugging endpoint".into(),
        });
    }
    Ok(format!("ws://{host}/devtools/page/{target_id}"))
}

fn close_owned_target(target: &OwnedTarget) {
    let result = close_target(&target.http, &target.target_id).map(|_| ());
    if let Err(err) = result {
        eprintln!(
            "cdp: closing owned target {} failed: {err}",
            target.target_id
        );
    }
}

fn http_get(url: &str) -> Result<String, CdpError> {
    let rest = url
        .strip_prefix("http://")
        .ok_or_else(|| CdpError::Transport {
            message: format!("unsupported url `{url}`"),
        })?;
    let (host, path) = rest.split_once('/').unwrap_or((rest, ""));
    let path = format!("/{path}");
    let host_port = match host.contains(':') {
        true => host.to_owned(),
        false => format!("{host}:80"),
    };
    let mut stream = connect_tcp(&host_port)?;
    stream
        .set_read_timeout(Some(Duration::from_secs(5)))
        .map_err(|err| CdpError::Transport {
            message: err.to_string(),
        })?;
    let request = format!("GET {path} HTTP/1.1\r\nHost: {host}\r\nConnection: close\r\n\r\n");
    stream
        .write_all(request.as_bytes())
        .map_err(|err| CdpError::Transport {
            message: err.to_string(),
        })?;
    // Chrome often leaves this socket open. Reading to EOF hits the timeout,
    // and Linux reports that as EAGAIN. Stop at Content-Length instead.
    let (headers, mut body) = read_http(&mut stream)?;
    let headers_lower = headers.to_ascii_lowercase();
    if headers_lower.contains("transfer-encoding: chunked") {
        return decode_chunked(&String::from_utf8_lossy(&body));
    }
    if let Some(length) = content_length(&headers_lower) {
        while body.len() < length {
            let mut buf = [0u8; 1024];
            let n = stream.read(&mut buf).map_err(|err| CdpError::Transport {
                message: err.to_string(),
            })?;
            if n == 0 {
                break;
            }
            body.extend_from_slice(&buf[..n]);
        }
        body.truncate(length);
    }
    Ok(String::from_utf8_lossy(&body).into_owned())
}

fn read_http(stream: &mut TcpStream) -> Result<(String, Vec<u8>), CdpError> {
    let mut buf = Vec::new();
    let mut chunk = [0u8; 1024];
    loop {
        let n = stream.read(&mut chunk).map_err(|err| CdpError::Transport {
            message: err.to_string(),
        })?;
        if n == 0 {
            return Err(CdpError::Transport {
                message: "HTTP response ended before headers".into(),
            });
        }
        buf.extend_from_slice(&chunk[..n]);
        if let Some(split) = buf.windows(4).position(|window| window == b"\r\n\r\n") {
            let headers = String::from_utf8_lossy(&buf[..split]).into_owned();
            let body = buf[split + 4..].to_vec();
            return Ok((headers, body));
        }
        if buf.len() > 65_536 {
            return Err(CdpError::Transport {
                message: "HTTP headers are too large".into(),
            });
        }
    }
}

fn content_length(headers_lower: &str) -> Option<usize> {
    for line in headers_lower.lines() {
        let Some(value) = line.strip_prefix("content-length:") else {
            continue;
        };
        return value.trim().parse().ok();
    }
    None
}

fn decode_chunked(body: &str) -> Result<String, CdpError> {
    let mut rest = body;
    let mut out = String::new();
    loop {
        let (size_line, after) = rest.split_once("\r\n").ok_or_else(|| CdpError::Transport {
            message: "truncated chunked body".into(),
        })?;
        let size =
            usize::from_str_radix(size_line.trim(), 16).map_err(|err| CdpError::Transport {
                message: err.to_string(),
            })?;
        if size == 0 {
            return Ok(out);
        }
        if after.len() < size {
            return Err(CdpError::Transport {
                message: "short HTTP chunk".into(),
            });
        }
        out.push_str(&after[..size]);
        rest = &after[size..];
        if let Some(stripped) = rest.strip_prefix("\r\n") {
            rest = stripped;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn http_endpoint_resolves_to_the_first_page_target_not_the_browser() {
        // Shape of Chrome's `/json/list`: service workers and iframes can come
        // before the page, and the browser target is not listed at all.
        let body = r#"[
            {"type": "service_worker", "webSocketDebuggerUrl": "ws://127.0.0.1:9333/devtools/page/SW"},
            {"type": "page", "webSocketDebuggerUrl": "ws://127.0.0.1:9333/devtools/page/A"},
            {"type": "page", "webSocketDebuggerUrl": "ws://127.0.0.1:9333/devtools/page/B"}
        ]"#;
        assert_eq!(
            first_page_websocket_url(body),
            Ok("ws://127.0.0.1:9333/devtools/page/A".to_owned())
        );
    }

    #[test]
    fn json_list_without_a_page_target_is_a_transport_error() {
        let body = r#"[{"type": "service_worker", "webSocketDebuggerUrl": "ws://x/sw"}, {"type": "page"}]"#;
        assert_eq!(
            first_page_websocket_url(body),
            Err(CdpError::Transport {
                message: "json/list has no page target with a webSocketDebuggerUrl".into(),
            })
        );
        assert!(matches!(
            first_page_websocket_url("{}"),
            Err(CdpError::Transport { .. })
        ));
        assert!(matches!(
            first_page_websocket_url("not json"),
            Err(CdpError::BadJson { .. })
        ));
    }

    #[test]
    fn refused_endpoints_are_transport_errors_without_a_socket() {
        let err = match WebSocketTransport::connect("wss://127.0.0.1/devtools/browser") {
            Err(err) => err,
            Ok(_) => panic!("wss must not open a socket"),
        };
        assert_eq!(
            err,
            CdpError::Transport {
                message: "wss is not supported; use a local ws:// debugging port".into(),
            }
        );
        assert_eq!(
            err.to_string(),
            "CDP transport: wss is not supported; use a local ws:// debugging port"
        );

        let err = match WebSocketTransport::connect("ftp://127.0.0.1/json") {
            Err(err) => err,
            Ok(_) => panic!("ftp must not open a socket"),
        };
        assert_eq!(
            err,
            CdpError::Transport {
                message: "CDP endpoint `ftp://127.0.0.1/json` must be http:// or ws://".into(),
            }
        );
    }

    #[test]
    fn json_version_resolves_browser_websocket_url() {
        assert_eq!(
            version_websocket_url(
                r#"{"Browser":"Chrome","webSocketDebuggerUrl":"ws://127.0.0.1:9333/devtools/browser/BROWSER"}"#
            ),
            Ok("ws://127.0.0.1:9333/devtools/browser/BROWSER".to_owned())
        );
    }

    #[test]
    fn json_version_without_websocket_url_is_a_transport_error() {
        assert_eq!(
            version_websocket_url(r#"{"Browser":"Chrome"}"#),
            Err(CdpError::Transport {
                message: "json/version has no webSocketDebuggerUrl".into(),
            })
        );
    }

    #[test]
    fn page_socket_url_uses_http_endpoint_host_and_target_id() {
        assert_eq!(
            page_socket_url("http://127.0.0.1:9333", "TARGET"),
            Ok("ws://127.0.0.1:9333/devtools/page/TARGET".to_owned())
        );
    }

    #[test]
    fn open_tab_refuses_non_http_endpoints_without_a_socket() {
        for endpoint in [
            "ws://127.0.0.1:9333/devtools/page/TARGET",
            "ftp://127.0.0.1",
        ] {
            let error = match open_tab(endpoint) {
                Err(error) => error,
                Ok(_) => panic!("open_tab must refuse non-http endpoints"),
            };
            assert_eq!(
                error,
                CdpError::Transport {
                    message: "open_tab needs the http:// debugging endpoint".into(),
                }
            );
        }
    }

    #[test]
    fn deep_result_tree_passes_the_envelope_decode_unparsed() {
        // DOM.getDocument on a real page nests past serde_json's 128-level
        // Value cap (~2 JSON levels per DOM level): the transport must hand
        // `result` to the caller's deep parser as raw text instead of dying
        // in the envelope decode.
        let mut tree = String::from("null");
        for _ in 0..150 {
            tree = format!("{{\"children\":[{tree}]}}");
        }
        let result_text = format!("{{\"root\":{tree}}}");
        assert!(
            serde_json::from_str::<Value>(&result_text).is_err(),
            "fixture must exceed the Value depth cap"
        );
        let frame = frame_fields(&format!("{{\"id\":7,\"result\":{result_text}}}"))
            .unwrap()
            .unwrap();
        assert_eq!(
            frame
                .get("id")
                .and_then(|raw| raw.get().parse::<i64>().ok()),
            Some(7)
        );
        assert_eq!(
            frame.get("result").map(|raw| raw.get()),
            Some(result_text.as_str())
        );
    }

    #[test]
    fn foreign_frames_and_deep_event_params_do_not_break_decode() {
        // Valid JSON that is not an object is not a CDP message: skipped,
        // like any frame that does not carry our response id.
        assert!(frame_fields("[1,2]").unwrap().is_none());
        assert!(frame_fields("\"text\"").unwrap().is_none());
        assert!(frame_fields("null").unwrap().is_none());
        assert!(matches!(
            frame_fields("{nope"),
            Err(CdpError::BadJson { .. })
        ));
        // An event whose params overflow the Value cap is undeliverable;
        // the frame still decodes so the reader can drop just that event
        // instead of failing the in-flight call.
        let mut deep = String::from("null");
        for _ in 0..140 {
            deep = format!("{{\"a\":{deep}}}");
        }
        let frame = frame_fields(&format!("{{\"method\":\"X.y\",\"params\":{deep}}}"))
            .unwrap()
            .unwrap();
        assert!(!frame.contains_key("id"));
        assert_eq!(
            frame
                .get("method")
                .and_then(|raw| raw_string(raw))
                .as_deref(),
            Some("X.y")
        );
        assert!(event_params(&frame).is_none());
    }

    #[test]
    fn connect_socket_fails_fast_when_the_peer_never_answers() {
        // A listener that accepts TCP but never completes the ws upgrade must
        // not stall the CLI: `tungstenite::connect` applies no deadline until
        // it returns.
        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind");
        let port = listener.local_addr().expect("addr").port();
        std::thread::spawn(move || {
            let _held = listener.accept();
            std::thread::sleep(Duration::from_secs(30));
        });
        let started = Instant::now();
        let result = connect_socket(&format!("ws://127.0.0.1:{port}/devtools/page/X"));
        assert!(result.is_err());
        assert!(
            started.elapsed() < Duration::from_secs(20),
            "handshake took {:?}",
            started.elapsed()
        );
    }

    /// A peer that answers the focus-emulation call `connect` makes, then
    /// runs `script` on the next frames it reads (one request id each).
    fn fake_page(script: fn(&mut WebSocket<TcpStream>, i64)) -> String {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind");
        let port = listener.local_addr().expect("addr").port();
        std::thread::spawn(move || {
            let (stream, _) = listener.accept().expect("accept");
            let mut socket = tungstenite::accept(stream).expect("ws upgrade");
            let mut answered_connect = false;
            while let Ok(message) = socket.read() {
                let Message::Text(text) = message else {
                    continue;
                };
                let id = serde_json::from_str::<Value>(text.as_ref()).expect("json")["id"]
                    .as_i64()
                    .expect("id");
                if !answered_connect {
                    answered_connect = true;
                    let reply = json!({"id": id, "result": {}}).to_string();
                    socket.send(Message::Text(reply.into())).expect("send");
                    continue;
                }
                script(&mut socket, id);
            }
        });
        format!("ws://127.0.0.1:{port}/devtools/page/X")
    }

    #[test]
    fn a_dialog_opening_ends_the_wait_and_stays_buffered() {
        // Chrome does not answer the call that opened an alert until the
        // alert closes; the opening event arrives while the call waits.
        // The press call sends the event and no reply (the reply comes
        // late, before the next call's own reply).
        let url = fake_page(|socket, id| {
            let send = |socket: &mut WebSocket<TcpStream>, value: Value| {
                socket
                    .send(Message::Text(value.to_string().into()))
                    .expect("send");
            };
            if id == 2 {
                send(
                    socket,
                    json!({"method": "Page.javascriptDialogOpening",
                           "params": {"type": "alert", "message": "hi", "url": "http://x/"}}),
                );
            } else {
                // The abandoned call's late reply, then this call's own.
                send(socket, json!({"id": 2, "result": {"late": true}}));
                send(
                    socket,
                    json!({"method": "Page.javascriptDialogClosed", "params": {"result": true}}),
                );
                send(socket, json!({"id": id, "result": {"handled": id}}));
            }
        });
        let mut transport = WebSocketTransport::connect(&url).expect("connect");
        let started = Instant::now();
        let err = transport
            .call("Runtime.callFunctionOn", "{}")
            .expect_err("the dialog interrupts the wait");
        assert_eq!(
            err,
            CdpError::DialogOpened {
                method: "Runtime.callFunctionOn".into()
            }
        );
        assert!(
            started.elapsed() < Duration::from_secs(3),
            "returned after {:?}",
            started.elapsed()
        );
        let events = transport.drain_events();
        assert_eq!(events.len(), 1, "{events:?}");
        assert_eq!(events[0].method, "Page.javascriptDialogOpening");
        assert_eq!(events[0].params["type"], "alert");
        // The late reply to id 2 is not mistaken for the next call's.
        let body = transport
            .call("Page.handleJavaScriptDialog", r#"{"accept":true}"#)
            .expect("next call");
        assert_eq!(body, r#"{"handled":3}"#);
        let events = transport.drain_events();
        assert_eq!(events.len(), 1, "{events:?}");
        assert_eq!(events[0].method, "Page.javascriptDialogClosed");
    }
}
