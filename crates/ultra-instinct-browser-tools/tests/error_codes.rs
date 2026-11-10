//! Pins the `code` of every `ToolError` (the table in docs/embedding.md):
//! one stable snake_case code per variant, in declaration order, on every
//! wire value next to its stable `variant`.

use serde_json::{json, Value};
use ultra_instinct_browser_tools::ToolError;
use ultra_instinct_cdp::CdpError;

/// Every `ToolError::code`, in declaration order (docs/embedding.md). A
/// code never changes meaning once shipped: change this list only by
/// appending.
const ERROR_CODES: [&str; 63] = [
    "missing_fixture",
    "missing_before",
    "missing_after",
    "duplicate_source",
    "missing_tool_name",
    "unknown_tool",
    "goal_not_accepted",
    "coordinates_not_accepted",
    "empty_text",
    "unknown_region",
    "unknown_role",
    "unknown_action",
    "unsupported_action",
    "non_finite_confidence",
    "runner_up_needs_confidence",
    "runner_up_is_target",
    "bad_runner_up",
    "target_not_top",
    "confidence_with_query",
    "snapshot_evicted",
    "unknown_snapshot",
    "bad_snapshot",
    "expect_needs_observe_after",
    "expected_text_missing",
    "region_still_present",
    "bad_confidence",
    "confidence_out_of_range",
    "missing_expect",
    "both_expectations",
    "act_needs_cdp",
    "unknown_executor",
    "not_implemented",
    "browser_use_is_replay",
    "browser_use_rejected",
    "browser_use_script",
    "cua_is_replay",
    "cua_rejected",
    "cua_script",
    "invalid_arguments",
    "ticket_invalid",
    "verify_no_effect",
    "io",
    "fixture",
    "browser",
    "needs_confirm",
    "domain_not_allowed",
    "stale_ref",
    "unknown_ref",
    "snapshot_required",
    "target_gone",
    "not_owned",
    "dialog_open",
    "timeout",
    "pipe_closed",
    "frame_too_large",
    "tool_not_allowed",
    "path_not_absolute",
    "path_not_allowed",
    "downloads_off",
    "output_too_large",
    "secrets_full",
    "secret_on_screen",
    "uploads_off",
];

fn s(text: &str) -> String {
    text.to_owned()
}

/// One value of every variant, in declaration order.
fn every_variant() -> Vec<ToolError> {
    vec![
        ToolError::MissingFixture,
        ToolError::MissingBefore,
        ToolError::MissingAfter,
        ToolError::DuplicateSource,
        ToolError::MissingToolName,
        ToolError::UnknownTool(s("x")),
        ToolError::GoalNotAccepted,
        ToolError::CoordinatesNotAccepted,
        ToolError::EmptyText,
        ToolError::UnknownRegion(s("r")),
        ToolError::UnknownRole(s("r")),
        ToolError::UnknownAction(s("a")),
        ToolError::UnsupportedAction(s("a")),
        ToolError::NonFiniteConfidence,
        ToolError::RunnerUpNeedsConfidence,
        ToolError::RunnerUpIsTarget,
        ToolError::BadRunnerUp(s("m")),
        ToolError::TargetNotTop {
            region: s("a"),
            top: s("b"),
        },
        ToolError::ConfidenceWithQuery,
        ToolError::SnapshotEvicted { id: 1, oldest: 2 },
        ToolError::UnknownSnapshot(3),
        ToolError::BadSnapshot(s("x")),
        ToolError::ExpectNeedsObserveAfter,
        ToolError::ExpectedTextMissing { expected: s("t") },
        ToolError::RegionStillPresent { id: s("r") },
        ToolError::BadConfidence(s("x")),
        ToolError::ConfidenceOutOfRange(s("2")),
        ToolError::MissingExpect,
        ToolError::BothExpectations,
        ToolError::ActNeedsCdp,
        ToolError::UnknownExecutor(s("e")),
        ToolError::NotImplemented { executor: s("e") },
        ToolError::BrowserUseIsReplay,
        ToolError::BrowserUseRejected { message: s("m") },
        ToolError::BrowserUseScript { message: s("m") },
        ToolError::CuaIsReplay,
        ToolError::CuaRejected { message: s("m") },
        ToolError::CuaScript { message: s("m") },
        ToolError::InvalidArguments(s("m")),
        ToolError::TicketInvalid(s("disabled")),
        ToolError::VerifyNoEffect,
        ToolError::Io {
            path: s("/p"),
            message: s("m"),
        },
        ToolError::Fixture(s("m")),
        ToolError::Browser(s("m")),
        ToolError::NeedsConfirm { label: s("Pay") },
        ToolError::DomainNotAllowed { host: s("a.test") },
        ToolError::StaleRef {
            reason: s("target-changed"),
        },
        ToolError::UnknownRef {
            index: 9,
            indexed: 3,
        },
        ToolError::SnapshotRequired,
        ToolError::TargetGone { target_id: s("T1") },
        ToolError::NotOwned { target_id: s("U1") },
        ToolError::DialogOpen {
            kind: s("alert"),
            message: s("Saved"),
            detail: s("d"),
        },
        ToolError::Timeout {
            method: s("X.y"),
            after_ms: 30_000,
        },
        ToolError::PipeClosed {
            message: s("CDP pipe closed: end of stream"),
        },
        ToolError::FrameTooLarge { limit: 64 },
        ToolError::ToolNotAllowed {
            tool: s("browser_exec"),
        },
        ToolError::PathNotAbsolute { path: s("out.png") },
        ToolError::PathNotAllowed {
            path: s("/a/b"),
            dir: s("/d"),
        },
        ToolError::DownloadsOff,
        ToolError::OutputTooLarge { limit: 1, bytes: 2 },
        ToolError::SecretsFull { limit: 64 },
        ToolError::SecretOnScreen {
            message: s("no objectId"),
        },
        ToolError::UploadsOff,
    ]
}

#[test]
fn every_variant_has_its_pinned_code_on_the_wire() {
    let all = every_variant();
    let codes: Vec<&str> = all.iter().map(ToolError::code).collect();
    assert_eq!(codes, ERROR_CODES, "one code per variant, in order");
    let mut distinct = codes.clone();
    distinct.sort_unstable();
    distinct.dedup();
    assert_eq!(distinct.len(), codes.len(), "codes are unique");
    for err in &all {
        let value = err.to_value();
        assert_eq!(value["code"], err.code(), "{err:?}");
        assert_eq!(value["variant"], err.variant(), "{err:?}");
        let code = err.code();
        assert!(
            !code.is_empty() && code.chars().all(|c| c.is_ascii_lowercase() || c == '_'),
            "{code}"
        );
    }
}

/// The codes the embedding doc names, with their fields.
#[test]
fn named_codes_carry_their_fields() {
    let field = |err: ToolError, key: &str| -> Value { err.to_value()[key].clone() };
    assert_eq!(
        field(ToolError::NeedsConfirm { label: s("Pay") }, "label"),
        "Pay"
    );
    assert_eq!(
        field(ToolError::DomainNotAllowed { host: s("a.test") }, "host"),
        "a.test"
    );
    assert_eq!(
        field(ToolError::TargetGone { target_id: s("T1") }, "target_id"),
        "T1"
    );
    assert_eq!(
        field(ToolError::NotOwned { target_id: s("U1") }, "target_id"),
        "U1"
    );
    let dialog = ToolError::DialogOpen {
        kind: s("confirm"),
        message: s("Leave?"),
        detail: s("d"),
    }
    .to_value();
    assert_eq!(
        (&dialog["type"], &dialog["message"]),
        (&json!("confirm"), &json!("Leave?"))
    );
    assert_eq!(
        field(
            ToolError::Timeout {
                method: s("X.y"),
                after_ms: 5
            },
            "method"
        ),
        "X.y"
    );
    assert_eq!(field(ToolError::FrameTooLarge { limit: 64 }, "limit"), 64);
    assert_eq!(
        field(ToolError::ToolNotAllowed { tool: s("t") }, "tool"),
        "t"
    );
    assert_eq!(
        field(ToolError::PathNotAbsolute { path: s("a") }, "path"),
        "a"
    );
    assert_eq!(
        field(ToolError::OutputTooLarge { limit: 7, bytes: 9 }, "limit"),
        7
    );
    // Untyped variants keep their plain body, plus `code`.
    assert_eq!(
        ToolError::InvalidArguments(s("m")).to_value(),
        json!({"code": "invalid_arguments", "variant": "InvalidArguments", "message": "m"})
    );
    assert_eq!(
        ToolError::MissingFixture.to_value(),
        json!({"code": "missing_fixture", "variant": "MissingFixture"})
    );
}

#[test]
fn typed_refusals_keep_their_old_variant_message_and_display() {
    let confirm = ToolError::NeedsConfirm {
        label: "Pay $18".into(),
    };
    assert_eq!(
        confirm.to_value(),
        json!({"code": "needs_confirm", "variant": "TicketInvalid",
               "message": "needs-confirm: \"Pay $18\"", "label": "Pay $18"})
    );
    assert_eq!(
        confirm.to_string(),
        ToolError::TicketInvalid("needs-confirm: \"Pay $18\"".into()).to_string()
    );
    let timeout = ToolError::cdp(&CdpError::Timeout {
        method: "X.y".into(),
        after: std::time::Duration::from_secs(30),
    });
    assert_eq!(timeout.code(), "timeout");
    assert_eq!(
        timeout.to_value()["message"],
        "CDP transport: no CDP result for `X.y` within 30 s"
    );
    // A generic Browser error becomes the typed failure seen in the call.
    assert_eq!(
        ToolError::Browser("x".into()).or_cdp(Some(&CdpError::PipeClosed {
            message: "end of stream".into()
        })),
        ToolError::PipeClosed {
            message: "CDP pipe closed: end of stream".into()
        }
    );
    // A typed refusal is kept whatever CDP saw.
    let stale = ToolError::StaleRef {
        reason: "target-changed".into(),
    };
    assert_eq!(
        stale
            .clone()
            .or_cdp(Some(&CdpError::Timeout {
                method: "X".into(),
                after: std::time::Duration::from_secs(1)
            }))
            .code(),
        "stale_ref"
    );
    assert!(ToolError::from_cdp(&CdpError::NoScriptedResponse { method: "X".into() }).is_none());
    // An attached server's refusal of a tab it did not open has its own code.
    let not_owned = ToolError::cdp(&CdpError::NotOwned {
        target_id: "U1".into(),
    });
    assert_eq!(not_owned.code(), "not_owned");
    assert!(
        not_owned
            .to_string()
            .contains("not a tab this session opened"),
        "{not_owned}"
    );
}

/// An oversize frame costs its call only: the wording says the frame was
/// refused and the pipe stays usable, never that it closed.
#[test]
fn frame_too_large_says_the_pipe_stays_usable() {
    let cdp = CdpError::FrameTooLarge { limit: 64 };
    let tool = ToolError::FrameTooLarge { limit: 64 };
    let texts = [
        cdp.to_string(),
        tool.to_string(),
        tool.to_value()["message"]
            .as_str()
            .unwrap_or_default()
            .to_owned(),
        ToolError::from_cdp(&cdp).expect("typed").to_string(),
    ];
    for text in &texts {
        assert!(!text.contains("closed"), "{text}");
        assert!(text.contains("refused") && text.contains("64"), "{text}");
        assert!(text.contains("stays usable"), "{text}");
    }
}
