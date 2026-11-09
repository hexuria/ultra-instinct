//! The secret list behind every tool's masking (`ToolState`): what is
//! remembered, for how long, how short secrets match, and which binary
//! fields alone are exempt.

use serde_json::json;
use ultra_instinct_browser_tools::{ToolError, ToolState, MASK, MAX_SENSITIVE_SECRETS};

#[test]
fn sensitive_text_of_any_length_is_kept_and_short_plain_text_is_not() {
    let mut state = ToolState::default();
    state.remember_typed("7", true).unwrap();
    state.remember_typed("abc", false).unwrap();
    state.remember_typed("   ", true).unwrap();
    assert_eq!(
        state.scrub_text("pin 7, word abc"),
        format!("pin {MASK}, word abc")
    );
    state.remember_typed("rust", false).unwrap();
    assert_eq!(state.scrub_text("trusty rust"), format!("t{MASK}y {MASK}"));
}

#[test]
fn a_short_secret_is_masked_only_as_a_whole_token() {
    let mut state = ToolState::default();
    state.remember_typed("123", true).unwrap();
    assert_eq!(
        state.scrub_text("cvv=123&id=41123;123x;(123)"),
        format!("cvv={MASK}&id=41123;123x;({MASK})")
    );
    assert_eq!(state.scrub_text("123"), MASK);
}

#[test]
fn sensitive_entries_are_never_evicted_and_the_list_refuses_when_full() {
    let mut state = ToolState::default();
    state.remember_typed("first-secret", true).unwrap();
    for n in 0..200 {
        state
            .remember_typed(&format!("plain text {n}"), false)
            .unwrap();
    }
    for n in 1..MAX_SENSITIVE_SECRETS {
        state.remember_typed(&format!("secret-{n}"), true).unwrap();
    }
    assert_eq!(
        state.remember_typed("one-too-many", true),
        Err(ToolError::SecretsFull {
            limit: MAX_SENSITIVE_SECRETS
        })
    );
    // A value already kept is no new entry.
    state.remember_typed("secret-1", true).unwrap();
    assert_eq!(state.scrub_text("first-secret"), MASK);
    assert_eq!(state.scrub_text("one-too-many"), "one-too-many");
    // The newest 32 plain entries stay, the oldest went.
    assert_eq!(state.scrub_text("plain text 199"), MASK);
    assert_eq!(state.scrub_text("plain text 0"), "plain text 0");
    assert_eq!(
        ToolError::SecretsFull { limit: 64 }.to_value()["code"],
        "secrets_full"
    );
}

#[test]
fn only_the_screenshot_and_pdf_binary_fields_are_exempt() {
    let mut state = ToolState::default();
    state.remember_typed("hunter2", true).unwrap();
    let body = json!({"base64": "hunter2", "data": "hunter2", "format": "png", "note": "hunter2"});
    let shot = state
        .scrub_result("browser_screenshot", Ok(body.clone()))
        .unwrap();
    assert_eq!(shot["base64"], "hunter2");
    assert_eq!(shot["data"], MASK);
    assert_eq!(shot["note"], MASK);
    let pdf = state
        .scrub_result("browser_save_as_pdf", Ok(body.clone()))
        .unwrap();
    assert_eq!(pdf["data"], "hunter2");
    assert_eq!(pdf["base64"], MASK);
    let exec = state
        .scrub_result("browser_exec", Ok(json!({"result": body})))
        .unwrap();
    assert!(!exec.to_string().contains("hunter2"), "{exec}");
    let err = state
        .scrub_result("observe", Err(ToolError::Browser("saw hunter2".into())))
        .unwrap_err();
    assert_eq!(err, ToolError::Browser(format!("saw {MASK}")));
}

/// A page that sends a typed secret in a URL sends it encoded: percent
/// encoding (either hex case, `%20` or `+` for a space) and, in HTML, its
/// entity-escaped form are masked like the literal text, in a get_state url
/// line and in a HAR entry's url and queryString alike.
#[test]
fn url_and_html_encoded_forms_of_a_secret_are_masked() {
    let mut state = ToolState::default();
    state.remember_typed("p@ss word", true).unwrap();
    for encoded in ["p%40ss+word", "p%40ss%20word", "p@ss+word", "p%40ss word"] {
        let line = format!("url: https://shop.example/login?pw={encoded}&next=1");
        assert_eq!(
            state.scrub_text(&line),
            format!("url: https://shop.example/login?pw={MASK}&next=1"),
            "{encoded}"
        );
    }
    state.remember_typed("<a&b>\"c'", true).unwrap();
    assert_eq!(
        state.scrub_text("<input value=\"&lt;a&amp;b&gt;&quot;c&#39;\">"),
        format!("<input value=\"{MASK}\">")
    );
    assert_eq!(
        state.scrub_text("q=%3ca%26b%3E%22c%27"),
        format!("q={MASK}")
    );

    let event = ultra_instinct_cdp::CdpEvent {
        method: "Network.requestWillBeSent".into(),
        params: json!({"requestId": "r1", "timestamp": 1.0, "wallTime": 1.0,
            "request": {"url": "https://shop.example/a?pw=p%40ss+word&b=p%40ss%20word",
                        "method": "GET", "headers": {}}}),
    };
    let mut har = ultra_instinct_cdp::har::build_har(&[event]);
    assert!(har.to_string().contains("p%40ss"), "{har}");
    let har = state
        .scrub_result("browser_har", Ok(std::mem::take(&mut har)))
        .unwrap();
    let text = har.to_string();
    assert!(!text.contains("p%40ss") && !text.contains("word"), "{text}");
    assert!(text.contains(MASK), "{text}");
}

/// A short secret's encoded forms keep the whole-token rule.
#[test]
fn an_encoded_short_secret_is_masked_only_as_a_whole_token() {
    let mut state = ToolState::default();
    state.remember_typed("1 2", true).unwrap();
    assert_eq!(
        state.scrub_text("pin=1+2&x=31%202&y=1%2023&z=%201%202%20"),
        format!("pin={MASK}&x=31%202&y=1%2023&z=%20{MASK}%20")
    );
}
