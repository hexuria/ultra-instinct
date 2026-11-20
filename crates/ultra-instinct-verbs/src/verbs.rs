//! Shell verb parsing and rendering: `aui <verb> …` becomes one `browser_*`
//! call (see [`parse_verb`]) and its reply one short human line (see
//! [`render`]). The daemon and its socket live in `daemon.rs`.
//!
//! `<ref>` is `@eN` or `N` from `aui snap`, passed through as the tool's
//! `index`. Paths (shot, state, download, HAR) resolve against this shell's
//! cwd before they are sent: the daemon writes the file.

use serde_json::{json, Map, Value};

use crate::daemon::DEFAULT_SESSION;
pub use crate::daemon::{Request, SessionName, Verb};
use crate::CliError;

/// Shell verbs, in usage order.
pub const VERBS: [&str; 49] = [
    "diff",
    "open",
    "snap",
    "click",
    "type",
    "select",
    "key",
    "scroll",
    "wait",
    "shot",
    "tabs",
    "close",
    "dialog",
    "forward",
    "reload",
    "check",
    "uncheck",
    "focus",
    "keyboard",
    "get",
    "is",
    "count",
    "state",
    "cookies",
    "storage",
    "set",
    "download",
    "network",
    "back",
    "hover",
    "drag",
    "upload",
    "text",
    "read",
    "html",
    "find",
    "pdf",
    "tab",
    "eval",
    "console",
    "requests",
    "dblclick",
    "rightclick",
    "midclick",
    "tripleclick",
    "hold",
    "scrollto",
    "zoom",
    "options",
];

/// Flags that take a value (`--flag v` or `--flag=v`).
const VALUE_FLAGS: [&str; 24] = [
    "--ms",
    "--scale",
    "--session",
    "--cdp",
    "--max-output",
    "--allow-domain",
    "--confirm-press",
    "--confirm",
    "--text",
    "--url",
    "--control",
    "--gone",
    "--load",
    "--timeout",
    "--label",
    "--role",
    "--name",
    "--domain",
    "--path",
    "--expires",
    "--same-site",
    "--timeout-ms",
    "--idle",
    "--profile",
];

/// Flags that take no value.
const SWITCHES: [&str; 17] = [
    "--boundary",
    "--landscape",
    "--background",
    "--snap",
    "--script",
    "--json",
    "--mouse",
    "--launch",
    "--attach",
    "--headed",
    "--ignore-cache",
    "--annotate",
    "--reveal",
    "--http-only",
    "--secure",
    "--overwrite",
    "--bail",
];

/// Tools `--max-output N` gives `max_chars: N`.
const OUTPUT_TOOLS: [&str; 6] = [
    "browser_get_state",
    "browser_get_text",
    "browser_get_html",
    "browser_extract_content",
    "browser_read_console",
    "browser_read_network",
];

/// One verb's flags and positional words. A verb takes the flags it knows;
/// [`Flags::finish`] refuses whatever is left.
pub(crate) struct Flags {
    values: Vec<(&'static str, String)>,
    switches: Vec<&'static str>,
    positional: Vec<String>,
}

impl Flags {
    pub(crate) fn scan(args: &[String]) -> Result<Self, CliError> {
        let mut out = Self {
            values: Vec::new(),
            switches: Vec::new(),
            positional: Vec::new(),
        };
        let mut i = 0;
        while i < args.len() {
            let arg = &args[i];
            i += 1;
            if !arg.starts_with("--") {
                out.positional.push(arg.clone());
                continue;
            }
            let (name, inline) = match arg.split_once('=') {
                Some((name, value)) => (name, Some(value.to_owned())),
                None => (arg.as_str(), None),
            };
            if let Some(flag) = VALUE_FLAGS.iter().copied().find(|f| *f == name) {
                let value = match inline {
                    Some(value) => value,
                    None => {
                        let value = args.get(i).cloned().ok_or(CliError::MissingValue(flag))?;
                        i += 1;
                        value
                    }
                };
                out.values.push((flag, value));
            } else if let Some(flag) = SWITCHES
                .iter()
                .copied()
                .find(|f| *f == name && inline.is_none())
            {
                if out.switches.contains(&flag) {
                    return Err(CliError::DuplicateFlag(flag));
                }
                out.switches.push(flag);
            } else {
                return Err(CliError::UnknownFlag(arg.clone()));
            }
        }
        Ok(out)
    }

    /// Every value of a repeatable flag.
    pub(crate) fn all(&mut self, flag: &str) -> Vec<String> {
        let (hit, rest) = std::mem::take(&mut self.values)
            .into_iter()
            .partition::<Vec<_>, _>(|(f, _)| *f == flag);
        self.values = rest;
        hit.into_iter().map(|(_, v)| v).collect()
    }

    /// At most one value.
    pub(crate) fn one(&mut self, flag: &'static str) -> Result<Option<String>, CliError> {
        let mut all = self.all(flag);
        if all.len() > 1 {
            return Err(CliError::DuplicateFlag(flag));
        }
        Ok(all.pop())
    }

    pub(crate) fn switch(&mut self, flag: &str) -> bool {
        let at = self.switches.iter().position(|f| *f == flag);
        at.map(|at| self.switches.remove(at)).is_some()
    }

    pub(crate) fn positional(&mut self) -> Vec<String> {
        std::mem::take(&mut self.positional)
    }

    /// Refuse a flag no part of the verb took.
    pub(crate) fn finish(&self) -> Result<(), CliError> {
        if let Some((flag, _)) = self.values.first() {
            return Err(CliError::UnknownFlag((*flag).to_owned()));
        }
        if let Some(flag) = self.switches.first() {
            return Err(CliError::UnknownFlag((*flag).to_owned()));
        }
        Ok(())
    }
}

fn number<T: std::str::FromStr>(flag: &'static str, raw: &str) -> Result<T, CliError> {
    raw.trim().parse().map_err(|_| CliError::BadNumber {
        flag,
        value: raw.to_owned(),
    })
}

/// `path` against this shell's cwd (the daemon writes the file).
fn absolute(path: &str) -> String {
    let path = std::path::PathBuf::from(path);
    std::env::current_dir()
        .map(|cwd| cwd.join(&path))
        .unwrap_or(path)
        .to_string_lossy()
        .into_owned()
}

/// Parse `aui <verb> …` (`args` excludes the verb).
pub fn parse_verb(verb: &str, args: &[String]) -> Result<Verb, CliError> {
    let verb: &'static str = VERBS
        .iter()
        .copied()
        .find(|v| *v == verb)
        .ok_or_else(|| CliError::UnknownCommand(verb.to_owned()))?;
    let mut flags = Flags::scan(args)?;
    let session = SessionName::parse(
        flags
            .one("--session")?
            .as_deref()
            .unwrap_or(DEFAULT_SESSION),
    )?;
    let cdp = flags.one("--cdp")?;
    let max_output = flags
        .one("--max-output")?
        .map(|raw| number::<usize>("--max-output", &raw))
        .transpose()?;
    let boundary = flags.switch("--boundary")
        || std::env::var("AUI_CONTENT_BOUNDARIES").is_ok_and(|v| !v.trim().is_empty());
    let (mut allow_domains, mut confirm_press) = (Vec::new(), Vec::new());
    let (mut launch, mut headed, mut idle_secs, mut profile) = (false, false, None, None);
    let mut attach = false;
    if verb == "open" {
        allow_domains = flags.all("--allow-domain");
        confirm_press = flags.all("--confirm-press");
        launch = flags.switch("--launch");
        headed = flags.switch("--headed");
        attach = flags.switch("--attach");
        let dir = flags.one("--profile")?;
        let others = [launch, cdp.is_some(), dir.is_some(), headed];
        crate::daemon::check_attach(attach, others, !allow_domains.is_empty())?;
        if headed && !launch {
            return Err(CliError::Config("--headed needs --launch".into()));
        }
        profile = crate::daemon::launch_profile(launch, cdp.is_some(), dir.as_deref())?;
        idle_secs = flags
            .one("--idle")?
            .map(|raw| crate::daemon::parse_idle(&raw))
            .transpose()?;
    }
    for host in &allow_domains {
        ultra_instinct_guard::HostPattern::parse(host)
            .map_err(|err| CliError::Config(err.to_string()))?;
    }
    for glob in &confirm_press {
        ultra_instinct_guard::LabelGlob::parse(glob)
            .map_err(|err| CliError::Config(err.to_string()))?;
    }
    let snap_after = flags.switch("--snap");
    let mut request = build(verb, &mut flags)?;
    flags.finish()?;
    // `--snap`: the page after the input, printed and bound like `aui snap`.
    if snap_after {
        match &mut request {
            Request::Tool { name, arguments } if ultra_instinct_mcp::STATE_AFTER.contains(name) => {
                arguments["state"] = json!(true);
            }
            _ => {
                return Err(CliError::Config(format!(
                    "--snap: aui {verb} is not an input; run aui snap"
                )))
            }
        }
    }
    if let (Some(max), Request::Tool { name, arguments }) = (max_output, &mut request) {
        if OUTPUT_TOOLS.contains(name) {
            arguments["max_chars"] = json!(max);
        }
    }
    Ok(Verb {
        verb,
        session,
        cdp,
        request,
        allow_domains,
        confirm_press,
        launch,
        headed,
        profile,
        idle_secs,
        attach,
        boundary,
    })
}

/// Tools whose reply is page content: text a page author controls.
const PAGE_CONTENT_TOOLS: [&str; 12] = [
    "browser_get_state",
    "browser_get_text",
    "browser_get_html",
    "browser_extract_content",
    "browser_find",
    "browser_read_console",
    "browser_read_network",
    "browser_exec",
    "browser_get",
    "browser_get_dropdown_options",
    "browser_list_tabs",
    "browser_dialog",
];

/// 128 random bits as hex, from the OS-seeded keys of two `RandomState`s.
/// Fresh for every output, so a page cannot know it when it is rendered.
fn nonce() -> String {
    use std::hash::{BuildHasher, Hasher};
    let word = || {
        std::collections::hash_map::RandomState::new()
            .build_hasher()
            .finish()
    };
    format!("{:016x}{:016x}", word(), word())
}

/// `content` between two marker lines carrying a fresh id that does not
/// occur in it. Everything between them is page data, never instructions.
fn bounded(content: &str) -> String {
    let mut id = nonce();
    while content.contains(&id) {
        id = nonce();
    }
    let newline = if content.ends_with('\n') { "" } else { "\n" };
    format!("<<<page-content {id}>>>\n{content}{newline}<<<end page-content {id}>>>\n")
}

/// The verb's output. With `--boundary` page content sits inside
/// [`bounded`] markers: a reading verb's whole output, and the page an input
/// verb prints after itself with `--snap`. A page that prints a marker of
/// its own cannot close the block: it does not have this output's id.
pub fn render(verb: &Verb, reply: &str) -> Result<String, CliError> {
    let out = render_plain(verb, reply)?;
    let page_content = matches!(
        &verb.request,
        Request::Tool { name, .. } if PAGE_CONTENT_TOOLS.contains(name)
    );
    match verb.boundary && page_content {
        true => Ok(bounded(&out)),
        false => Ok(out),
    }
}

/// `--script`: script click; `--mouse`: pointer (the default); not both.
fn click_kind(flags: &mut Flags) -> Result<Option<bool>, CliError> {
    match (flags.switch("--mouse"), flags.switch("--script")) {
        (true, true) => Err(CliError::Config(
            "--mouse and --script: pick one (a pointer press is the default)".into(),
        )),
        (_, true) => Ok(Some(false)),
        (true, false) => Ok(Some(true)),
        (false, false) => Ok(None),
    }
}

/// One verb's request. Usage errors name the verb's own form.
fn build(verb: &'static str, flags: &mut Flags) -> Result<Request, CliError> {
    let usage = |form: &str| Err(CliError::Config(format!("usage: aui {verb} {form}")));
    let tool = |name: &'static str, arguments: Value| Ok(Request::Tool { name, arguments });
    let words = flags.positional();
    let p: Vec<&str> = words.iter().map(String::as_str).collect();
    match verb {
        "open" => match p[..] {
            [url] => tool("browser_navigate", json!({"url": url})),
            _ => usage("<url> [--launch [--headed] [--profile <dir>] | --attach] [--idle <secs>] [--allow-domain <host>]... [--confirm-press <glob>]..."),
        },
        "snap" => {
            let format = if flags.switch("--json") { "json" } else { "text" };
            match p[..] {
                [] => tool("browser_get_state", json!({"format": format})),
                _ => usage("[--json]"),
            }
        }
        "diff" => match p[..] {
            [] => tool("browser_get_state", json!({"format": "text", "diff": true})),
            _ => usage(""),
        },
        "click" => {
            let confirm = flags.one("--confirm")?;
            let mouse = click_kind(flags)?;
            let [index] = p[..] else {
                return usage("<ref> [--confirm <label>] [--script]");
            };
            let mut arguments = json!({"index": index, "verify": true});
            if let Some(label) = confirm {
                arguments["confirm"] = json!(label);
            }
            if let Some(mouse) = mouse {
                arguments["mouse"] = json!(mouse);
            }
            tool("browser_click", arguments)
        }
        "type" | "select" => {
            let [index, ref rest @ ..] = p[..] else {
                return usage(if verb == "type" { "<ref> <text>" } else { "<ref> <option>" });
            };
            if rest.is_empty() {
                return usage(if verb == "type" { "<ref> <text>" } else { "<ref> <option>" });
            }
            let name = if verb == "type" { "browser_type" } else { "browser_select_dropdown" };
            tool(name, json!({"index": index, "text": rest.join(" "), "verify": true}))
        }
        "key" => match p[..] {
            [index, key] => tool(
                "browser_send_key",
                json!({"index": index, "key": key, "verify": true}),
            ),
            _ => usage("<ref> <key>"),
        },
        "scroll" => match p[..] {
            [] | ["down"] => tool("browser_scroll", json!({"down": true})),
            ["up"] => tool("browser_scroll", json!({"down": false})),
            _ => usage("[up|down]"),
        },
        "wait" => wait(flags, &p),
        "shot" => {
            let annotate = flags.switch("--annotate");
            let path = match p[..] {
                [] => format!(
                    "aui-shot-{}.png",
                    std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .map(|d| d.as_secs())
                        .unwrap_or(0)
                ),
                [path] => path.to_owned(),
                _ => return usage("[--annotate] [path]"),
            };
            let mut arguments = json!({"path": absolute(&path)});
            if annotate {
                arguments["annotate"] = json!(true);
            }
            tool("browser_screenshot", arguments)
        }
        "tabs" => match p[..] {
            [] => tool("browser_list_tabs", json!({})),
            _ => usage(""),
        },
        "close" => match p[..] {
            [] => Ok(Request::Stop),
            _ => usage(""),
        },
        "dialog" => match p[..] {
            [] | ["status"] => tool("browser_dialog", json!({"action": "status"})),
            ["accept"] => tool("browser_dialog", json!({"action": "accept"})),
            ["accept", ref text @ ..] => tool(
                "browser_dialog",
                json!({"action": "accept", "text": text.join(" ")}),
            ),
            ["dismiss"] => tool("browser_dialog", json!({"action": "dismiss"})),
            _ => usage("[status] | accept [text] | dismiss"),
        },
        "dblclick" | "rightclick" | "midclick" | "tripleclick" => {
            let confirm = flags.one("--confirm")?;
            let [index] = p[..] else {
                return usage("<ref> [--confirm <label>]");
            };
            let mut arguments = json!({"index": index});
            if let Some(label) = confirm {
                arguments["confirm"] = json!(label);
            }
            let name = match verb {
                "dblclick" => "browser_double_click",
                "rightclick" => "browser_right_click",
                "midclick" => "browser_middle_click",
                _ => "browser_triple_click",
            };
            tool(name, arguments)
        }
        // The tool holds a key on the focused element (no ticket): `focus`
        // or `click` the field first.
        "hold" => {
            let ms = flags.one("--ms")?;
            let confirm = flags.one("--confirm")?;
            let [key] = p[..] else {
                return usage("<key> [--ms <n>] [--confirm <label>]");
            };
            let mut arguments = json!({"key": key});
            if let Some(ms) = ms {
                let ms: u64 = ms
                    .parse()
                    .map_err(|_| CliError::Config(format!("--ms {ms:?} is not a whole number")))?;
                arguments["hold_ms"] = json!(ms);
            }
            if let Some(label) = confirm {
                arguments["confirm"] = json!(label);
            }
            tool("browser_hold_key", arguments)
        }
        "scrollto" => match p[..] {
            [index] => tool("browser_scroll_to", json!({"index": index})),
            _ => usage("<ref>"),
        },
        "zoom" => match p[..] {
            [direction @ ("in" | "out" | "reset")] => {
                tool("browser_zoom", json!({"direction": direction}))
            }
            // `NaN` and `inf` parse as f64 but are not JSON numbers: the tool
            // would read the missing factor as `reset`.
            [factor] => match factor.parse::<f64>() {
                Ok(factor) if factor.is_finite() && factor > 0.0 => tool("browser_zoom", json!({"factor": factor})),
                _ => usage("in|out|reset|<factor>"),
            },
            _ => usage("in|out|reset|<factor>"),
        },
        "options" => match p[..] {
            [index] => tool("browser_get_dropdown_options", json!({"index": index})),
            _ => usage("<ref>"),
        },
        "tab" => match p[..] {
            ["new"] => tool("browser_new_tab", json!({})),
            ["new", url] => tool("browser_new_tab", json!({"url": url})),
            ["switch", id] => tool("browser_switch_tab", json!({"tab_id": id})),
            ["close", id] => tool("browser_close_tab", json!({"tab_id": id})),
            _ => usage("new [url] | switch <tab> | close <tab>"),
        },
        "eval" => match p[..] {
            [script] => tool("browser_exec", json!({"script": script})),
            _ => usage("<script>"),
        },
        "console" => match p[..] {
            [] => tool("browser_read_console", json!({})),
            _ => usage(""),
        },
        "requests" => match p[..] {
            [] => tool("browser_read_network", json!({})),
            _ => usage(""),
        },
        "text" => match p[..] {
            [] => tool("browser_get_text", json!({})),
            _ => usage(""),
        },
        "read" => match p[..] {
            [] => tool("browser_extract_content", json!({})),
            _ => usage(""),
        },
        "html" => match p[..] {
            [] => tool("browser_get_html", json!({})),
            _ => usage(""),
        },
        "find" => {
            let role = flags.one("--role")?;
            let mut arguments = json!({});
            match p[..] {
                [] if role.is_some() => {}
                [text] => arguments["text"] = json!(text),
                _ => return usage("[text] [--role <role>]"),
            }
            if let Some(role) = role {
                arguments["role"] = json!(role);
            }
            tool("browser_find", arguments)
        }
        "pdf" => {
            let landscape = flags.switch("--landscape");
            let background = flags.switch("--background");
            let scale = flags.one("--scale")?;
            let [path] = p[..] else {
                return usage("<path> [--landscape] [--background] [--scale <n>]");
            };
            let mut arguments = json!({
                "path": absolute(path),
                "landscape": landscape,
                "print_background": background,
            });
            if let Some(scale) = scale {
                let scale: f64 = scale.parse().map_err(|_| {
                    CliError::Config(format!("--scale {scale:?} is not a number"))
                })?;
                arguments["scale"] = json!(scale);
            }
            tool("browser_save_as_pdf", arguments)
        }
        "back" => match p[..] {
            [] => tool("browser_go_back", json!({})),
            _ => usage(""),
        },
        "hover" => match p[..] {
            [index] => tool("browser_hover", json!({"index": index})),
            _ => usage("<ref>"),
        },
        "drag" => {
            let confirm = flags.one("--confirm")?;
            let [from, to] = p[..] else {
                return usage("<ref> <ref> [--confirm <label>]");
            };
            let mut arguments = json!({"from_index": from, "to_index": to});
            if let Some(label) = confirm {
                arguments["confirm"] = json!(label);
            }
            tool("browser_drag", arguments)
        }
        "upload" => {
            let confirm = flags.one("--confirm")?;
            let [index, ref paths @ ..] = p[..] else {
                return usage("<ref> <path>... [--confirm <label>]");
            };
            if paths.is_empty() {
                return usage("<ref> <path>... [--confirm <label>]");
            }
            let paths: Vec<String> = paths.iter().map(|path| absolute(path)).collect();
            let mut arguments = json!({"index": index, "paths": paths});
            if let Some(label) = confirm {
                arguments["confirm"] = json!(label);
            }
            tool("browser_file_upload", arguments)
        }
        "forward" => match p[..] {
            [] => tool("browser_go_forward", json!({})),
            _ => usage(""),
        },
        "reload" => {
            let ignore_cache = flags.switch("--ignore-cache");
            match p[..] {
                [] => tool("browser_reload", json!({"ignore_cache": ignore_cache})),
                _ => usage("[--ignore-cache]"),
            }
        }
        "check" | "uncheck" => {
            let mouse = click_kind(flags)?;
            let [index] = p[..] else {
                return usage("<ref> [--script]");
            };
            let mut arguments = json!({"index": index, "checked": verb == "check"});
            if let Some(mouse) = mouse {
                arguments["mouse"] = json!(mouse);
            }
            tool("browser_check", arguments)
        }
        "focus" => match p[..] {
            [index] => tool("browser_focus", json!({"index": index})),
            _ => usage("<ref>"),
        },
        "keyboard" => {
            let confirm = flags.one("--confirm")?;
            match p[..] {
                [mode @ ("type" | "insert"), ref text @ ..] if !text.is_empty() => {
                    let mut arguments = json!({"text": text.join(" "), "mode": mode});
                    if let Some(label) = confirm {
                        arguments["confirm"] = json!(label);
                    }
                    tool("browser_keyboard", arguments)
                }
                _ => usage("type|insert <text> [--confirm <label>]"),
            }
        }
        "get" => match p[..] {
            [what @ ("value" | "label" | "role" | "box"), index] => {
                tool("browser_get", json!({"index": index, "what": what}))
            }
            ["attr", index, name] => tool(
                "browser_get",
                json!({"index": index, "what": "attr", "name": name}),
            ),
            _ => usage("value|label|role|box <ref> | attr <ref> <name>"),
        },
        "is" => match p[..] {
            [what @ ("visible" | "enabled" | "checked" | "editable"), index] => {
                tool("browser_is", json!({"index": index, "what": what}))
            }
            _ => usage("visible|enabled|checked|editable <ref>"),
        },
        "count" => {
            let mut arguments = Map::new();
            for (flag, key) in [("--label", "label"), ("--role", "role")] {
                if let Some(value) = flags.one(flag)? {
                    arguments.insert(key.into(), json!(value));
                }
            }
            match p[..] {
                [] => tool("browser_count", Value::Object(arguments)),
                _ => usage("[--label <label>] [--role <role>]"),
            }
        }
        "state" => match p[..] {
            ["save", path] => tool("browser_state_save", json!({"path": absolute(path)})),
            ["load", path] => tool("browser_state_load", json!({"path": absolute(path)})),
            _ => usage("save|load <path>"),
        },
        "cookies" => cookies(flags, &p),
        "storage" => storage(flags, &p),
        "set" => set(&p),
        "download" => {
            let overwrite = flags.switch("--overwrite");
            let timeout_ms = flags
                .one("--timeout-ms")?
                .map(|raw| number::<u64>("--timeout-ms", &raw))
                .transpose()?;
            let [index, path] = p[..] else {
                return usage("<ref> <path> [--overwrite] [--timeout-ms N]");
            };
            let mut arguments = json!({"index": index, "path": absolute(path)});
            if overwrite {
                arguments["overwrite"] = json!(true);
            }
            if let Some(ms) = timeout_ms {
                arguments["timeout_ms"] = json!(ms);
            }
            tool("browser_download", arguments)
        }
        "network" => match p[..] {
            ["block", ref patterns @ ..] if !patterns.is_empty() => tool(
                "browser_network",
                json!({"action": "block", "patterns": patterns}),
            ),
            ["unblock"] => tool("browser_network", json!({"action": "unblock"})),
            ["unblock", ref patterns @ ..] => tool(
                "browser_network",
                json!({"action": "unblock", "patterns": patterns}),
            ),
            ["har", "start"] => tool("browser_network", json!({"action": "har_start"})),
            ["har", "stop", path] => tool(
                "browser_network",
                json!({"action": "har_stop", "path": absolute(path)}),
            ),
            _ => usage("block <pattern>... | unblock [<pattern>...] | har start | har stop <path>"),
        },
        _ => unreachable!("every verb in VERBS has a request"),
    }
}

fn wait(flags: &mut Flags, p: &[&str]) -> Result<Request, CliError> {
    const USAGE: &str =
        "usage: aui wait --text|--url|--control|--gone <v> | --load domcontentloaded|load [--timeout s]";
    let mut wait = Map::new();
    for (flag, key) in [
        ("--text", "text"),
        ("--url", "url"),
        ("--control", "control"),
        ("--gone", "gone"),
        ("--load", "load"),
    ] {
        if let Some(value) = flags.one(flag)? {
            wait.insert(key.into(), json!(value));
        }
    }
    if let Some(load) = wait.get("load").and_then(Value::as_str) {
        if !matches!(load, "domcontentloaded" | "load") {
            return Err(CliError::Config(format!(
                "--load is domcontentloaded or load, got `{load}`"
            )));
        }
    }
    if let Some(raw) = flags.one("--timeout")? {
        let secs: f64 = number("--timeout", &raw)?;
        if !(secs.is_finite() && secs >= 0.0) {
            return Err(CliError::BadNumber {
                flag: "--timeout",
                value: raw,
            });
        }
        wait.insert("seconds".into(), json!(secs));
    }
    if !p.is_empty() || !wait.keys().any(|k| k != "seconds") {
        return Err(CliError::Config(USAGE.into()));
    }
    Ok(Request::Tool {
        name: "browser_wait",
        arguments: Value::Object(wait),
    })
}

fn cookies(flags: &mut Flags, p: &[&str]) -> Result<Request, CliError> {
    let mut arguments = Map::new();
    let mut take = |flags: &mut Flags, flag: &'static str, key: &str| -> Result<(), CliError> {
        if let Some(value) = flags.one(flag)? {
            arguments.insert(key.into(), json!(value));
        }
        Ok(())
    };
    match p {
        [] | ["list"] => {
            take(flags, "--name", "name")?;
            take(flags, "--domain", "domain")?;
            arguments.insert("action".into(), json!("list"));
            if flags.switch("--reveal") {
                arguments.insert("reveal".into(), json!(true));
            }
        }
        ["set", name, value] => {
            take(flags, "--domain", "domain")?;
            take(flags, "--path", "path")?;
            take(flags, "--same-site", "same_site")?;
            arguments.insert("action".into(), json!("set"));
            arguments.insert("name".into(), json!(name));
            arguments.insert("value".into(), json!(value));
            if let Some(raw) = flags.one("--expires")? {
                arguments.insert("expires".into(), json!(number::<f64>("--expires", &raw)?));
            }
            for (flag, key) in [("--http-only", "http_only"), ("--secure", "secure")] {
                if flags.switch(flag) {
                    arguments.insert(key.into(), json!(true));
                }
            }
        }
        ["clear"] => {
            take(flags, "--name", "name")?;
            take(flags, "--domain", "domain")?;
            arguments.insert("action".into(), json!("clear"));
        }
        _ => {
            return Err(CliError::Config(
                "usage: aui cookies [list] [--name N] [--domain D] [--reveal] | set <name> <value> [--domain D] [--path P] [--expires SECS] [--http-only] [--secure] [--same-site Strict|Lax|None] | clear [--name N] [--domain D]".into(),
            ))
        }
    }
    Ok(Request::Tool {
        name: "browser_cookies",
        arguments: Value::Object(arguments),
    })
}

fn storage(flags: &mut Flags, p: &[&str]) -> Result<Request, CliError> {
    let usage = || {
        Err(CliError::Config(
            "usage: aui storage local|session [list] [--reveal] | get <key> | set <key> <value> | clear [<key>]".into(),
        ))
    };
    let [area @ ("local" | "session"), ref rest @ ..] = p[..] else {
        return usage();
    };
    let arguments = match rest {
        [] | ["list"] => {
            let mut arguments = json!({"area": area, "action": "list"});
            if flags.switch("--reveal") {
                arguments["reveal"] = json!(true);
            }
            arguments
        }
        ["get", key] => json!({"area": area, "action": "get", "key": key}),
        ["set", key, value] => json!({"area": area, "action": "set", "key": key, "value": value}),
        ["clear"] => json!({"area": area, "action": "clear"}),
        ["clear", key] => json!({"area": area, "action": "clear", "key": key}),
        _ => return usage(),
    };
    Ok(Request::Tool {
        name: "browser_storage",
        arguments,
    })
}

fn set(p: &[&str]) -> Result<Request, CliError> {
    let arguments = match *p {
        ["viewport", width, height, ref scale @ ..] if scale.len() <= 1 => {
            let mut arguments = json!({
                "what": "viewport",
                "width": number::<u32>("width", width)?,
                "height": number::<u32>("height", height)?,
            });
            if let [scale] = scale {
                arguments["scale"] = json!(number::<f64>("scale", scale)?);
            }
            arguments
        }
        ["device", ref name @ ..] if !name.is_empty() => {
            json!({"what": "device", "name": name.join(" ")})
        }
        ["media", scheme @ ("dark" | "light" | "no-preference")] => {
            json!({"what": "media", "color_scheme": scheme})
        }
        ["offline", on @ ("on" | "off")] => json!({"what": "offline", "on": on == "on"}),
        ["headers", ref pairs @ ..] if !pairs.is_empty() => {
            let mut map = Map::new();
            for pair in pairs {
                match pair.split_once('=') {
                    Some((key, value)) if !key.trim().is_empty() => {
                        map.insert(key.trim().to_owned(), json!(value));
                    }
                    _ => {
                        return Err(CliError::Config(format!(
                            "aui set headers takes K=V pairs, got `{pair}`"
                        )))
                    }
                }
            }
            json!({"what": "headers", "map": map})
        }
        ["reset"] => json!({"what": "reset"}),
        _ => {
            return Err(CliError::Config(
                "usage: aui set viewport W H [scale] | device <name> | media dark|light|no-preference | offline on|off | headers K=V... | reset".into(),
            ))
        }
    };
    Ok(Request::Tool {
        name: "browser_set",
        arguments,
    })
}

/// Short human text for one daemon reply. A refusal or an error is `Err`
/// (non-zero exit).
/// The first 8 characters of a tab id: what `aui tabs` prints and
/// `aui tab switch|close` takes.
fn short(id: &str) -> &str {
    id.get(..8).unwrap_or(id)
}

/// What a diagnostic read lost, after its count: ` · N dropped` when the
/// daemon's event buffer overflowed, and the tool's cut marker when
/// `--max-output` left entries out. Both are gone for good (reads drain).
fn dropped(body: &Value) -> String {
    let mut out = String::new();
    if let Some(lost) = body["dropped"].as_u64().filter(|&lost| lost > 0) {
        out.push_str(&format!(" · {lost} dropped"));
    }
    if let Some(cut) = body["truncated"].as_str() {
        out.push_str(&format!(" · {cut}"));
    }
    out
}

fn render_plain(verb: &Verb, reply: &str) -> Result<String, CliError> {
    let reply: Value = serde_json::from_str(reply)
        .map_err(|err| CliError::Browser(format!("bad daemon reply: {err}")))?;
    if let Some(error) = reply.get("error") {
        let message = error.get("message").and_then(Value::as_str).unwrap_or("");
        return Err(CliError::Refused(format!("✗ error: {message}")));
    }
    let result = &reply["result"];
    let (name, arguments) = match &verb.request {
        Request::Stop => return Ok("✓ daemon stopped\n".to_owned()),
        Request::Tool { name, arguments } => (*name, arguments),
    };
    // A screenshot's first item is the image; its metadata is the text item.
    let text = result["content"]
        .as_array()
        .and_then(|items| items.iter().find_map(|item| item["text"].as_str()))
        .unwrap_or_default();
    if result["isError"] == true {
        return Err(CliError::Refused(render_error(text)));
    }
    if name == "browser_get_state" && arguments["format"] == "text" {
        return Ok(format!("{text}\n"));
    }
    let body: Value = serde_json::from_str(text).unwrap_or(Value::Null);
    let s = |key: &str| body[key].as_str().unwrap_or_default().to_owned();
    let line = match name {
        "browser_navigate" => format!(
            "✓ session {} · opened {}",
            verb.session.as_str(),
            s("navigated")
        ),
        "browser_scroll_to" => format!("✓ scrolled to @e{}", body["index"]),
        "browser_hold_key" => format!("✓ held {}", s("key")),
        "browser_zoom" => format!("✓ zoom {}", body["zoomed"]),
        "browser_get_dropdown_options" => {
            let labels = |key: &str| -> Vec<String> {
                body[key]
                    .as_array()
                    .map(|all| {
                        all.iter()
                            .filter_map(|label| label.as_str().map(str::to_owned))
                            .collect()
                    })
                    .unwrap_or_default()
            };
            // `selected` is the selected labels joined with ", ".
            let options = labels("options");
            let chosen = s("selected");
            let selected: Vec<&str> = if options.contains(&chosen) {
                vec![chosen.as_str()]
            } else {
                chosen.split(", ").collect()
            };
            let mut out = String::new();
            for option in &options {
                let mark = if selected.contains(&option.as_str()) {
                    '*'
                } else {
                    ' '
                };
                out.push_str(&format!("{mark} {option}\n"));
            }
            out.push_str(&format!("{} options", options.len()));
            out
        }
        "browser_click"
        | "browser_type"
        | "browser_select_dropdown"
        | "browser_send_key"
        | "browser_double_click"
        | "browser_right_click"
        | "browser_middle_click"
        | "browser_triple_click" => {
            let mut line = format!("✓ ticket {}", body["ticket"]);
            if let Some(kind) = body["verification"].as_str() {
                line.push_str(&format!(" · {kind}"));
            }
            line
        }
        "browser_scroll" => format!("✓ scrolled {}", s("scrolled")),
        "browser_wait" => {
            let secs = body["after_ms"].as_u64().unwrap_or(0) as f64 / 1000.0;
            let looks = body["looks"].as_u64().unwrap_or(0);
            if body["seen"] != true {
                return Err(CliError::Refused(format!(
                    "✗ not seen in {secs:.1} s · last url {}",
                    body["url"].as_str().unwrap_or("unknown")
                )));
            }
            format!("✓ seen after {secs:.1} s · {looks} looks")
        }
        "browser_new_tab" => format!("✓ new tab {} · {}", short(&s("tab_id")), s("url")),
        "browser_switch_tab" => {
            format!("✓ on tab {} · {}", short(&s("active_tab")), s("url"))
        }
        "browser_close_tab" => format!("✓ closed tab {}", short(&s("tab_id"))),
        "browser_exec" => match &body["result"] {
            Value::String(text) => text.clone(),
            other => other.to_string(),
        },
        "browser_read_console" => {
            let entries = body["entries"].as_array().cloned().unwrap_or_default();
            let mut out = String::new();
            for entry in &entries {
                out.push_str(&format!(
                    "{}  {}\n",
                    entry["level"].as_str().unwrap_or_default(),
                    entry["text"].as_str().unwrap_or_default()
                ));
            }
            out.push_str(&format!("{} entries", entries.len()));
            out.push_str(&dropped(&body));
            out
        }
        "browser_read_network" => {
            let entries = body["entries"].as_array().cloned().unwrap_or_default();
            let mut out = String::new();
            for row in &entries {
                let entry = &row["entry"];
                let outcome = match (&entry["error"], &entry["status"]) {
                    (Value::String(error), _) => error.clone(),
                    (_, Value::Null) => "-".to_owned(),
                    (_, status) => status.to_string(),
                };
                out.push_str(&format!(
                    "{}  {outcome}  {}\n",
                    entry["method"].as_str().unwrap_or("-"),
                    entry["url"].as_str().unwrap_or_default()
                ));
            }
            out.push_str(&format!("{} requests", entries.len()));
            out.push_str(&dropped(&body));
            out
        }
        "browser_get_text" => s("text").to_owned(),
        "browser_get_html" => s("html").to_owned(),
        "browser_extract_content" => s("content").to_owned(),
        "browser_find" => {
            let matches = body["matches"].as_array().cloned().unwrap_or_default();
            let mut out = String::new();
            for found in &matches {
                out.push_str(&format!(
                    "@e{} {} {:?}\n",
                    found["index"],
                    found["role"].as_str().unwrap_or_default(),
                    found["label"].as_str().unwrap_or_default()
                ));
            }
            out.push_str(&format!("{} found", matches.len()));
            out
        }
        "browser_screenshot" | "browser_download" | "browser_save_as_pdf" => {
            format!("✓ saved {} ({} bytes)", s("path"), body["bytes"])
        }
        "browser_list_tabs" => {
            let tabs = body["tabs"].as_array().cloned().unwrap_or_default();
            let mut out = String::new();
            for tab in &tabs {
                out.push_str(&format!(
                    "{}  {}  {}\n",
                    short(tab["tab_id"].as_str().unwrap_or_default()),
                    tab["url"].as_str().unwrap_or_default(),
                    tab["title"].as_str().unwrap_or_default()
                ));
            }
            out.push_str(&format!("{} tabs", tabs.len()));
            out
        }
        "browser_dialog" => render_dialog(&body),
        "browser_go_forward" => "✓ went forward".to_owned(),
        "browser_go_back" => "✓ went back".to_owned(),
        "browser_hover" => format!("✓ hovering @e{}", body["hovered"]),
        "browser_drag" => format!(
            "✓ dragged @e{} to @e{}",
            body["dragged"]["from"], body["dragged"]["to"]
        ),
        "browser_file_upload" => {
            let files = arguments["paths"].as_array().map_or(0, Vec::len);
            format!(
                "✓ ticket {} · {files} files set on @e{}",
                body["ticket"], body["index"]
            )
        }
        "browser_reload" => "✓ reloaded".to_owned(),
        "browser_check" => {
            let state = if body["checked"] == true {
                "checked"
            } else {
                "unchecked"
            };
            if body["changed"] == true {
                format!("✓ {state} @e{}", body["index"])
            } else {
                format!("✓ already {state} @e{}", body["index"])
            }
        }
        "browser_focus" => format!("✓ focused @e{}", body["focused"]),
        "browser_keyboard" => {
            let done = if body["mode"] == "insert" {
                "inserted"
            } else {
                "typed"
            };
            format!("✓ {done} {} chars", body["typed"])
        }
        "browser_get" => match &body["value"] {
            Value::String(value) => value.clone(),
            Value::Null => "(none)".to_owned(),
            Value::Object(b) if arguments["what"] == "box" => {
                format!("{} {} {} {}", b["x"], b["y"], b["w"], b["h"])
            }
            other => other.to_string(),
        },
        "browser_is" => (body["value"] == true).to_string(),
        "browser_count" => {
            let refs: Vec<String> = body["indexes"]
                .as_array()
                .map(|all| all.iter().map(|i| format!("@e{i}")).collect())
                .unwrap_or_default();
            if refs.is_empty() {
                body["count"].to_string()
            } else {
                format!("{}  {}", body["count"], refs.join(" "))
            }
        }
        "browser_state_save" => format!(
            "✓ saved state {} · {} cookies · {} origins",
            s("path"),
            body["cookies"],
            body["origins"].as_array().map_or(0, Vec::len)
        ),
        "browser_state_load" => {
            let mut line = format!("✓ loaded state {} · {} cookies", s("path"), body["cookies"]);
            if let Some(origin) = body["applied_origin"].as_str() {
                line.push_str(&format!(" · storage for {origin}"));
            }
            let pending: Vec<&str> = body["pending_origins"]
                .as_array()
                .map(|all| all.iter().filter_map(Value::as_str).collect())
                .unwrap_or_default();
            if !pending.is_empty() {
                line.push_str(&format!("\npending origins: {}", pending.join(", ")));
                if let Some(hint) = body["hint"].as_str() {
                    line.push_str(&format!("\n{hint}"));
                }
            }
            line
        }
        "browser_cookies" => render_cookies(&body, arguments),
        "browser_storage" => render_storage(&body, arguments),
        "browser_set" => render_set(&body),
        "browser_network" => render_network(&body, arguments),
        _ => text.to_owned(),
    };
    // `--snap`: the page after the input, like `aui snap` prints it.
    match (&body["state"], &body["state_error"]) {
        (Value::String(state), _) if verb.boundary => Ok(format!("{line}\n{}", bounded(state))),
        (Value::String(state), _) => Ok(format!("{line}\n{state}\n")),
        (_, Value::Null) => Ok(format!("{line}\n")),
        (_, error) => Ok(format!(
            "{line}\nnote: the press ran, but reading the page after it failed ({}); run aui snap\n",
            error["message"].as_str().unwrap_or("read failed")
        )),
    }
}

fn render_dialog(body: &Value) -> String {
    let kind = body["type"].as_str().unwrap_or("JavaScript");
    let message = body["message"].as_str().unwrap_or_default();
    match body["handled"].as_str() {
        Some("accept") => format!("✓ accepted {kind} dialog"),
        Some(_) => format!("✓ dismissed {kind} dialog"),
        None if body["open"] == true => format!("✓ {kind} dialog open · {message:?}"),
        None if body["tracked"] == false => "✓ no dialog open (not tracked yet on this tab)".into(),
        None => "✓ no dialog open".into(),
    }
}

fn render_cookies(body: &Value, arguments: &Value) -> String {
    match arguments["action"].as_str() {
        Some("set") => format!(
            "✓ set cookie {} on {}",
            body["set"]["name"].as_str().unwrap_or_default(),
            body["set"]["domain"].as_str().unwrap_or_default()
        ),
        Some("clear") => match &body["cleared"] {
            Value::String(all) => format!("✓ cleared {all} cookies"),
            n => format!("✓ cleared {n} cookies"),
        },
        _ => {
            let mut out = String::new();
            for cookie in body["cookies"].as_array().into_iter().flatten() {
                let mut flags = Vec::new();
                if cookie["http_only"] == true {
                    flags.push("http-only");
                }
                if cookie["secure"] == true {
                    flags.push("secure");
                }
                out.push_str(&format!(
                    "{}={}  {}{}{}\n",
                    cookie["name"].as_str().unwrap_or_default(),
                    cookie["value"].as_str().unwrap_or_default(),
                    cookie["domain"].as_str().unwrap_or_default(),
                    cookie["path"].as_str().unwrap_or_default(),
                    if flags.is_empty() {
                        String::new()
                    } else {
                        format!("  {}", flags.join(" "))
                    }
                ));
            }
            out.push_str(&format!("{} cookies", body["count"]));
            out
        }
    }
}

fn render_storage(body: &Value, arguments: &Value) -> String {
    let area = arguments["area"].as_str().unwrap_or_default();
    match arguments["action"].as_str() {
        Some("get") => body["value"]
            .as_str()
            .map_or_else(|| "(none)".to_owned(), str::to_owned),
        Some("set") => format!(
            "✓ set {area} {} ({} chars)",
            body["set"].as_str().unwrap_or_default(),
            body["value_length"]
        ),
        Some("clear") => format!(
            "✓ cleared {area} {}",
            body["cleared"].as_str().unwrap_or_default()
        ),
        _ => {
            let mut out = String::new();
            for (key, entry) in body["entries"].as_object().into_iter().flatten() {
                out.push_str(&format!(
                    "{key}={}\n",
                    entry["value"].as_str().unwrap_or_default()
                ));
            }
            out.push_str(&format!(
                "{} {area} keys · {}",
                body["count"],
                body["origin"].as_str().unwrap_or_default()
            ));
            out
        }
    }
}

fn render_set(body: &Value) -> String {
    match body["set"].as_str().unwrap_or_default() {
        "viewport" => format!(
            "✓ viewport {}x{} @{}",
            body["width"], body["height"], body["scale"]
        ),
        "device" => format!(
            "✓ device {} ({}x{})",
            body["name"].as_str().unwrap_or_default(),
            body["width"],
            body["height"]
        ),
        "media" => format!(
            "✓ color scheme {}",
            body["color_scheme"].as_str().unwrap_or_default()
        ),
        "offline" if body["on"] == true => "✓ offline".to_owned(),
        "offline" => "✓ online".to_owned(),
        "headers" => {
            let names: Vec<&str> = body["names"]
                .as_array()
                .map(|all| all.iter().filter_map(Value::as_str).collect())
                .unwrap_or_default();
            format!("✓ extra headers: {}", names.join(", "))
        }
        _ => "✓ settings reset".to_owned(),
    }
}

fn render_network(body: &Value, arguments: &Value) -> String {
    match arguments["action"].as_str() {
        Some("har_start") if body["already"] == true => "✓ already recording HAR".to_owned(),
        Some("har_start") => "✓ recording HAR".to_owned(),
        Some("har_stop") => format!(
            "✓ saved HAR {} · {} entries",
            body["path"].as_str().unwrap_or_default(),
            body["entries"]
        ),
        _ => {
            let list: Vec<&str> = body["blocked"]
                .as_array()
                .map(|all| all.iter().filter_map(Value::as_str).collect())
                .unwrap_or_default();
            if list.is_empty() {
                "✓ blocking nothing".to_owned()
            } else {
                format!("✓ blocking {}", list.join(" "))
            }
        }
    }
}

/// `✗ refused: <Reason> · run aui snap` for a ticket / gate / policy
/// refusal, `✗ error: …` otherwise.
fn render_error(text: &str) -> String {
    let error: Value = serde_json::from_str(text).unwrap_or(Value::Null);
    // A `dialog_open` error's `message` is the dialog's text; the failure is `detail`.
    let message = match error["code"].as_str() {
        Some("dialog_open") => error["detail"].as_str(),
        _ => error["message"].as_str(),
    }
    .unwrap_or(text);
    match error["code"].as_str() {
        Some("snapshot_required") => {
            return "✗ refused: no aui snap on this session yet · run aui snap".into()
        }
        Some("unknown_snapshot" | "snapshot_evicted") => {
            return "✗ refused: the last aui snap expired · run aui snap".into()
        }
        _ => {}
    }
    if error["variant"] != "TicketInvalid" {
        return format!("✗ error: {message}");
    }
    let reason = ["ticket invalid: ", "gate refused: ", "ticket "]
        .iter()
        .fold(message, |m, prefix| m.strip_prefix(prefix).unwrap_or(m));
    let (token, detail) = match reason.split_once(": ") {
        Some((token, detail)) => (token, Some(detail)),
        None => (reason, None),
    };
    let name: String = token
        .split('-')
        .map(|word| {
            let mut chars = word.chars();
            chars
                .next()
                .map(|c| c.to_ascii_uppercase().to_string() + chars.as_str())
                .unwrap_or_default()
        })
        .collect();
    match (token, detail) {
        ("needs-confirm", Some(label)) => {
            let label: String = serde_json::from_str(label).unwrap_or_else(|_| label.to_owned());
            format!(
                "✗ refused: NeedsConfirm {label:?} · rerun with --confirm {}",
                crate::shell_quote(&label)
            )
        }
        ("domain-not-allowed", Some(host)) => {
            format!("✗ refused: DomainNotAllowed {host} · not in --allow-domain")
        }
        _ => format!("✗ refused: {name} · run aui snap"),
    }
}
