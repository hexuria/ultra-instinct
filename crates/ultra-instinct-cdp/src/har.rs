//! HAR 1.2 from buffered CDP `Network.*` events (`browser_network`
//! `har_stop`). Bodies are left out; cookie and authorization header values
//! are replaced by `•••`. Pure: the caller records the events and masks any
//! typed secrets in the result.

use std::collections::BTreeMap;

use serde_json::{json, Value};

use crate::CdpEvent;

/// Header names whose values never reach a HAR file.
const SECRET_HEADERS: [&str; 4] = [
    "cookie",
    "set-cookie",
    "authorization",
    "proxy-authorization",
];
const REDACTED: &str = "•••";

/// One request in flight while the HAR is assembled.
#[derive(Default)]
struct Pending {
    request: Value,
    wall_time: Option<f64>,
    start: Option<f64>,
    response: Option<Value>,
    response_at: Option<f64>,
    finished_at: Option<f64>,
    encoded_length: Option<f64>,
    error: Option<String>,
    resource_type: Option<String>,
    /// Headers from `requestWillBeSentExtraInfo` (the wire set, with `Cookie`).
    extra_request_headers: Option<Value>,
    /// Headers from `responseReceivedExtraInfo` (the wire set, with `Set-Cookie`).
    extra_response_headers: Option<Value>,
}

/// HAR 1.2 from buffered `Network.*` events. Bodies are left out; cookie
/// and authorization header values are replaced by `•••`.
pub fn build_har(events: &[CdpEvent]) -> Value {
    let mut done: Vec<Pending> = Vec::new();
    let mut open: BTreeMap<String, usize> = BTreeMap::new();
    // Every hop of a requestId (redirects reuse it), and the ExtraInfo
    // header sets in arrival order. Chrome may send an ExtraInfo event
    // before or after its main event, so they are paired by position once
    // all events are read: the n-th request / response ExtraInfo of an id
    // belongs to its n-th hop.
    let mut hops: BTreeMap<String, Vec<usize>> = BTreeMap::new();
    let mut request_extra: BTreeMap<String, Vec<Value>> = BTreeMap::new();
    let mut response_extra: BTreeMap<String, Vec<Value>> = BTreeMap::new();
    for event in events {
        let params = &event.params;
        let Some(id) = params.get("requestId").and_then(Value::as_str) else {
            continue;
        };
        let timestamp = params.get("timestamp").and_then(Value::as_f64);
        match event.method.as_str() {
            "Network.requestWillBeSent" => {
                if let Some(&slot) = open.get(id) {
                    // A redirect reuses the requestId: the earlier hop ends
                    // with the redirect response.
                    if let Some(redirect) = params.get("redirectResponse") {
                        let hop = &mut done[slot];
                        hop.response = Some(redirect.clone());
                        hop.response_at = timestamp;
                        hop.finished_at = timestamp;
                    }
                }
                done.push(Pending {
                    request: params.get("request").cloned().unwrap_or(Value::Null),
                    wall_time: params.get("wallTime").and_then(Value::as_f64),
                    start: timestamp,
                    resource_type: params
                        .get("type")
                        .and_then(Value::as_str)
                        .map(str::to_owned),
                    ..Pending::default()
                });
                open.insert(id.to_owned(), done.len() - 1);
                hops.entry(id.to_owned()).or_default().push(done.len() - 1);
            }
            "Network.requestWillBeSentExtraInfo" => {
                let headers = params.get("headers").cloned().unwrap_or(Value::Null);
                request_extra
                    .entry(id.to_owned())
                    .or_default()
                    .push(headers);
            }
            "Network.responseReceivedExtraInfo" => {
                let headers = params.get("headers").cloned().unwrap_or(Value::Null);
                response_extra
                    .entry(id.to_owned())
                    .or_default()
                    .push(headers);
            }
            "Network.responseReceived" => {
                if let Some(&slot) = open.get(id) {
                    done[slot].response = params.get("response").cloned();
                    done[slot].response_at = timestamp;
                }
            }
            "Network.loadingFinished" => {
                if let Some(&slot) = open.get(id) {
                    done[slot].finished_at = timestamp;
                    done[slot].encoded_length =
                        params.get("encodedDataLength").and_then(Value::as_f64);
                }
            }
            "Network.loadingFailed" => {
                if let Some(&slot) = open.get(id) {
                    done[slot].finished_at = timestamp;
                    done[slot].error = Some(
                        params
                            .get("errorText")
                            .and_then(Value::as_str)
                            .unwrap_or("failed")
                            .to_owned(),
                    );
                }
            }
            _ => {}
        }
    }
    for (id, slots) in &hops {
        let requests = request_extra.remove(id).unwrap_or_default();
        let responses = response_extra.remove(id).unwrap_or_default();
        for (&slot, headers) in slots.iter().zip(requests) {
            done[slot].extra_request_headers = Some(headers);
        }
        for (&slot, headers) in slots.iter().zip(responses) {
            done[slot].extra_response_headers = Some(headers);
        }
    }
    let entries: Vec<Value> = done.iter().map(har_entry).collect();
    json!({
        "log": {
            "version": "1.2",
            "creator": {"name": "ultra-instinct", "version": env!("CARGO_PKG_VERSION")},
            "pages": [],
            "entries": entries,
        }
    })
}

fn har_entry(p: &Pending) -> Value {
    let ms = |from: Option<f64>, to: Option<f64>| match (from, to) {
        (Some(a), Some(b)) if b >= a => (b - a) * 1000.0,
        _ => 0.0,
    };
    let wait = ms(p.start, p.response_at);
    let receive = ms(p.response_at.or(p.start), p.finished_at);
    let url = p.request.get("url").and_then(Value::as_str).unwrap_or("");
    let response = p.response.as_ref().unwrap_or(&Value::Null);
    let http_version = http_version(response.get("protocol").and_then(Value::as_str));
    let status = response.get("status").and_then(Value::as_i64).unwrap_or(0);
    let response_headers = har_headers(
        with_wire_headers(response.get("headers"), p.extra_response_headers.as_ref()).as_ref(),
    );
    let request_headers = har_headers(
        with_wire_headers(p.request.get("headers"), p.extra_request_headers.as_ref()).as_ref(),
    );
    let redirect_url = response_headers
        .iter()
        .find(|h| {
            h["name"]
                .as_str()
                .is_some_and(|n| n.eq_ignore_ascii_case("location"))
        })
        .and_then(|h| h["value"].as_str())
        .unwrap_or("")
        .to_owned();
    let mut entry = json!({
        "startedDateTime": iso_8601(p.wall_time.unwrap_or(0.0)),
        "time": wait + receive,
        "request": {
            "method": p.request.get("method").and_then(Value::as_str).unwrap_or("GET"),
            "url": url,
            "httpVersion": http_version,
            "cookies": [],
            "headers": request_headers,
            "queryString": query_string(url),
            "headersSize": -1,
            "bodySize": -1,
        },
        "response": {
            "status": status,
            "statusText": response.get("statusText").and_then(Value::as_str).unwrap_or(""),
            "httpVersion": http_version,
            "cookies": [],
            "headers": response_headers,
            "content": {
                "size": 0,
                "mimeType": response.get("mimeType").and_then(Value::as_str).unwrap_or(""),
                "comment": "body not recorded",
            },
            "redirectURL": redirect_url,
            "headersSize": -1,
            "bodySize": -1,
        },
        "cache": {},
        "timings": {"send": 0.0, "wait": wait, "receive": receive},
    });
    if let Some(ip) = response.get("remoteIPAddress").and_then(Value::as_str) {
        entry["serverIPAddress"] = json!(ip);
    }
    if let Some(kind) = &p.resource_type {
        entry["_resourceType"] = json!(kind);
    }
    if let Some(len) = p.encoded_length {
        entry["response"]["_transferSize"] = json!(len);
    }
    if let Some(error) = &p.error {
        entry["response"]["_error"] = json!(error);
    }
    entry
}

fn http_version(protocol: Option<&str>) -> String {
    match protocol {
        None | Some("") => "HTTP/1.1".to_owned(),
        Some(p) if p.to_ascii_lowercase().starts_with("http/") => p.to_ascii_uppercase(),
        Some(p) => p.to_owned(),
    }
}

/// The main event's headers plus each ExtraInfo header whose name
/// (case-insensitive) the main event lacks, such as `Cookie` and
/// `Set-Cookie`. A main-event header is never replaced.
fn with_wire_headers(main: Option<&Value>, wire: Option<&Value>) -> Option<Value> {
    let Some(Value::Object(wire)) = wire else {
        return main.cloned();
    };
    let mut merged = match main {
        Some(Value::Object(map)) => map.clone(),
        _ => serde_json::Map::new(),
    };
    for (name, value) in wire {
        if !merged.keys().any(|k| k.eq_ignore_ascii_case(name)) {
            merged.insert(name.clone(), value.clone());
        }
    }
    Some(Value::Object(merged))
}

/// CDP `{name: value}` headers as HAR `[{name, value}]`, one entry per line
/// of a folded value, secrets replaced.
fn har_headers(headers: Option<&Value>) -> Vec<Value> {
    let Some(Value::Object(map)) = headers else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for (name, value) in map {
        let secret = SECRET_HEADERS.iter().any(|s| name.eq_ignore_ascii_case(s));
        let text = match value {
            Value::String(s) => s.clone(),
            other => other.to_string(),
        };
        for line in text.split('\n') {
            let value = if secret { REDACTED } else { line };
            out.push(json!({"name": name, "value": value}));
        }
    }
    out
}

fn query_string(url: &str) -> Vec<Value> {
    let Some((_, rest)) = url.split_once('?') else {
        return Vec::new();
    };
    let query = rest.split('#').next().unwrap_or("");
    query
        .split('&')
        .filter(|pair| !pair.is_empty())
        .map(|pair| {
            let (name, value) = pair.split_once('=').unwrap_or((pair, ""));
            json!({"name": name, "value": value})
        })
        .collect()
}

/// `YYYY-MM-DDTHH:MM:SS.mmmZ` for epoch seconds (UTC).
fn iso_8601(epoch_secs: f64) -> String {
    let total_ms = (epoch_secs.max(0.0) * 1000.0).round() as i64;
    let (secs, millis) = (total_ms.div_euclid(1000), total_ms.rem_euclid(1000));
    let (days, day_secs) = (secs.div_euclid(86_400), secs.rem_euclid(86_400));
    // Civil date from days since 1970-01-01 (Howard Hinnant's algorithm).
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}.{millis:03}Z",
        day_secs / 3600,
        (day_secs / 60) % 60,
        day_secs % 60
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn iso_8601_formats_utc_with_millis() {
        assert_eq!(iso_8601(0.0), "1970-01-01T00:00:00.000Z");
        assert_eq!(iso_8601(1_700_000_000.123), "2023-11-14T22:13:20.123Z");
        assert_eq!(iso_8601(951_782_400.0), "2000-02-29T00:00:00.000Z");
    }

    fn ev(method: &str, params: Value) -> CdpEvent {
        CdpEvent {
            method: method.to_owned(),
            params,
        }
    }

    fn har_of(events: &[CdpEvent]) -> Value {
        build_har(events)
    }

    fn values(headers: &Value, name: &str) -> Vec<String> {
        headers
            .as_array()
            .unwrap()
            .iter()
            .filter(|h| h["name"].as_str().unwrap().eq_ignore_ascii_case(name))
            .map(|h| h["value"].as_str().unwrap().to_owned())
            .collect()
    }

    #[test]
    fn extra_info_cookie_headers_are_listed_masked() {
        let events = [
            // Chrome may send the request ExtraInfo before its main event.
            ev(
                "Network.requestWillBeSentExtraInfo",
                json!({"requestId": "r1", "headers": {
                    "cookie": "sid=wire-secret", "accept": "*/*", "host": "a.test"
                }}),
            ),
            ev(
                "Network.requestWillBeSent",
                json!({"requestId": "r1", "timestamp": 1.0, "wallTime": 1.0,
                    "request": {"url": "https://a.test/", "method": "GET",
                        "headers": {"Accept": "text/html"}}}),
            ),
            ev(
                "Network.responseReceived",
                json!({"requestId": "r1", "timestamp": 1.1,
                    "response": {"status": 200, "headers": {"Content-Type": "text/html"}}}),
            ),
            // ... and the response ExtraInfo after its main event.
            ev(
                "Network.responseReceivedExtraInfo",
                json!({"requestId": "r1", "headers": {
                    "set-cookie": "sid=new-secret\ntheme=dark", "content-type": "text/html"
                }}),
            ),
        ];
        let har = har_of(&events);
        let text = har.to_string();
        assert!(
            !text.contains("wire-secret")
                && !text.contains("new-secret")
                && !text.contains("theme=dark")
        );
        let entry = &har["log"]["entries"][0];
        let request = &entry["request"]["headers"];
        assert_eq!(values(request, "cookie"), ["•••"]);
        // The main event's header wins; the wire duplicate is not added.
        assert_eq!(values(request, "accept"), ["text/html"]);
        assert_eq!(values(request, "host"), ["a.test"]);
        let response = &entry["response"]["headers"];
        assert_eq!(values(response, "set-cookie"), ["•••", "•••"]);
        assert_eq!(values(response, "content-type"), ["text/html"]);
    }

    #[test]
    fn extra_info_pairs_with_redirect_hops_in_order() {
        let req = |url: &str, ts: f64, redirect: Option<Value>| {
            let mut params = json!({"requestId": "r", "timestamp": ts, "wallTime": ts,
                "request": {"url": url, "method": "GET", "headers": {}}});
            if let Some(r) = redirect {
                params["redirectResponse"] = r;
            }
            ev("Network.requestWillBeSent", params)
        };
        let req_extra = |cookie: &str| {
            ev(
                "Network.requestWillBeSentExtraInfo",
                json!({"requestId": "r", "headers": {"cookie": cookie, "x-hop": cookie}}),
            )
        };
        let resp_extra = |hop: &str| {
            ev(
                "Network.responseReceivedExtraInfo",
                json!({"requestId": "r", "headers": {"set-cookie": "s=1", "x-hop": hop}}),
            )
        };
        let events = [
            req("https://a.test/old", 1.0, None),
            req_extra("one"),
            resp_extra("one"),
            req(
                "https://a.test/new",
                2.0,
                Some(json!({"status": 302, "headers": {"Location": "/new"}})),
            ),
            req_extra("two"),
            ev(
                "Network.responseReceived",
                json!({"requestId": "r", "timestamp": 2.5,
                    "response": {"status": 200, "headers": {}}}),
            ),
            resp_extra("two"),
        ];
        let har = har_of(&events);
        let entries = har["log"]["entries"].as_array().unwrap();
        assert_eq!(entries.len(), 2);
        for (entry, hop) in entries.iter().zip(["one", "two"]) {
            assert_eq!(values(&entry["request"]["headers"], "x-hop"), [hop]);
            assert_eq!(values(&entry["request"]["headers"], "cookie"), ["•••"]);
            assert_eq!(values(&entry["response"]["headers"], "x-hop"), [hop]);
            assert_eq!(values(&entry["response"]["headers"], "set-cookie"), ["•••"]);
        }
        assert_eq!(entries[0]["response"]["status"], 302);
        assert_eq!(entries[0]["response"]["redirectURL"], "/new");
    }

    #[test]
    fn query_string_splits_pairs_without_decoding() {
        let qs = query_string("https://a.test/p?x=1&y=%20&flag#frag");
        assert_eq!(
            qs,
            vec![
                json!({"name": "x", "value": "1"}),
                json!({"name": "y", "value": "%20"}),
                json!({"name": "flag", "value": ""}),
            ]
        );
    }
}
