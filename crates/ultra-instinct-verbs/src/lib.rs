//! The `aui` shell verbs: one verb per `browser_*` tool family, run on a
//! session daemon that owns one MCP `Server` (one CDP socket per session).
//!
//! [`verbs`] parses a verb into a tool call and renders the reply,
//! [`daemon`] is the per-session socket server and its client, and
//! [`batch`] runs several verbs in order. The `ultra-instinct` CLI crate
//! dispatches to these and owns every other command.

#![forbid(unsafe_code)]

pub mod batch;
pub mod daemon;
pub mod diff;
pub mod probe;
pub(crate) mod record;
pub mod verbs;

#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum CliError {
    UnknownCommand(String),
    MissingValue(&'static str),
    UnknownFlag(String),
    DuplicateFlag(&'static str),
    UnknownRole(String),
    UnknownAction(String),
    EmptyText,
    Io {
        path: String,
        message: String,
    },
    Fixture(String),
    /// `guard` rejected the request (for example an unsupported `near` field).
    Guard(String),
    ExpectedTextMissing {
        expected: String,
    },
    RegionStillPresent {
        id: String,
    },
    RunnerUpNeedsConfidence,
    NonFiniteConfidence,
    MissingSource,
    Browser(String),
    /// Missing or invalid environment configuration (e.g. `TYPESAFE_API_KEY`).
    Config(String),
    BadConfidence(String),
    MissingExpect,
    UnknownRegion(String),
    UnknownExecutor(String),
    UnsupportedAction(String),
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
    /// `mcp` is served by the binary, which owns stdin. This library call does not.
    McpIsStdio,
    /// `run` ended in a failed agent outcome; the transcript is attached.
    Agent(String),
    BadNumber {
        flag: &'static str,
        value: String,
    },
    /// The guard or the user's policy refused (shell verbs, `run --url`).
    /// The message is the whole line to show.
    Refused(String),
    /// `aui batch` with a failed command. The body is the whole stdout
    /// (every command's output); the exit is non-zero.
    BatchFailed(String),
}

impl std::fmt::Display for CliError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::UnknownCommand(name) => write!(f, "unknown command `{name}`"),
            Self::MissingValue(flag) => write!(f, "missing value for {flag}"),
            Self::UnknownFlag(flag) => write!(f, "unknown flag `{flag}`"),
            Self::DuplicateFlag(flag) => write!(f, "duplicate flag {flag}"),
            Self::UnknownRole(role) => write!(f, "unknown role `{role}`"),
            Self::UnknownAction(action) => write!(f, "unknown action `{action}`"),
            Self::EmptyText => f.write_str("text must contain at least one alphanumeric token"),
            Self::Io { path, message } => write!(f, "cannot read {path}: {message}"),
            Self::Fixture(message) => write!(f, "fixture: {message}"),
            Self::Guard(message) => write!(f, "guard: {message}"),
            Self::ExpectedTextMissing { expected } => {
                write!(f, "expected text `{expected}` did not appear")
            }
            Self::RegionStillPresent { id } => write!(f, "region `{id}` is still present"),
            Self::RunnerUpNeedsConfidence => f.write_str("--runner-up requires --confidence"),
            Self::NonFiniteConfidence => f.write_str("confidence must be finite"),
            Self::MissingSource => f.write_str("command requires --fixture <path> or --cdp [url]"),
            Self::Browser(message) => write!(f, "browser: {message}"),
            Self::Config(message) => write!(f, "config: {message}"),
            Self::BadConfidence(value) => write!(f, "bad confidence `{value}`"),
            Self::MissingExpect => f.write_str("verify requires --expect-text or --expect-absent"),
            Self::UnknownRegion(id) => write!(f, "unknown region `{id}`"),
            Self::UnknownExecutor(name) => write!(f, "unknown executor `{name}`"),
            Self::UnsupportedAction(action) => {
                write!(f, "browser-use executor cannot perform `{action}`")
            }
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
            Self::McpIsStdio => {
                f.write_str("mcp serves JSON-RPC on stdio; run the ultra-instinct binary")
            }
            Self::Agent(transcript) => write!(f, "agent run failed\n{transcript}"),
            Self::BadNumber { flag, value } => write!(f, "{flag} expects a number, got `{value}`"),
            Self::Refused(message) => f.write_str(message),
            Self::BatchFailed(body) => f.write_str(body),
        }
    }
}

impl std::error::Error for CliError {}

/// `label` in double quotes a POSIX shell reads back verbatim.
pub fn shell_quote(label: &str) -> String {
    let mut out = String::from("\"");
    for c in label.chars() {
        if matches!(c, '"' | '\\' | '$' | '`') {
            out.push('\\');
        }
        out.push(c);
    }
    out.push('"');
    out
}
