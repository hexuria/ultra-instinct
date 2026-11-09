//! Pins the `code` of every `ToolError` (the table in docs/embedding.md):
//! one stable snake_case code per variant, in declaration order, on every
//! wire value next to the `variant` it always had.

use serde_json::{json, Value};
use ultra_instinct_browser_tools::{ToolError, ERROR_CODES};

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
    // Old variants keep their exact old body, plus `code`.
    assert_eq!(
        ToolError::InvalidArguments(s("m")).to_value(),
        json!({"code": "invalid_arguments", "variant": "InvalidArguments", "message": "m"})
    );
    assert_eq!(
        ToolError::MissingFixture.to_value(),
        json!({"code": "missing_fixture", "variant": "MissingFixture"})
    );
}
