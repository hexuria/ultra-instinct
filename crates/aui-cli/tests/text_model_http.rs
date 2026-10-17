//! `HttpTextModel` wire contracts against a local HTTP server: request path,
//! auth headers, and body per API, and each provider's reply mapped onto the
//! `parse_command_reply` contract. No network, no real keys.
#![cfg(feature = "model-text")]

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::mpsc::{channel, Receiver};
use std::time::Duration;

use aui_cli::http_text::{HttpTextModel, TextApi};
use aui_policy::{TextModel, TextModelError, TextModelReply, TextModelRequest};
use serde_json::{json, Value};

/// One test owns every env arm: sibling tests would race on the shared
/// process environment.
#[test]
fn env_selects_api_and_guards_base_url() {
    const ENV: [&str; 5] = [
        "TEXT_MODEL_API_KEY",
        "TEXT_MODEL_BASE_URL",
        "TEXT_MODEL",
        "TEXT_MODEL_API",
        "TEXT_MODEL_REASONING",
    ];
    let set = |pairs: &[(&str, &str)]| {
        for name in ENV {
            std::env::remove_var(name);
        }
        for (name, value) in pairs {
            std::env::set_var(name, value);
        }
        HttpTextModel::from_env()
    };
    let api = |pairs: &[(&str, &str)]| set(pairs).expect("config").expect("model").api();
    let key = ("TEXT_MODEL_API_KEY", "k");

    // No key, no model call (jev parity).
    assert!(set(&[]).expect("config").is_none());

    // The API follows the exact base URL host unless TEXT_MODEL_API says otherwise.
    assert_eq!(api(&[key]), TextApi::OpenAi);
    assert_eq!(
        api(&[key, ("TEXT_MODEL_BASE_URL", "https://api.anthropic.com")]),
        TextApi::Anthropic
    );
    assert_eq!(
        api(&[
            key,
            (
                "TEXT_MODEL_BASE_URL",
                "https://generativelanguage.googleapis.com/v1beta"
            ),
            ("TEXT_MODEL", "gemini-test"),
        ]),
        TextApi::Gemini
    );
    assert_eq!(
        api(&[
            key,
            (
                "TEXT_MODEL_BASE_URL",
                "https://evil.test/?x=api.anthropic.com"
            )
        ]),
        TextApi::OpenAi
    );
    assert_eq!(
        api(&[
            key,
            ("TEXT_MODEL_BASE_URL", "http://127.0.0.1:9"),
            ("TEXT_MODEL_API", "Anthropic")
        ]),
        TextApi::Anthropic
    );
    assert!(set(&[key, ("TEXT_MODEL_API", "cohere")]).is_err());

    // Gemini has no default model, and the model is spliced into the URL path.
    assert!(set(&[key, ("TEXT_MODEL_API", "gemini")]).is_err());
    for model in ["../files", "a?key=x", "a/b"] {
        assert!(
            set(&[key, ("TEXT_MODEL_API", "gemini"), ("TEXT_MODEL", model)]).is_err(),
            "{model}"
        );
    }

    // The key rides a header: cleartext is refused except for localhost.
    for bad in [
        "http://api.evil.example/v1",
        "ftp://api.deepseek.com",
        "http://127.0.0.1.evil.test",
        "localhost:8000",
    ] {
        assert!(set(&[key, ("TEXT_MODEL_BASE_URL", bad)]).is_err(), "{bad}");
    }
    for ok in [
        "http://localhost:8000",
        "http://127.0.0.1:3000/v1",
        "http://[::1]:8080",
    ] {
        assert!(
            set(&[key, ("TEXT_MODEL_BASE_URL", ok)])
                .expect(ok)
                .is_some(),
            "{ok}"
        );
    }
    set(&[]).expect("cleared");
}

#[test]
fn openai_wire_and_string_fingerprint_echo() {
    let (port, rx) = serve(vec![ok_json(&json!({
        "choices": [{"message": {"content": r#"{"text":"tokyo","context_fingerprint":"42"}"#}}]
    }))]);
    let mut model = model(
        TextApi::OpenAi,
        &format!("http://127.0.0.1:{port}/v1"),
        Some("m"),
        false,
    );
    assert_tokyo(model.complete(&ask()));

    let sent = rx.recv_timeout(Duration::from_secs(5)).expect("request");
    assert!(
        sent.request_line.starts_with("POST /v1/chat/completions "),
        "{}",
        sent.request_line
    );
    assert!(
        sent.headers.contains("authorization: bearer k"),
        "{}",
        sent.headers
    );
    assert_eq!(sent.body["model"], "m");
    assert_eq!(sent.body["reasoning"], json!({"effort": "low"}));
    assert_eq!(
        prompt_fingerprint(sent.body["messages"][1]["content"].as_str()),
        "42"
    );
}

#[test]
fn anthropic_wire_appends_version_and_skips_thinking_blocks() {
    let (port, rx) = serve(vec![ok_json(&anthropic_reply(
        "end_turn",
        r#"{"text":"tokyo","context_fingerprint":"42","declined":""}"#,
    ))]);
    // Host-only base gets /v1; no model means the Haiku default.
    let mut model = model(
        TextApi::Anthropic,
        &format!("http://127.0.0.1:{port}"),
        None,
        false,
    );
    assert_tokyo(model.complete(&ask()));

    let sent = rx.recv_timeout(Duration::from_secs(5)).expect("request");
    assert!(
        sent.request_line.starts_with("POST /v1/messages "),
        "{}",
        sent.request_line
    );
    assert!(sent.headers.contains("x-api-key: k"), "{}", sent.headers);
    assert!(
        sent.headers.contains("anthropic-version: 2023-06-01"),
        "{}",
        sent.headers
    );
    assert!(!sent.headers.contains("authorization:"), "{}", sent.headers);
    assert_eq!(sent.body["model"], "claude-haiku-5-5");
    assert_eq!(sent.body["output_config"]["effort"], "low");
    assert_eq!(sent.body["output_config"]["format"]["type"], "json_schema");
    assert_eq!(
        prompt_fingerprint(sent.body["messages"][0]["content"].as_str()),
        "42"
    );
}

#[test]
fn anthropic_reasoning_none_omits_effort() {
    let (port, rx) = serve(vec![ok_json(&anthropic_reply(
        "end_turn",
        r#"{"text":"tokyo","context_fingerprint":"42","declined":""}"#,
    ))]);
    let mut model = model(
        TextApi::Anthropic,
        &format!("http://127.0.0.1:{port}"),
        None,
        true,
    );
    assert_tokyo(model.complete(&ask()));
    let sent = rx.recv_timeout(Duration::from_secs(5)).expect("request");
    assert!(
        sent.body["output_config"].get("effort").is_none(),
        "{}",
        sent.body
    );
}

#[test]
fn gemini_wire_keeps_key_out_of_url_and_skips_thought_parts() {
    let (port, rx) = serve(vec![ok_json(&json!({
        "candidates": [{
            "finishReason": "STOP",
            "content": {"parts": [
                {"text": "the field is Destination", "thought": true},
                {"text": r#"{"text":"tokyo","context_fingerprint":"42"}"#},
            ]},
        }]
    }))]);
    let mut model = model(
        TextApi::Gemini,
        &format!("http://127.0.0.1:{port}"),
        Some("models/gemini-test"),
        false,
    );
    assert_tokyo(model.complete(&ask()));

    let sent = rx.recv_timeout(Duration::from_secs(5)).expect("request");
    assert!(
        sent.request_line
            .starts_with("POST /v1beta/models/gemini-test:generateContent "),
        "{}",
        sent.request_line
    );
    assert!(
        sent.headers.contains("x-goog-api-key: k"),
        "{}",
        sent.headers
    );
    assert!(!sent.request_line.contains("key="), "{}", sent.request_line);
    assert_eq!(
        sent.body["generationConfig"]["responseMimeType"],
        "application/json"
    );
    assert_eq!(
        prompt_fingerprint(sent.body["contents"][0]["parts"][0]["text"].as_str()),
        "42"
    );
}

/// A provider-side refusal or truncation must never become typed text: it
/// surfaces as Declined / Malformed so the resolver falls back or abstains.
#[test]
fn provider_refusals_and_truncation_are_not_text() {
    let cases = [
        (
            TextApi::Anthropic,
            anthropic_reply(
                "end_turn",
                r#"{"text":"","context_fingerprint":"","declined":"no value in goal"}"#,
            ),
            "declined",
        ),
        (
            TextApi::Anthropic,
            anthropic_reply("refusal", ""),
            "declined",
        ),
        (
            TextApi::Anthropic,
            anthropic_reply("max_tokens", r#"{"text":"tok"#),
            "malformed",
        ),
        (
            TextApi::Gemini,
            json!({"promptFeedback": {"blockReason": "SAFETY"}}),
            "declined",
        ),
        (
            TextApi::Gemini,
            json!({"candidates": [{"finishReason": "MAX_TOKENS", "content": {"parts": [{"text": "{\"te"}]}}]}),
            "malformed",
        ),
    ];
    for (api, reply, expected) in cases {
        let (port, _rx) = serve(vec![ok_json(&reply)]);
        let mut model = model(api, &format!("http://127.0.0.1:{port}"), Some("m"), false);
        let err = model.complete(&ask()).expect_err("must not resolve text");
        let kind = match err {
            TextModelError::Declined(_) => "declined",
            TextModelError::Malformed(_) => "malformed",
            _ => "other",
        };
        assert_eq!(kind, expected, "{api:?} {reply}: {err}");
    }
}

#[test]
fn retries_once_on_retryable_status() {
    let (port, rx) = serve(vec![
        "HTTP/1.1 500 Internal Server Error\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
            .to_owned(),
        ok_json(
            &json!({"choices": [{"message": {"content": r#"{"text":"tokyo","context_fingerprint":42}"#}}]}),
        ),
    ]);
    let mut model = model(
        TextApi::OpenAi,
        &format!("http://127.0.0.1:{port}/v1"),
        Some("m"),
        false,
    );
    assert_tokyo(model.complete(&ask()));
    for _ in 0..2 {
        rx.recv_timeout(Duration::from_secs(5))
            .expect("two attempts");
    }
}

fn model(api: TextApi, base: &str, model: Option<&str>, reasoning_off: bool) -> HttpTextModel {
    HttpTextModel::new(api, Some(base), model, "k".into(), reasoning_off).expect("model")
}

fn ask() -> TextModelRequest {
    TextModelRequest {
        goal: "type tokyo into Destination".into(),
        field_label: "Destination".into(),
        field_role: "text_field".into(),
        context_fingerprint: 42,
        max_chars: 60,
        page_text: None,
    }
}

fn assert_tokyo(reply: Result<TextModelReply, TextModelError>) {
    let reply = reply.expect("complete");
    assert_eq!(reply.text, "tokyo");
    assert_eq!(reply.context_fingerprint, 42);
}

/// The fingerprint travels as a decimal string — the one thing a language
/// model can echo back without corrupting it.
fn prompt_fingerprint(prompt: Option<&str>) -> String {
    let prompt: Value = serde_json::from_str(prompt.expect("prompt text")).expect("prompt json");
    prompt["context_fingerprint"]
        .as_str()
        .expect("string fingerprint")
        .to_owned()
}

fn anthropic_reply(stop_reason: &str, text: &str) -> Value {
    json!({
        "stop_reason": stop_reason,
        "content": [
            {"type": "thinking", "thinking": "", "signature": "s"},
            {"type": "text", "text": text},
        ],
    })
}

fn ok_json(body: &Value) -> String {
    let body = body.to_string();
    format!(
        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    )
}

struct Sent {
    request_line: String,
    /// Lowercased header block.
    headers: String,
    body: Value,
}

/// Serve `responses` in order, one per connection; each request arrives on
/// the returned channel.
fn serve(responses: Vec<String>) -> (u16, Receiver<Sent>) {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
    let port = listener.local_addr().expect("addr").port();
    let (tx, rx) = channel();
    std::thread::spawn(move || {
        for response in responses {
            let (mut stream, _) = listener.accept().expect("accept");
            let sent = read_request(&mut stream);
            let _ = tx.send(sent);
            stream
                .write_all(response.as_bytes())
                .expect("write response");
        }
    });
    (port, rx)
}

fn read_request(stream: &mut TcpStream) -> Sent {
    let mut buf = Vec::new();
    let mut chunk = [0u8; 8192];
    let headers_end = loop {
        let n = stream.read(&mut chunk).expect("read");
        assert!(n > 0, "connection closed mid-headers");
        buf.extend_from_slice(&chunk[..n]);
        if let Some(pos) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
            break pos;
        }
    };
    let head = String::from_utf8_lossy(&buf[..headers_end]).to_string();
    let (request_line, headers) = head.split_once("\r\n").unwrap_or((&head, ""));
    let headers = headers.to_ascii_lowercase();
    let content_length: usize = headers
        .lines()
        .find_map(|line| {
            line.strip_prefix("content-length:")
                .and_then(|v| v.trim().parse().ok())
        })
        .expect("content-length header");
    let mut body = buf[headers_end + 4..].to_vec();
    while body.len() < content_length {
        let n = stream.read(&mut chunk).expect("read");
        assert!(n > 0, "connection closed mid-body");
        body.extend_from_slice(&chunk[..n]);
    }
    Sent {
        request_line: request_line.to_owned(),
        headers,
        body: serde_json::from_slice(&body).expect("json body"),
    }
}
