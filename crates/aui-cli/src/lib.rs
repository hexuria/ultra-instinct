//! Argument parsing and fixture locate for the `ultra-instinct` binary.
//!
//! The library is the tested surface. The binary prints this module's stdout
//! and exits non-zero on [`CliError`].

#![forbid(unsafe_code)]

use aui_core::{Action, LocateQuery, Role, Zone};
use aui_resonance::{RegionMatcher, WeightedMatcher};

#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum CliError {
    UnknownCommand(String),
    MissingFixture,
    MissingValue(&'static str),
    UnknownFlag(String),
    DuplicateFlag(&'static str),
    UnknownRole(String),
    UnknownPosition(String),
    UnknownAction(String),
    BadDims(String),
    EmptyText,
    Io {
        path: String,
        message: String,
    },
    Fixture(String),
    Locate(String),
    ExpectedTextMissing {
        expected: String,
    },
    RegionStillPresent {
        id: String,
    },
    ConfidenceBelowThreshold {
        confidence_millis: i32,
        minimum_millis: i32,
    },
    AmbiguousTarget {
        top_millis: i32,
        runner_up_millis: i32,
        margin_millis: i32,
        minimum_margin_millis: i32,
    },
    RunnerUpNeedsConfidence,
    NonFiniteConfidence,
    DimsRequireHgra,
    MissingSource,
    Browser(String),
    /// Missing or invalid environment configuration (e.g. `TYPESAFE_API_KEY`).
    Config(String),
    UnknownMatcher(String),
    MissingRegion,
    MissingVerb,
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
}

impl std::fmt::Display for CliError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::UnknownCommand(name) => write!(f, "unknown command `{name}`"),
            Self::MissingFixture => f.write_str("locate requires --fixture <path>"),
            Self::MissingValue(flag) => write!(f, "missing value for {flag}"),
            Self::UnknownFlag(flag) => write!(f, "unknown flag `{flag}`"),
            Self::DuplicateFlag(flag) => write!(f, "duplicate flag {flag}"),
            Self::UnknownRole(role) => write!(f, "unknown role `{role}`"),
            Self::UnknownPosition(position) => write!(f, "unknown position `{position}`"),
            Self::UnknownAction(action) => write!(f, "unknown action `{action}`"),
            Self::BadDims(dims) => write!(f, "unsupported dims `{dims}`"),
            Self::EmptyText => {
                f.write_str("locate text must contain at least one alphanumeric token")
            }
            Self::Io { path, message } => write!(f, "cannot read {path}: {message}"),
            Self::Fixture(message) => write!(f, "fixture: {message}"),
            Self::Locate(message) => write!(f, "locate: {message}"),
            Self::ExpectedTextMissing { expected } => {
                write!(f, "expected text `{expected}` did not appear")
            }
            Self::RegionStillPresent { id } => write!(f, "region `{id}` is still present"),
            Self::ConfidenceBelowThreshold {
                confidence_millis,
                minimum_millis,
            } => write!(
                f,
                "confidence {confidence_millis} is below the act minimum {minimum_millis}"
            ),
            Self::AmbiguousTarget {
                top_millis,
                runner_up_millis,
                margin_millis,
                minimum_margin_millis,
            } => write!(
                f,
                "top {top_millis} and runner-up {runner_up_millis} differ by {margin_millis} millis, below the act margin {minimum_margin_millis}"
            ),
            Self::RunnerUpNeedsConfidence => f.write_str("--runner-up requires --confidence"),
            Self::NonFiniteConfidence => f.write_str("confidence must be finite"),
            Self::DimsRequireHgra => f.write_str("--dims is only valid with --matcher hgra"),
            Self::MissingSource => f.write_str("command requires --fixture <path> or --cdp [url]"),
            Self::Browser(message) => write!(f, "browser: {message}"),
            Self::Config(message) => write!(f, "config: {message}"),
            Self::UnknownMatcher(name) => write!(f, "unknown matcher `{name}`"),
            Self::MissingRegion => f.write_str("act requires a region id"),
            Self::MissingVerb => f.write_str("act requires a verb (`press`)"),
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
        }
    }
}

impl std::error::Error for CliError {}

pub fn usage() -> &'static str {
    "ultra-instinct run|replay|lessons|exams|observe|locate|inspect|guard|verify|diff|mcp\nrun --goal <text> (--cdp [url] [--url <page>] | --fixture <replay.cdp.json|page.manifold>) [--max-steps N] [--wait-secs N] [--text-model-cmd <program>] [--policy instinct|jev|clef|clef-flash|dojo] [--diary <dir>] [--lessons <store>] [--no-screenshot] [--no-page-text] [--allow-derived-text]\n  goal clauses: `click X if present` acts when X shows up (else skipped); `wait for X` waits for X; both wait up to --wait-secs (default 30)\n  owned agent loop: observe -> Instinct -> gate -> ticket -> execute -> verify (no LLM, no MCP)\n  --cdp with --url opens a background tab and closes it at exit; without --url drives the first existing tab\n  --diary <dir> writes the battle diary (schema v1 JSONL): every decision, step, and outcome\nreplay --diary <dir> [--lessons <store>] re-scores every recorded decision with Instinct: agree / would-abstain / regress\nlessons --diary <dir> --store <path> distills diaries into the versioned lesson store (words / places / moves / trust)
exams --diary <dir> [--lessons <store>] reports remote calls per 100 decisions and the belt each pairing holds\nmcp serves newline-delimited JSON-RPC on stdin.\nguard [--fixture <path> | --cdp [url]] --target <label> [--role button] [--proposed <id>] [--json]\nact is a deprecated alias of guard and never clicks.\nlocate [--fixture <path>] [text] [--role ...] [--matcher weighted] [--json]\nverify (--expect-text <text> | --expect-absent <id>) [--fixture <path>]\nDefault matcher: weighted. HGRA is experimental.\n"
}

/// Run one invocation. `args` does not include the program name.
/// The returned string is the full stdout body.
pub fn execute(args: &[String]) -> Result<String, CliError> {
    if args.is_empty() || args.iter().any(|arg| arg == "--help" || arg == "-h") {
        return Ok(usage().to_owned());
    }
    match args[0].as_str() {
        "locate" => locate_command(&args[1..]),
        "observe" => crate::session_cmd::observe_command(&args[1..]),
        "act" => crate::session_cmd::act_command(&args[1..]),
        "guard" => crate::session_cmd::guard_command(&args[1..]),
        "verify" => crate::session_cmd::verify_command(&args[1..]),
        "diff" => crate::session_cmd::diff_command(&args[1..]),
        "inspect" => crate::session_cmd::inspect_command(&args[1..]),
        "run" => crate::run_cmd::run_command(&args[1..]),
        "replay" => crate::replay_cmd::replay_command(&args[1..]),
        "lessons" => crate::lessons_cmd::lessons_command(&args[1..]),
        "exams" => crate::exams_cmd::exams_command(&args[1..]),
        "mcp" => Err(CliError::McpIsStdio),
        other => Err(CliError::UnknownCommand(other.to_owned())),
    }
}

fn locate_command(args: &[String]) -> Result<String, CliError> {
    let mut fixture: Option<String> = None;
    let mut text: Option<String> = None;
    let mut role: Option<String> = None;
    let mut position: Option<String> = None;
    let mut action: Option<String> = None;
    let mut dims: Option<String> = None;
    let mut matcher: Option<String> = None;
    let mut cdp: Option<String> = None;
    let mut json = false;
    let mut index = 0;
    while index < args.len() {
        let arg = &args[index];
        if let Some(value) = arg.strip_prefix("--fixture=") {
            set_once("fixture", &mut fixture, value.to_owned())?;
        } else if let Some(value) = arg.strip_prefix("--text=") {
            set_once("text", &mut text, value.to_owned())?;
        } else if let Some(value) = arg.strip_prefix("--role=") {
            set_once("role", &mut role, value.to_owned())?;
        } else if let Some(value) = arg.strip_prefix("--position=") {
            set_once("position", &mut position, value.to_owned())?;
        } else if let Some(value) = arg.strip_prefix("--action=") {
            set_once("action", &mut action, value.to_owned())?;
        } else if let Some(value) = arg.strip_prefix("--dims=") {
            set_once("dims", &mut dims, value.to_owned())?;
        } else if let Some(value) = arg.strip_prefix("--matcher=") {
            set_once("matcher", &mut matcher, value.to_owned())?;
        } else if let Some(value) = arg.strip_prefix("--cdp=") {
            set_once("cdp", &mut cdp, value.to_owned())?;
        } else if arg == "--cdp" {
            if cdp.is_some() {
                return Err(CliError::DuplicateFlag("--cdp"));
            }
            let next = args.get(index + 1);
            if next.is_some_and(|value| !value.starts_with("--")) {
                cdp = Some(next.unwrap().clone());
                index += 1;
            } else {
                cdp = Some(aui_browser::DEFAULT_CDP_HTTP.to_owned());
            }
        } else if arg == "--json" {
            if json {
                return Err(CliError::DuplicateFlag("--json"));
            }
            json = true;
        } else if let Some(flag) = arg.strip_prefix("--") {
            let name = flag.split('=').next().unwrap_or(flag);
            let slot = match name {
                "fixture" => Some(&mut fixture),
                "text" => Some(&mut text),
                "role" => Some(&mut role),
                "position" => Some(&mut position),
                "action" => Some(&mut action),
                "dims" => Some(&mut dims),
                "matcher" => Some(&mut matcher),
                _ => None,
            };
            let Some(slot) = slot else {
                return Err(CliError::UnknownFlag(arg.clone()));
            };
            index += 1;
            let Some(value) = args.get(index) else {
                return Err(CliError::MissingValue(flag_name(name)));
            };
            if value.starts_with("--") {
                return Err(CliError::MissingValue(flag_name(name)));
            }
            set_once(name, slot, value.clone())?;
        } else {
            set_once("text", &mut text, arg.clone())?;
        }
        index += 1;
    }
    if fixture.is_none() && cdp.is_none() {
        return Err(CliError::MissingFixture);
    }
    if fixture.is_some() && cdp.is_some() {
        return Err(CliError::DuplicateFlag("--cdp"));
    }
    let query = build_query(text, role, position, action)?;
    let matcher_name = matcher.unwrap_or_else(|| "weighted".to_owned());
    let manifold = session_cmd::load_source(fixture.as_deref(), cdp.as_deref())?;
    if dims.is_some() {
        return Err(CliError::DimsRequireHgra);
    }
    let ranked = match matcher_name.as_str() {
        "weighted" => WeightedMatcher::default()
            .rank(&query, &manifold)
            .map_err(|err| CliError::Locate(err.to_string()))?,
        "hgra" => {
            return Err(CliError::UnknownMatcher(
                "hgra is an experiment; enable aui-resonance feature `hgra`".into(),
            ))
        }
        other => return Err(CliError::UnknownMatcher(other.to_owned())),
    };
    if json {
        Ok(render_json(&manifold, &matcher_name, &ranked))
    } else {
        Ok(render_text(&manifold, &ranked))
    }
}

pub(crate) fn set_once(
    name: &str,
    slot: &mut Option<String>,
    value: String,
) -> Result<(), CliError> {
    if slot.is_some() {
        return Err(CliError::DuplicateFlag(flag_name(name)));
    }
    *slot = Some(value);
    Ok(())
}

pub(crate) fn flag_name(name: &str) -> &'static str {
    match name {
        "fixture" => "--fixture",
        "text" => "--text",
        "role" => "--role",
        "position" => "--position",
        "action" => "--action",
        "dims" => "--dims",
        "json" => "--json",
        "matcher" => "--matcher",
        "cdp" => "--cdp",
        "confidence" => "--confidence",
        "runner-up" => "--runner-up",
        "expect-text" => "--expect-text",
        "expect-absent" => "--expect-absent",
        "before" => "--before",
        "after" => "--after",
        _ => "--flag",
    }
}

fn build_query(
    text: Option<String>,
    role: Option<String>,
    position: Option<String>,
    action: Option<String>,
) -> Result<LocateQuery, CliError> {
    let mut query = LocateQuery::new();
    if let Some(text) = text {
        query = query.text(text).map_err(|_| CliError::EmptyText)?;
    }
    if let Some(role) = role {
        let parsed = Role::parse(&role.to_ascii_lowercase()).ok_or(CliError::UnknownRole(role))?;
        query = query.role(parsed);
    }
    if let Some(position) = position {
        let parsed = Zone::parse(&position.to_ascii_lowercase())
            .ok_or(CliError::UnknownPosition(position))?;
        query = query.position(parsed);
    }
    if let Some(action) = action {
        let parsed =
            Action::parse(&action.to_ascii_lowercase()).ok_or(CliError::UnknownAction(action))?;
        query = query.action(parsed);
    }
    Ok(query)
}

fn render_text(
    manifold: &aui_core::InteractionManifold,
    ranked: &[aui_resonance::Match],
) -> String {
    let mut out = String::new();
    for candidate in ranked {
        let region = manifold.get(candidate.id());
        let label = region.map(|region| region.label()).unwrap_or("");
        let role = region
            .map(|region| region.role().as_str())
            .unwrap_or("unknown");
        out.push_str(&format!(
            "{}\t{}\t{:.6}\t{}\t{}\n",
            candidate.rank(),
            candidate.id(),
            candidate.confidence(),
            role,
            label
        ));
    }
    out
}

fn render_json(
    manifold: &aui_core::InteractionManifold,
    matcher: &str,
    ranked: &[aui_resonance::Match],
) -> String {
    let mut out = String::new();
    out.push_str("{\n");
    out.push_str("  \"product\": \"ultra-instinct\",\n");
    out.push_str(&format!("  \"matcher\": \"{matcher}\",\n"));
    out.push_str("  \"candidates\": [\n");
    for (index, candidate) in ranked.iter().enumerate() {
        let region = manifold.get(candidate.id());
        let label = region.map(|region| region.label()).unwrap_or("");
        let role = region
            .map(|region| region.role().as_str())
            .unwrap_or("unknown");
        let comma = if index + 1 == ranked.len() { "" } else { "," };
        out.push_str("    {\n");
        out.push_str(&format!("      \"rank\": {},\n", candidate.rank()));
        out.push_str(&format!(
            "      \"id\": \"{}\",\n",
            json_escape(candidate.id().as_str())
        ));
        out.push_str(&format!("      \"role\": \"{role}\",\n"));
        out.push_str(&format!("      \"label\": \"{}\",\n", json_escape(label)));
        out.push_str(&format!(
            "      \"confidence\": {:.6}\n",
            candidate.confidence()
        ));
        out.push_str(&format!("    }}{comma}\n"));
    }
    out.push_str("  ]\n}\n");
    out
}

fn json_escape(text: &str) -> String {
    let mut out = String::new();
    for ch in text.chars() {
        match ch {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            ch if ch.is_control() => out.push_str(&format!("\\u{:04x}", u32::from(ch))),
            ch => out.push(ch),
        }
    }
    out
}

mod compare;
pub mod diary;
pub mod dojo_policy;
pub mod exams_cmd;
pub(crate) mod lessons_cmd;
pub(crate) mod replay_cmd;
pub(crate) mod run_cmd;
pub(crate) mod session_cmd;

#[cfg(feature = "clef")]
pub mod clef;
#[cfg(feature = "model-text")]
pub mod http_text;
#[cfg(feature = "jev")]
pub mod typesafe;

#[cfg(feature = "jev")]
pub use compare::fixture_compare_live;
pub use compare::{eval_corpus, fixture_compare, CompareError, CorpusReport, FixtureReport};
