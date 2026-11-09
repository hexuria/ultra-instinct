//! Argument parsing for the `ultra-instinct` binary.
//!
//! The library is the tested surface. The binary prints this module's stdout
//! and exits non-zero on [`CliError`].

#![forbid(unsafe_code)]

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
    /// `guard` rejected the request (for example a removed `near` field).
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

pub fn usage() -> &'static str {
    "ultra-instinct run|replay|lessons|exams|doctor|install|observe|guard|verify|mcp|skills|batch|session|open|snap|click|type|select|key|scroll|wait|shot|tabs|close|dialog|forward|reload|check|uncheck|focus|keyboard|get|is|count|state|cookies|storage|set|download|network\nrun --goal <text> (--cdp [url] [--url <page>] | --fixture <replay.cdp.json|page.manifold>) [--max-steps N] [--wait-secs N] [--text-model-cmd <program>] [--policy instinct|jev|clef|clef-flash|dojo] [--diary <dir>] [--lessons <store>] [--no-screenshot] [--no-page-text] [--no-derived-text] [--second-opinion] [--live-steps] [--launch [--headed]] [--load-state <path>] [--save-state <path>] [--allow-domain <host>]... [--confirm-press <glob>]... [--confirm <label>]...\n  --launch starts a headless Chrome for the run (headed with --headed) unless --cdp is given, and closes it at exit\n  --load-state sets saved cookies + storage before the first observe; --save-state saves them when the run ends (counts only, never values)\n  --allow-domain shop.example|*.shop.example: input only on those hosts (--url must match); --confirm-press \"Pay*\": a matching click ends BLOCKED until --confirm \"<label>\"\n  goal clauses: `click X if present` acts when X shows up (else skipped); `wait for X` waits for X; both wait up to --wait-secs (default 30)\n  owned agent loop: observe -> Instinct -> gate -> ticket -> execute -> verify (no LLM, no MCP)\n  --cdp with --url opens a background tab and closes it at exit; without --url drives the first existing tab\n  --diary <dir> writes the battle diary (schema v1 JSONL): every decision, step, and outcome\ndoctor [--json] [--quick] [--cdp <url>] checks Chrome, the --cdp endpoint, a test launch, and $TMPDIR/aui; non-zero when a required check fails\ninstall [--run] prints the command that installs Chrome for Testing into ~/.cache/ultra-instinct/chrome (--run runs it with npx, then doctor)\nreplay --diary <dir> [--lessons <store>] re-scores every recorded decision with Instinct: agree / would-abstain / regress\nlessons --diary <dir> --store <path> distills diaries into the versioned lesson store (words / places / moves / trust)
exams --diary <dir> [--lessons <store>] reports remote calls per 100 decisions and the belt each pairing holds\nmcp serves newline-delimited JSON-RPC on stdin.\nshell verbs (one browser per --session <name>, default `default`; a daemon on $TMPDIR/aui/<session>.sock attaches to --cdp <url>, default http://127.0.0.1:9222, or with open --launch starts its own Chrome, which lives as long as the daemon):\n  open <url> [--launch [--headed]] [--idle <secs>] [--allow-domain <host>]... [--confirm-press <glob>]... | snap [--json] | click <ref> [--confirm <label>] [--mouse] | type <ref> <text> | select <ref> <option> | key <ref> <key>\n  scroll [up|down] | wait --text|--url|--control|--gone <v> | --load domcontentloaded|load [--timeout s] | shot [--annotate] [path] | tabs | close (stops the daemon and its Chrome)\n  open --idle <secs> (1 to 86400, default 3600) ends the daemon and its Chrome after that long without a verb; open flags are set when the session starts\n  dialog [status] | dialog accept [text] | dialog dismiss | forward | reload [--ignore-cache] | check|uncheck <ref> [--mouse] | focus <ref> | keyboard type|insert <text> [--confirm <label>]\n  get value|label|role|box <ref> | get attr <ref> <name> | is visible|enabled|checked|editable <ref> | count [--label L] [--role R]\n  state save|load <path> | cookies [list] [--name N] [--domain D] [--reveal] | cookies set <name> <value> [--domain D] [--path P] [--expires SECS] [--http-only] [--secure] [--same-site Strict|Lax|None] | cookies clear [--name N] [--domain D]\n  storage local|session [list] [--reveal] | get <key> | set <key> <value> | clear [<key>]\n  set viewport W H [scale] | set device <name> | set media dark|light|no-preference | set offline on|off | set headers K=V... | set reset\n  download <ref> <path> [--overwrite] [--timeout-ms N] | network block <pattern>... | network unblock [<pattern>...] | network har start | network har stop <path>\n  <ref> is @eN from snap (or N); --max-output N cuts snap text; relative paths are this shell's; a refusal or error exits non-zero\nbatch [--bail] [--json] [--session <name>] \"<verb args>\"... runs verbs in order on one session (no args: a JSON array of strings on stdin); non-zero if any failed\nsession prints the current session name; session list lists live sessions and removes stale sockets\nskills list | skills get <name> prints a guide that ships in the binary (agents: aui skills get core)\nobserve (--fixture <path> | --cdp [url]) prints every region: id, role, label, rect\nguard (--fixture <path> | --cdp [url]) --target <label> [--role button] [--within <id>] [--proposed <id>] [--json]\n  the region whose label equals --target (case and spacing ignored) is the target: unique or refuse, then the hard gate; never clicks\nverify (--expect-text <text> | --expect-absent <id>) (--fixture <path> | --cdp [url])\n"
}

/// Run one invocation. `args` does not include the program name.
/// The returned string is the full stdout body.
pub fn execute(args: &[String]) -> Result<String, CliError> {
    if args.is_empty() || args.iter().any(|arg| arg == "--help" || arg == "-h") {
        return Ok(usage().to_owned());
    }
    match args[0].as_str() {
        "observe" => crate::session_cmd::observe_command(&args[1..]),
        "guard" => crate::session_cmd::guard_command(&args[1..]),
        "verify" => crate::session_cmd::verify_command(&args[1..]),
        "run" => crate::run_cmd::run_command(&args[1..]),
        "replay" => crate::replay_cmd::replay_command(&args[1..]),
        "lessons" => crate::lessons_cmd::lessons_command(&args[1..]),
        "exams" => crate::exams_cmd::exams_command(&args[1..]),
        "doctor" => crate::doctor_cmd::doctor_command(&args[1..]),
        "install" => crate::install_cmd::install_command(&args[1..]),
        "mcp" => Err(CliError::McpIsStdio),
        "daemon" => crate::daemon::daemon_command(&args[1..]),
        "skills" => crate::skills_cmd::skills_command(&args[1..]),
        "batch" => crate::batch::batch_command(&args[1..]),
        "session" => crate::daemon::session_command(&args[1..]),
        verb if crate::verbs::VERBS.contains(&verb) => {
            crate::daemon::verb_command(verb, &args[1..])
        }
        other => Err(CliError::UnknownCommand(other.to_owned())),
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
        "action" => "--action",
        "json" => "--json",
        "cdp" => "--cdp",
        "within" => "--within",
        "proposed" => "--proposed",
        "expect-text" => "--expect-text",
        "expect-absent" => "--expect-absent",
        _ => "--flag",
    }
}

pub(crate) mod batch;
pub(crate) mod daemon;
pub mod diary;
pub(crate) mod doctor_cmd;
pub mod dojo_policy;
pub mod exams_cmd;
pub(crate) mod install_cmd;
pub(crate) mod lessons_cmd;
pub(crate) mod replay_cmd;
pub(crate) mod run_cmd;
pub(crate) mod session_cmd;
pub(crate) mod skills_cmd;
pub(crate) mod state_cmd;
pub(crate) mod verbs;

// Remote transports live in `ultra-instinct-remote`; re-exported under their old paths.
#[cfg(feature = "clef")]
pub use ultra_instinct_remote::clef;
#[cfg(feature = "model-text")]
pub use ultra_instinct_remote::http_text;
#[cfg(all(feature = "jev", feature = "model-text"))]
pub use ultra_instinct_remote::second;
#[cfg(feature = "jev")]
pub use ultra_instinct_remote::typesafe;
