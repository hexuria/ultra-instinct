//! Exact failures a tool call can return.
//!
//! [`ToolError::to_value`] is the JSON a client matches on. Every value
//! carries a stable snake_case `code` ([`ToolError::code`]; the table is in
//! docs/embedding.md, pinned by `tests/error_codes.rs`); `variant` and `message` keep their earlier text, so a
//! client that read them still works. Typed refusals that used to be free
//! text (`needs-confirm`, `domain-not-allowed`, a stale ref, CDP pipe and
//! timeout failures) are their own variants and carry their fields; their
//! `variant` stays the one they had (`TicketInvalid`, `Browser`,
//! `InvalidArguments`). A low-confidence act is not one of these: it is a
//! successful tool result with `executed: false`.

use std::fmt;

use serde_json::{json, Value};
use ultra_instinct_cdp::CdpError;

#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum ToolError {
    MissingFixture,
    MissingBefore,
    MissingAfter,
    DuplicateSource,
    MissingToolName,
    UnknownTool(String),
    GoalNotAccepted,
    CoordinatesNotAccepted,
    EmptyText,
    UnknownRegion(String),
    UnknownRole(String),
    UnknownAction(String),
    UnsupportedAction(String),
    NonFiniteConfidence,
    /// `runner_up` was given without `confidence`, which would make the act
    /// look inspected and skip the gate.
    RunnerUpNeedsConfidence,
    /// `runner_up.id` is the region being pressed.
    RunnerUpIsTarget,
    BadRunnerUp(String),
    /// Locate fields on `act` ranked a different region first.
    TargetNotTop {
        region: String,
        top: String,
    },
    /// `act` was given both `confidence` and locate fields.
    ConfidenceWithQuery,
    SnapshotEvicted {
        id: u64,
        oldest: u64,
    },
    UnknownSnapshot(u64),
    BadSnapshot(String),
    /// An expectation was given with `observe_after: false`.
    ExpectNeedsObserveAfter,
    ExpectedTextMissing {
        expected: String,
    },
    RegionStillPresent {
        id: String,
    },
    BadConfidence(String),
    /// A caller-supplied `confidence` or `runner_up.confidence` outside
    /// `[0, 1]`. The value is the JSON text the caller sent.
    ConfidenceOutOfRange(String),
    MissingExpect,
    BothExpectations,
    ActNeedsCdp,
    UnknownExecutor(String),
    NotImplemented {
        executor: String,
    },
    BrowserUseIsReplay,
    BrowserUseRejected {
        message: String,
    },
    BrowserUseScript {
        message: String,
    },
    CuaIsReplay,
    CuaRejected {
        message: String,
    },
    CuaScript {
        message: String,
    },
    InvalidArguments(String),
    /// Allow ticket no longer matches before-snapshot world/target.
    TicketInvalid(String),
    /// Ticketed verify saw an empty before/after delta.
    VerifyNoEffect,
    Io {
        path: String,
        message: String,
    },
    Fixture(String),
    Browser(String),
    /// The user's policy needs a confirm for this press: pass the label back
    /// as `confirm` (that call only). Nothing was pressed.
    NeedsConfirm {
        label: String,
    },
    /// The policy does not allow input on (or navigation to) this host.
    DomainNotAllowed {
        host: String,
    },
    /// The element the caller named is not what it saw any more: it moved,
    /// changed, or left the page, or its neighbourhood changed (`reason`:
    /// `world-changed`, `target-changed`, `target-gone`, `other-tab`).
    /// Nothing was pressed; call `browser_get_state` again.
    StaleRef {
        reason: String,
    },
    /// No interactive element at this index (in the snapshot named, or on
    /// the page now).
    UnknownRef {
        index: usize,
        indexed: usize,
    },
    /// The server requires `snapshot` on index presses (`require_snapshot`).
    SnapshotRequired,
    /// The page target detached or was destroyed (CDP `TargetGone`).
    TargetGone {
        target_id: String,
    },
    /// A JavaScript dialog is open on the tab: the page answers nothing
    /// else until `browser_dialog` handles it. `detail` is the failure text.
    DialogOpen {
        kind: String,
        message: String,
        detail: String,
    },
    /// A CDP call to `method` got no answer within its deadline.
    Timeout {
        method: String,
        after_ms: u64,
    },
    /// The CDP pipe is closed (EOF, read / write error); every later call
    /// fails the same way.
    PipeClosed {
        message: String,
    },
    /// A CDP pipe frame went past the connection's byte cap; the pipe is closed.
    FrameTooLarge {
        limit: usize,
    },
    /// The host's allow-list does not include this tool.
    ToolNotAllowed {
        tool: String,
    },
    /// A file path argument must be absolute.
    PathNotAbsolute {
        path: String,
    },
    /// The host forces downloads into `dir`; `path` is elsewhere.
    PathNotAllowed {
        path: String,
        dir: String,
    },
    /// The host turned downloads off.
    DownloadsOff,
    /// An output went past the host's byte limit.
    OutputTooLarge {
        limit: usize,
        bytes: usize,
    },
    /// The server already masks `limit` values typed into sensitive
    /// fields; a new one is refused before it is typed (none is evicted).
    SecretsFull {
        limit: usize,
    },
}

impl ToolError {
    /// The `variant` field: the Rust variant name, except for typed errors
    /// that kept the variant they were sent as before they had their own
    /// (`TicketInvalid`, `InvalidArguments`, `Browser`).
    pub const fn variant(&self) -> &'static str {
        match self {
            Self::MissingFixture => "MissingFixture",
            Self::MissingBefore => "MissingBefore",
            Self::MissingAfter => "MissingAfter",
            Self::DuplicateSource => "DuplicateSource",
            Self::MissingToolName => "MissingToolName",
            Self::UnknownTool(_) => "UnknownTool",
            Self::GoalNotAccepted => "GoalNotAccepted",
            Self::CoordinatesNotAccepted => "CoordinatesNotAccepted",
            Self::EmptyText => "EmptyText",
            Self::UnknownRegion(_) => "UnknownRegion",
            Self::UnknownRole(_) => "UnknownRole",
            Self::UnknownAction(_) => "UnknownAction",
            Self::UnsupportedAction(_) => "UnsupportedAction",
            Self::NonFiniteConfidence => "NonFiniteConfidence",
            Self::RunnerUpNeedsConfidence => "RunnerUpNeedsConfidence",
            Self::RunnerUpIsTarget => "RunnerUpIsTarget",
            Self::BadRunnerUp(_) => "BadRunnerUp",
            Self::TargetNotTop { .. } => "TargetNotTop",
            Self::ConfidenceWithQuery => "ConfidenceWithQuery",
            Self::SnapshotEvicted { .. } => "SnapshotEvicted",
            Self::UnknownSnapshot(_) => "UnknownSnapshot",
            Self::BadSnapshot(_) => "BadSnapshot",
            Self::ExpectNeedsObserveAfter => "ExpectNeedsObserveAfter",
            Self::ExpectedTextMissing { .. } => "ExpectedTextMissing",
            Self::RegionStillPresent { .. } => "RegionStillPresent",
            Self::BadConfidence(_) => "BadConfidence",
            Self::ConfidenceOutOfRange(_) => "ConfidenceOutOfRange",
            Self::MissingExpect => "MissingExpect",
            Self::BothExpectations => "BothExpectations",
            Self::ActNeedsCdp => "ActNeedsCdp",
            Self::UnknownExecutor(_) => "UnknownExecutor",
            Self::NotImplemented { .. } => "NotImplemented",
            Self::BrowserUseIsReplay => "BrowserUseIsReplay",
            Self::BrowserUseRejected { .. } => "BrowserUseRejected",
            Self::BrowserUseScript { .. } => "BrowserUseScript",
            Self::CuaIsReplay => "CuaIsReplay",
            Self::CuaRejected { .. } => "CuaRejected",
            Self::CuaScript { .. } => "CuaScript",
            Self::InvalidArguments(_) | Self::UnknownRef { .. } => "InvalidArguments",
            Self::TicketInvalid(_)
            | Self::NeedsConfirm { .. }
            | Self::DomainNotAllowed { .. }
            | Self::StaleRef { .. } => "TicketInvalid",
            Self::VerifyNoEffect => "VerifyNoEffect",
            Self::Io { .. } => "Io",
            Self::Fixture(_) => "Fixture",
            Self::Browser(_)
            | Self::TargetGone { .. }
            | Self::DialogOpen { .. }
            | Self::Timeout { .. }
            | Self::PipeClosed { .. }
            | Self::FrameTooLarge { .. } => "Browser",
            Self::SnapshotRequired => "SnapshotRequired",
            Self::ToolNotAllowed { .. } => "ToolNotAllowed",
            Self::PathNotAbsolute { .. } => "PathNotAbsolute",
            Self::PathNotAllowed { .. } => "PathNotAllowed",
            Self::DownloadsOff => "DownloadsOff",
            Self::OutputTooLarge { .. } => "OutputTooLarge",
            Self::SecretsFull { .. } => "SecretsFull",
        }
    }

    /// The stable machine-readable `code` (snake_case), one per variant.
    pub const fn code(&self) -> &'static str {
        match self {
            Self::MissingFixture => "missing_fixture",
            Self::MissingBefore => "missing_before",
            Self::MissingAfter => "missing_after",
            Self::DuplicateSource => "duplicate_source",
            Self::MissingToolName => "missing_tool_name",
            Self::UnknownTool(_) => "unknown_tool",
            Self::GoalNotAccepted => "goal_not_accepted",
            Self::CoordinatesNotAccepted => "coordinates_not_accepted",
            Self::EmptyText => "empty_text",
            Self::UnknownRegion(_) => "unknown_region",
            Self::UnknownRole(_) => "unknown_role",
            Self::UnknownAction(_) => "unknown_action",
            Self::UnsupportedAction(_) => "unsupported_action",
            Self::NonFiniteConfidence => "non_finite_confidence",
            Self::RunnerUpNeedsConfidence => "runner_up_needs_confidence",
            Self::RunnerUpIsTarget => "runner_up_is_target",
            Self::BadRunnerUp(_) => "bad_runner_up",
            Self::TargetNotTop { .. } => "target_not_top",
            Self::ConfidenceWithQuery => "confidence_with_query",
            Self::SnapshotEvicted { .. } => "snapshot_evicted",
            Self::UnknownSnapshot(_) => "unknown_snapshot",
            Self::BadSnapshot(_) => "bad_snapshot",
            Self::ExpectNeedsObserveAfter => "expect_needs_observe_after",
            Self::ExpectedTextMissing { .. } => "expected_text_missing",
            Self::RegionStillPresent { .. } => "region_still_present",
            Self::BadConfidence(_) => "bad_confidence",
            Self::ConfidenceOutOfRange(_) => "confidence_out_of_range",
            Self::MissingExpect => "missing_expect",
            Self::BothExpectations => "both_expectations",
            Self::ActNeedsCdp => "act_needs_cdp",
            Self::UnknownExecutor(_) => "unknown_executor",
            Self::NotImplemented { .. } => "not_implemented",
            Self::BrowserUseIsReplay => "browser_use_is_replay",
            Self::BrowserUseRejected { .. } => "browser_use_rejected",
            Self::BrowserUseScript { .. } => "browser_use_script",
            Self::CuaIsReplay => "cua_is_replay",
            Self::CuaRejected { .. } => "cua_rejected",
            Self::CuaScript { .. } => "cua_script",
            Self::InvalidArguments(_) => "invalid_arguments",
            Self::TicketInvalid(_) => "ticket_invalid",
            Self::VerifyNoEffect => "verify_no_effect",
            Self::Io { .. } => "io",
            Self::Fixture(_) => "fixture",
            Self::Browser(_) => "browser",
            Self::NeedsConfirm { .. } => "needs_confirm",
            Self::DomainNotAllowed { .. } => "domain_not_allowed",
            Self::StaleRef { .. } => "stale_ref",
            Self::UnknownRef { .. } => "unknown_ref",
            Self::SnapshotRequired => "snapshot_required",
            Self::TargetGone { .. } => "target_gone",
            Self::DialogOpen { .. } => "dialog_open",
            Self::Timeout { .. } => "timeout",
            Self::PipeClosed { .. } => "pipe_closed",
            Self::FrameTooLarge { .. } => "frame_too_large",
            Self::ToolNotAllowed { .. } => "tool_not_allowed",
            Self::PathNotAbsolute { .. } => "path_not_absolute",
            Self::PathNotAllowed { .. } => "path_not_allowed",
            Self::DownloadsOff => "downloads_off",
            Self::OutputTooLarge { .. } => "output_too_large",
            Self::SecretsFull { .. } => "secrets_full",
        }
    }

    /// The typed error for a CDP failure that has one (pipe closed, frame
    /// too large, target gone, timeout, dialog opened); `None` for the rest.
    pub fn from_cdp(err: &CdpError) -> Option<Self> {
        Some(match err {
            CdpError::PipeClosed { .. } => Self::PipeClosed {
                message: err.to_string(),
            },
            CdpError::FrameTooLarge { limit } => Self::FrameTooLarge { limit: *limit },
            CdpError::TargetGone { target_id } => Self::TargetGone {
                target_id: target_id.clone(),
            },
            CdpError::Timeout { method, after } => Self::Timeout {
                method: method.clone(),
                after_ms: u64::try_from(after.as_millis()).unwrap_or(u64::MAX),
            },
            CdpError::DialogOpened { .. } => Self::DialogOpen {
                kind: String::new(),
                message: String::new(),
                detail: err.to_string(),
            },
            _ => return None,
        })
    }

    /// [`Self::from_cdp`], else the generic `Browser` error.
    pub fn cdp(err: &CdpError) -> Self {
        Self::from_cdp(err).unwrap_or_else(|| Self::Browser(err.to_string()))
    }

    /// `self` with a typed CDP failure seen during the same call in place of
    /// a generic `Browser` error (a CDP failure the agent path reported as
    /// text). Any other error is kept: it already says what refused.
    pub fn or_cdp(self, seen: Option<&CdpError>) -> Self {
        match (self, seen.and_then(Self::from_cdp)) {
            (Self::Browser(detail), Some(Self::DialogOpen { kind, message, .. })) => {
                Self::DialogOpen {
                    kind,
                    message,
                    detail,
                }
            }
            (Self::Browser(_), Some(typed)) => typed,
            (other, _) => other,
        }
    }

    /// The legacy message text this error was sent with, when it has one.
    fn message(&self) -> Option<String> {
        Some(match self {
            Self::NeedsConfirm { label } => {
                format!("needs-confirm: {}", Value::String(label.clone()))
            }
            Self::DomainNotAllowed { host } => format!("domain-not-allowed: {host}"),
            Self::StaleRef { reason } => format!("ticket {reason}"),
            Self::UnknownRef { index, indexed } => {
                format!("no interactive element at index {index} ({indexed} indexed)")
            }
            Self::TargetGone { target_id } => {
                format!("CDP target {target_id} detached or was destroyed")
            }
            Self::DialogOpen { detail, .. } => detail.clone(),
            Self::Timeout { method, after_ms } => {
                let within = if after_ms % 1000 == 0 {
                    format!("{} s", after_ms / 1000)
                } else {
                    format!("{after_ms} ms")
                };
                format!("CDP transport: no CDP result for `{method}` within {within}")
            }
            Self::PipeClosed { message } => message.clone(),
            Self::FrameTooLarge { limit } => {
                format!("CDP pipe frame exceeds {limit} bytes; pipe closed")
            }
            _ => return None,
        })
    }

    pub fn to_value(&self) -> Value {
        let mut value = match self {
            Self::UnknownTool(name) => json!({"variant": "UnknownTool", "name": name}),
            Self::UnknownRegion(id) => json!({"variant": "UnknownRegion", "id": id}),
            Self::UnknownRole(role) => json!({"variant": "UnknownRole", "role": role}),
            Self::UnknownAction(action) => json!({"variant": "UnknownAction", "action": action}),
            Self::UnsupportedAction(action) => {
                json!({"variant": "UnsupportedAction", "action": action})
            }
            Self::ExpectedTextMissing { expected } => {
                json!({"variant": "ExpectedTextMissing", "expected": expected})
            }
            Self::RegionStillPresent { id } => json!({"variant": "RegionStillPresent", "id": id}),
            Self::BadConfidence(value) => json!({"variant": "BadConfidence", "value": value}),
            Self::ConfidenceOutOfRange(value) => {
                json!({"variant": "ConfidenceOutOfRange", "value": value})
            }
            Self::BadRunnerUp(message) => json!({"variant": "BadRunnerUp", "message": message}),
            Self::TargetNotTop { region, top } => {
                json!({"variant": "TargetNotTop", "region": region, "top": top})
            }
            Self::SnapshotEvicted { id, oldest } => {
                json!({"variant": "SnapshotEvicted", "id": id, "oldest": oldest})
            }
            Self::UnknownSnapshot(id) => json!({"variant": "UnknownSnapshot", "id": id}),
            Self::BadSnapshot(value) => json!({"variant": "BadSnapshot", "value": value}),
            Self::UnknownExecutor(name) => json!({"variant": "UnknownExecutor", "name": name}),
            Self::NotImplemented { executor } => {
                json!({"variant": "NotImplemented", "executor": executor})
            }
            Self::BrowserUseRejected { message } => {
                json!({"variant": "BrowserUseRejected", "message": message})
            }
            Self::BrowserUseScript { message } => {
                json!({"variant": "BrowserUseScript", "message": message})
            }
            Self::CuaRejected { message } => {
                json!({"variant": "CuaRejected", "message": message})
            }
            Self::CuaScript { message } => {
                json!({"variant": "CuaScript", "message": message})
            }
            Self::InvalidArguments(message) => {
                json!({"variant": "InvalidArguments", "message": message})
            }
            Self::TicketInvalid(message) => {
                json!({"variant": "TicketInvalid", "message": message})
            }
            Self::VerifyNoEffect => json!({"variant": "VerifyNoEffect"}),
            Self::Io { path, message } => {
                json!({"variant": "Io", "path": path, "message": message})
            }
            Self::Fixture(message) => json!({"variant": "Fixture", "message": message}),
            Self::Browser(message) => json!({"variant": "Browser", "message": message}),
            Self::NeedsConfirm { label } => json!({"label": label}),
            Self::DomainNotAllowed { host } => json!({"host": host}),
            Self::StaleRef { reason } => json!({"reason": reason}),
            Self::UnknownRef { index, indexed } => json!({"index": index, "indexed": indexed}),
            Self::TargetGone { target_id } => json!({"target_id": target_id}),
            // `message` is the dialog's own text here; the failure is `detail`.
            Self::DialogOpen {
                kind,
                message,
                detail,
            } => json!({"type": kind, "message": message, "detail": detail}),
            Self::Timeout { method, after_ms } => json!({"method": method, "after_ms": after_ms}),
            Self::FrameTooLarge { limit } => json!({"limit": limit}),
            Self::ToolNotAllowed { tool } => json!({"tool": tool}),
            Self::PathNotAbsolute { path } => json!({"path": path}),
            Self::PathNotAllowed { path, dir } => json!({"path": path, "dir": dir}),
            Self::OutputTooLarge { limit, bytes } => json!({"limit": limit, "bytes": bytes}),
            Self::SecretsFull { limit } => json!({"limit": limit}),
            _ => json!({}),
        };
        if let Some(map) = value.as_object_mut() {
            map.entry("variant")
                .or_insert_with(|| json!(self.variant()));
            if let Some(message) = self.message() {
                map.entry("message").or_insert_with(|| json!(message));
            }
            if matches!(
                self,
                Self::SnapshotRequired
                    | Self::ToolNotAllowed { .. }
                    | Self::PathNotAbsolute { .. }
                    | Self::PathNotAllowed { .. }
                    | Self::DownloadsOff
                    | Self::OutputTooLarge { .. }
                    | Self::SecretsFull { .. }
            ) {
                map.insert("message".into(), json!(self.to_string()));
            }
            map.insert("code".into(), json!(self.code()));
        }
        value
    }

    pub fn to_json_string(&self) -> String {
        self.to_value().to_string()
    }
}

impl fmt::Display for ToolError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::MissingFixture => f.write_str("tool requires fixture or cdp"),
            Self::MissingBefore => f.write_str("verify requires before_snapshot"),
            Self::MissingAfter => f.write_str("verify requires after_snapshot"),
            Self::DuplicateSource => f.write_str("pass fixture or cdp, not both"),
            Self::MissingToolName => f.write_str("tools/call requires a name"),
            Self::UnknownTool(name) => write!(f, "unknown tool `{name}`"),
            Self::GoalNotAccepted => {
                f.write_str("ultra-instinct does not accept a goal or navigate")
            }
            Self::CoordinatesNotAccepted => {
                f.write_str("ultra-instinct does not accept coordinates; act on a region id")
            }
            Self::EmptyText => f.write_str("text must contain at least one alphanumeric token"),
            Self::UnknownRegion(id) => write!(f, "unknown region `{id}`"),
            Self::UnknownRole(role) => write!(f, "unknown role `{role}`"),
            Self::UnknownAction(action) => write!(f, "unknown action `{action}`"),
            Self::UnsupportedAction(action) => {
                write!(f, "browser session cannot perform `{action}`")
            }
            Self::NonFiniteConfidence => f.write_str("confidence must be finite"),
            Self::RunnerUpNeedsConfidence => f.write_str("runner_up requires confidence"),
            Self::RunnerUpIsTarget => f.write_str("runner_up must name a different region"),
            Self::BadRunnerUp(message) => write!(f, "bad runner_up: {message}"),
            Self::TargetNotTop { region, top } => {
                write!(f, "locate ranked `{top}` first, not `{region}`")
            }
            Self::ConfidenceWithQuery => {
                f.write_str("act takes confidence or locate fields, not both")
            }
            Self::SnapshotEvicted { id, oldest } => {
                write!(f, "snapshot {id} was evicted; oldest kept is {oldest}")
            }
            Self::UnknownSnapshot(id) => write!(f, "snapshot {id} was never recorded"),
            Self::BadSnapshot(value) => write!(f, "bad snapshot id `{value}`"),
            Self::ExpectNeedsObserveAfter => {
                f.write_str("expect_text and expect_absent require observe_after")
            }
            Self::ExpectedTextMissing { expected } => {
                write!(f, "expected text `{expected}` did not appear")
            }
            Self::RegionStillPresent { id } => write!(f, "region `{id}` is still present"),
            Self::BadConfidence(value) => write!(f, "bad confidence `{value}`"),
            Self::ConfidenceOutOfRange(value) => {
                write!(f, "confidence `{value}` must be between 0 and 1")
            }
            Self::MissingExpect => f.write_str("verify requires expect_text or expect_absent"),
            Self::BothExpectations => {
                f.write_str("verify accepts expect_text or expect_absent, not both")
            }
            Self::ActNeedsCdp => {
                f.write_str("act needs a CDP fixture or cdp; a manifold fixture has no DOM node")
            }
            Self::UnknownExecutor(name) => write!(f, "unknown executor `{name}`"),
            Self::NotImplemented { executor } => {
                write!(f, "{executor} executor is not implemented")
            }
            Self::BrowserUseIsReplay => {
                f.write_str("browser-use act uses a replay fixture, not a live CDP endpoint")
            }
            Self::BrowserUseRejected { message } => {
                write!(f, "browser-use rejected the semantic act: {message}")
            }
            Self::BrowserUseScript { message } => {
                write!(f, "invalid browser-use script: {message}")
            }
            Self::CuaIsReplay => {
                f.write_str("cua act uses a replay fixture, not a live CDP endpoint")
            }
            Self::CuaRejected { message } => {
                write!(f, "cua rejected the semantic act: {message}")
            }
            Self::CuaScript { message } => {
                write!(f, "invalid cua script: {message}")
            }
            Self::InvalidArguments(message) => write!(f, "invalid arguments: {message}"),
            Self::TicketInvalid(message) => write!(f, "ticket invalid: {message}"),
            Self::VerifyNoEffect => f.write_str("verify saw no effect"),
            Self::Io { path, message } => write!(f, "cannot read {path}: {message}"),
            Self::Fixture(message) => write!(f, "fixture: {message}"),
            Self::Browser(message) => write!(f, "browser: {message}"),
            Self::NeedsConfirm { .. } | Self::DomainNotAllowed { .. } | Self::StaleRef { .. } => {
                write!(f, "ticket invalid: {}", self.message().unwrap_or_default())
            }
            Self::UnknownRef { .. } => write!(
                f,
                "invalid arguments: {}",
                self.message().unwrap_or_default()
            ),
            Self::TargetGone { .. }
            | Self::DialogOpen { .. }
            | Self::Timeout { .. }
            | Self::PipeClosed { .. }
            | Self::FrameTooLarge { .. } => {
                write!(f, "browser: {}", self.message().unwrap_or_default())
            }
            Self::SnapshotRequired => f.write_str(
                "this server binds presses to a snapshot: pass `snapshot` from browser_get_state with the index",
            ),
            Self::ToolNotAllowed { tool } => write!(f, "tool `{tool}` is not allowed on this server"),
            Self::PathNotAbsolute { path } => write!(f, "path {path:?} must be absolute"),
            Self::PathNotAllowed { path, dir } => {
                write!(f, "path {path:?} is outside the download folder {dir:?}")
            }
            Self::DownloadsOff => f.write_str("downloads are turned off on this server"),
            Self::OutputTooLarge { limit, bytes } => {
                write!(f, "output is {bytes} bytes, over the {limit}-byte limit")
            }
            Self::SecretsFull { limit } => write!(
                f,
                "this server already masks {limit} typed secrets; refusing to type another"
            ),
        }
    }
}

impl std::error::Error for ToolError {}
