//! Shell verb parsing and rendering: `aui <verb> …` becomes one `browser_*`
//! call (see [`parse_verb`]) and its reply one short human line (see
//! [`render`]). The daemon and its socket live in [`crate::daemon`].
//!
//! `<ref>` is `@eN` or `N` from `aui snap`, passed through as the tool's
//! `index`. Paths (shot, state, download, HAR) resolve against this shell's
//! cwd before they are sent: the daemon writes the file.

use serde_json::{json, Map, Value};

use crate::daemon::{Request, SessionName, Verb, DEFAULT_SESSION};
use crate::CliError;

/// Shell verbs, in usage order.
pub const VERBS: [&str; 27] = [
    "open", "snap", "click", "type", "select", "key", "scroll", "wait", "shot", "tabs", "close",
    "dialog", "forward", "reload", "check", "uncheck", "focus", "keyboard", "get", "is", "count",
    "state", "cookies", "storage", "set", "download", "network",
];

/// Flags that take a value (`--flag v` or `--flag=v`).
const VALUE_FLAGS: [&str; 21] = [
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
];

/// Flags that take no value.
const SWITCHES: [&str; 13] = [
    "--snap",
    "--script",
    "--json",
    "--mouse",
    "--launch",
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
const OUTPUT_TOOLS: [&str; 7] = [
    "browser_get_state",
    "browser_get_text",
    "browser_get_page_text",
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
    let (mut allow_domains, mut confirm_press) = (Vec::new(), Vec::new());
    let (mut launch, mut headed, mut idle_secs) = (false, false, None);
    if verb == "open" {
        allow_domains = flags.all("--allow-domain");
        confirm_press = flags.all("--confirm-press");
        launch = flags.switch("--launch");
        headed = flags.switch("--headed");
        if headed && !launch {
            return Err(CliError::Config("--headed needs --launch".into()));
        }
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
        idle_secs,
    })
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
            _ => usage("<url> [--launch [--headed]] [--idle <secs>] [--allow-domain <host>]... [--confirm-press <glob>]..."),
        },
        "snap" => {
            let format = if flags.switch("--json") { "json" } else { "text" };
            match p[..] {
                [] => tool("browser_get_state", json!({"format": format})),
                _ => usage("[--json]"),
            }
        }
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
pub fn render(verb: &Verb, reply: &str) -> Result<String, CliError> {
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
        "browser_click" | "browser_type" | "browser_select_dropdown" | "browser_send_key" => {
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
        "browser_screenshot" | "browser_download" => {
            format!("✓ saved {} ({} bytes)", s("path"), body["bytes"])
        }
        "browser_list_tabs" => {
            let tabs = body["tabs"].as_array().cloned().unwrap_or_default();
            let mut out = String::new();
            for tab in &tabs {
                let id = tab["tab_id"].as_str().unwrap_or_default();
                out.push_str(&format!(
                    "{}  {}  {}\n",
                    id.get(..8).unwrap_or(id),
                    tab["url"].as_str().unwrap_or_default(),
                    tab["title"].as_str().unwrap_or_default()
                ));
            }
            out.push_str(&format!("{} tabs", tabs.len()));
            out
        }
        "browser_dialog" => render_dialog(&body),
        "browser_go_forward" => "✓ went forward".to_owned(),
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
        (Value::String(state), _) => Ok(format!("{line}\n{state}\n")),
        (_, Value::Null) => Ok(format!("{line}\n")),
        (_, error) => Ok(format!(
            "{line}\n✗ could not read the page after it ({}) · run aui snap\n",
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
        Some("unknown_snapshot") => {
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
                crate::run_cmd::shell_quote(&label)
            )
        }
        ("domain-not-allowed", Some(host)) => {
            format!("✗ refused: DomainNotAllowed {host} · not in --allow-domain")
        }
        _ => format!("✗ refused: {name} · run aui snap"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn a(raw: &[&str]) -> Vec<String> {
        raw.iter().map(|s| (*s).to_owned()).collect()
    }

    fn tool(verb: &str, args: &[&str]) -> (&'static str, Value) {
        match parse_verb(verb, &a(args))
            .unwrap_or_else(|err| panic!("{verb} {args:?}: {err}"))
            .request
        {
            Request::Tool { name, arguments } => (name, arguments),
            Request::Stop => ("stop", Value::Null),
        }
    }

    fn reply(text: &str, is_error: bool) -> String {
        json!({"jsonrpc": "2.0", "id": 1, "result": {
            "content": [{"type": "text", "text": text}], "isError": is_error}})
        .to_string()
    }

    fn rendered(verb: &str, args: &[&str], body: Value) -> String {
        render(
            &parse_verb(verb, &a(args)).unwrap(),
            &reply(&body.to_string(), false),
        )
        .unwrap()
    }

    #[test]
    fn verbs_map_to_browser_tools() {
        assert_eq!(
            tool("open", &["https://shop.example/"]),
            ("browser_navigate", json!({"url": "https://shop.example/"}))
        );
        assert_eq!(
            tool("snap", &[]),
            ("browser_get_state", json!({"format": "text"}))
        );
        assert_eq!(
            tool("snap", &["--json"]),
            ("browser_get_state", json!({"format": "json"}))
        );
        assert_eq!(
            tool("click", &["@e5", "--confirm", "Pay $18"]),
            (
                "browser_click",
                json!({"index": "@e5", "verify": true, "confirm": "Pay $18"})
            )
        );
        assert_eq!(
            tool("type", &["@e2", "kid@example.com"]),
            (
                "browser_type",
                json!({"index": "@e2", "text": "kid@example.com", "verify": true})
            )
        );
        assert_eq!(
            tool("select", &["3", "New", "Zealand"]),
            (
                "browser_select_dropdown",
                json!({"index": "3", "text": "New Zealand", "verify": true})
            )
        );
        assert_eq!(
            tool("key", &["@e2", "Enter"]),
            (
                "browser_send_key",
                json!({"index": "@e2", "key": "Enter", "verify": true})
            )
        );
        assert_eq!(
            tool("scroll", &["up"]),
            ("browser_scroll", json!({"down": false}))
        );
        assert_eq!(
            tool("scroll", &[]),
            ("browser_scroll", json!({"down": true}))
        );
        assert_eq!(
            tool("wait", &["--text", "Welcome", "--timeout", "2.5"]),
            ("browser_wait", json!({"text": "Welcome", "seconds": 2.5}))
        );
        assert_eq!(
            tool("wait", &["--load", "domcontentloaded"]),
            ("browser_wait", json!({"load": "domcontentloaded"}))
        );
        assert_eq!(
            tool("wait", &["--gone", "Loading…", "--load=load"]),
            ("browser_wait", json!({"gone": "Loading…", "load": "load"}))
        );
        assert_eq!(tool("tabs", &[]), ("browser_list_tabs", json!({})));
        assert_eq!(tool("close", &[]), ("stop", Value::Null));
        let (name, arguments) = tool("shot", &["out.png"]);
        assert_eq!(name, "browser_screenshot");
        let path = arguments["path"].as_str().unwrap();
        assert!(std::path::Path::new(path).is_absolute() && path.ends_with("out.png"));
        assert!(arguments.get("annotate").is_none());
    }

    #[test]
    fn page_verbs_map_to_the_extra_tools() {
        let dialog = |args: &[&str]| tool("dialog", args);
        assert_eq!(dialog(&[]), ("browser_dialog", json!({"action": "status"})));
        assert_eq!(
            dialog(&["status"]),
            ("browser_dialog", json!({"action": "status"}))
        );
        assert_eq!(
            dialog(&["accept"]),
            ("browser_dialog", json!({"action": "accept"}))
        );
        assert_eq!(
            dialog(&["accept", "Kid Smith"]),
            (
                "browser_dialog",
                json!({"action": "accept", "text": "Kid Smith"})
            )
        );
        assert_eq!(
            dialog(&["dismiss"]),
            ("browser_dialog", json!({"action": "dismiss"}))
        );
        assert_eq!(tool("forward", &[]), ("browser_go_forward", json!({})));
        assert_eq!(
            tool("reload", &[]),
            ("browser_reload", json!({"ignore_cache": false}))
        );
        assert_eq!(
            tool("reload", &["--ignore-cache"]),
            ("browser_reload", json!({"ignore_cache": true}))
        );
        assert_eq!(
            tool("check", &["@e3"]),
            ("browser_check", json!({"index": "@e3", "checked": true}))
        );
        assert_eq!(
            tool("uncheck", &["3", "--mouse"]),
            (
                "browser_check",
                json!({"index": "3", "checked": false, "mouse": true})
            )
        );
        assert_eq!(
            tool("focus", &["@e1"]),
            ("browser_focus", json!({"index": "@e1"}))
        );
        assert_eq!(
            tool("keyboard", &["type", "hello"]),
            ("browser_keyboard", json!({"text": "hello", "mode": "type"}))
        );
        assert_eq!(
            tool("keyboard", &["type", "\n", "--confirm", "Pay $18"]),
            (
                "browser_keyboard",
                json!({"text": "\n", "mode": "type", "confirm": "Pay $18"})
            )
        );
        assert_eq!(
            tool("keyboard", &["insert", "日本"]),
            (
                "browser_keyboard",
                json!({"text": "日本", "mode": "insert"})
            )
        );
        for what in ["value", "label", "role", "box"] {
            assert_eq!(
                tool("get", &[what, "@e2"]),
                ("browser_get", json!({"index": "@e2", "what": what}))
            );
        }
        assert_eq!(
            tool("get", &["attr", "@e2", "href"]),
            (
                "browser_get",
                json!({"index": "@e2", "what": "attr", "name": "href"})
            )
        );
        for what in ["visible", "enabled", "checked", "editable"] {
            assert_eq!(
                tool("is", &[what, "4"]),
                ("browser_is", json!({"index": "4", "what": what}))
            );
        }
        assert_eq!(tool("count", &[]), ("browser_count", json!({})));
        assert_eq!(
            tool("count", &["--label", "Delete", "--role", "button"]),
            (
                "browser_count",
                json!({"label": "Delete", "role": "button"})
            )
        );
        let (name, arguments) = tool("shot", &["--annotate", "a.png"]);
        assert_eq!(name, "browser_screenshot");
        assert_eq!(arguments["annotate"], true);
        assert!(std::path::Path::new(arguments["path"].as_str().unwrap()).is_absolute());
    }

    #[test]
    fn state_cookie_and_storage_verbs_map_to_their_tools() {
        let (name, arguments) = tool("state", &["save", "login.json"]);
        assert_eq!(name, "browser_state_save");
        let path = arguments["path"].as_str().unwrap();
        assert!(std::path::Path::new(path).is_absolute() && path.ends_with("login.json"));
        assert_eq!(
            tool("state", &["load", "/x/s.json"]).0,
            "browser_state_load"
        );
        assert_eq!(
            tool("state", &["load", "/x/s.json"]).1,
            json!({"path": "/x/s.json"})
        );
        assert_eq!(
            tool("cookies", &[]),
            ("browser_cookies", json!({"action": "list"}))
        );
        assert_eq!(
            tool(
                "cookies",
                &[
                    "list",
                    "--name",
                    "sid",
                    "--domain",
                    "shop.example",
                    "--reveal"
                ]
            ),
            (
                "browser_cookies",
                json!({"action": "list", "name": "sid", "domain": "shop.example", "reveal": true})
            )
        );
        assert_eq!(
            tool(
                "cookies",
                &[
                    "set",
                    "sid",
                    "abc 1",
                    "--domain",
                    ".shop.example",
                    "--path",
                    "/app",
                    "--expires",
                    "1900000000",
                    "--http-only",
                    "--secure",
                    "--same-site",
                    "Lax"
                ]
            ),
            (
                "browser_cookies",
                json!({"action": "set", "name": "sid", "value": "abc 1",
                       "domain": ".shop.example", "path": "/app", "expires": 1900000000.0,
                       "http_only": true, "secure": true, "same_site": "Lax"})
            )
        );
        assert_eq!(
            tool("cookies", &["clear", "--name", "sid"]),
            ("browser_cookies", json!({"action": "clear", "name": "sid"}))
        );
        assert_eq!(
            tool("storage", &["local"]),
            (
                "browser_storage",
                json!({"area": "local", "action": "list"})
            )
        );
        assert_eq!(
            tool("storage", &["session", "list", "--reveal"]),
            (
                "browser_storage",
                json!({"area": "session", "action": "list", "reveal": true})
            )
        );
        assert_eq!(
            tool("storage", &["local", "get", "token"]),
            (
                "browser_storage",
                json!({"area": "local", "action": "get", "key": "token"})
            )
        );
        assert_eq!(
            tool("storage", &["local", "set", "theme", "dark"]),
            (
                "browser_storage",
                json!({"area": "local", "action": "set", "key": "theme", "value": "dark"})
            )
        );
        assert_eq!(
            tool("storage", &["session", "clear"]),
            (
                "browser_storage",
                json!({"area": "session", "action": "clear"})
            )
        );
        assert_eq!(
            tool("storage", &["session", "clear", "cart"]),
            (
                "browser_storage",
                json!({"area": "session", "action": "clear", "key": "cart"})
            )
        );
    }

    #[test]
    fn env_verbs_map_to_set_download_and_network() {
        assert_eq!(
            tool("set", &["viewport", "1280", "720"]),
            (
                "browser_set",
                json!({"what": "viewport", "width": 1280, "height": 720})
            )
        );
        assert_eq!(
            tool("set", &["viewport", "390", "844", "3"]),
            (
                "browser_set",
                json!({"what": "viewport", "width": 390, "height": 844, "scale": 3.0})
            )
        );
        assert_eq!(
            tool("set", &["device", "iPhone 14"]),
            (
                "browser_set",
                json!({"what": "device", "name": "iPhone 14"})
            )
        );
        assert_eq!(
            tool("set", &["media", "dark"]),
            (
                "browser_set",
                json!({"what": "media", "color_scheme": "dark"})
            )
        );
        assert_eq!(
            tool("set", &["offline", "on"]),
            ("browser_set", json!({"what": "offline", "on": true}))
        );
        assert_eq!(
            tool("set", &["offline", "off"]),
            ("browser_set", json!({"what": "offline", "on": false}))
        );
        assert_eq!(
            tool("set", &["headers", "X-Test=1", "Accept-Language=en=US"]),
            (
                "browser_set",
                json!({"what": "headers", "map": {"X-Test": "1", "Accept-Language": "en=US"}})
            )
        );
        assert_eq!(
            tool("set", &["reset"]),
            ("browser_set", json!({"what": "reset"}))
        );
        let (name, arguments) = tool(
            "download",
            &["@e4", "out/r.pdf", "--overwrite", "--timeout-ms", "5000"],
        );
        assert_eq!(name, "browser_download");
        assert_eq!(arguments["index"], "@e4");
        assert_eq!(arguments["overwrite"], true);
        assert_eq!(arguments["timeout_ms"], 5000);
        assert!(std::path::Path::new(arguments["path"].as_str().unwrap()).is_absolute());
        assert_eq!(
            tool("download", &["2", "/tmp/r.pdf"]).1,
            json!({"index": "2", "path": "/tmp/r.pdf"})
        );
        assert_eq!(
            tool("network", &["block", "*.png", "*ads*"]),
            (
                "browser_network",
                json!({"action": "block", "patterns": ["*.png", "*ads*"]})
            )
        );
        assert_eq!(
            tool("network", &["unblock"]),
            ("browser_network", json!({"action": "unblock"}))
        );
        assert_eq!(
            tool("network", &["unblock", "*.png"]),
            (
                "browser_network",
                json!({"action": "unblock", "patterns": ["*.png"]})
            )
        );
        assert_eq!(
            tool("network", &["har", "start"]),
            ("browser_network", json!({"action": "har_start"}))
        );
        let (_, arguments) = tool("network", &["har", "stop", "run.har"]);
        assert_eq!(arguments["action"], "har_stop");
        assert!(std::path::Path::new(arguments["path"].as_str().unwrap()).is_absolute());
    }

    /// `--snap` prints the page after the input.
    #[test]
    fn snap_after_an_input_prints_the_page_after_it() {
        for (verb, args) in [
            ("click", &["@e5", "--snap"][..]),
            ("type", &["@e2", "rust", "--snap"][..]),
            ("open", &["https://a/", "--snap"][..]),
            ("scroll", &["--snap"][..]),
        ] {
            assert_eq!(tool(verb, args).1["state"], true, "{verb}");
        }
        let click = parse_verb("click", &a(&["@e5", "--snap"])).unwrap();
        let body = json!({"acted": "click", "ticket": 41, "verification": "navigation",
                          "state": "Done  http://h/\n@e1 link \"Home\"\n1 control · snapshot 8",
                          "snapshot": 8});
        let out = render(&click, &reply(&body.to_string(), false)).unwrap();
        assert_eq!(
            out,
            "✓ ticket 41 · navigation\nDone  http://h/\n@e1 link \"Home\"\n1 control · snapshot 8\n"
        );
        let failed = json!({"acted": "click", "ticket": 41,
                            "state_error": {"code": "dialog_open", "message": "a dialog is open"}});
        let out = render(&click, &reply(&failed.to_string(), false)).unwrap();
        assert!(
            out.ends_with("(a dialog is open) · run aui snap\n"),
            "{out}"
        );
    }

    #[test]
    fn max_output_reaches_only_the_text_tools() {
        assert_eq!(
            tool("snap", &["--max-output", "400"]).1,
            json!({"format": "text", "max_chars": 400})
        );
        assert_eq!(
            tool("click", &["@e1", "--max-output=9"]).1,
            json!({"index": "@e1", "verify": true})
        );
        assert!(parse_verb("snap", &a(&["--max-output", "lots"])).is_err());
    }

    /// `--script` → `mouse: false`, `--mouse` → `true`, neither → no key, both → error.
    #[test]
    fn click_flags_pick_the_press() {
        assert_eq!(
            tool("click", &["@e1", "--script"]),
            (
                "browser_click",
                json!({"index": "@e1", "verify": true, "mouse": false})
            )
        );
        assert_eq!(
            tool("click", &["@e1", "--mouse"]),
            (
                "browser_click",
                json!({"index": "@e1", "verify": true, "mouse": true})
            )
        );
        let (_, arguments) = tool("click", &["@e1"]);
        assert_eq!(arguments, json!({"index": "@e1", "verify": true}));
        assert!(arguments.get("mouse").is_none(), "{arguments}");
        let err = parse_verb("click", &a(&["@e1", "--mouse", "--script"])).unwrap_err();
        assert!(err.to_string().contains("pick one"), "{err}");
    }

    #[test]
    fn verb_flags_and_arity_are_checked() {
        let open = parse_verb(
            "open",
            &a(&[
                "https://shop.example/",
                "--session",
                "s1",
                "--cdp=http://127.0.0.1:9333",
                "--allow-domain",
                "shop.example",
                "--confirm-press",
                "Pay*",
            ]),
        )
        .unwrap();
        assert_eq!(open.session.as_str(), "s1");
        assert_eq!(open.cdp.as_deref(), Some("http://127.0.0.1:9333"));
        assert_eq!(open.allow_domains, ["shop.example"]);
        assert_eq!(open.confirm_press, ["Pay*"]);
        assert!(!open.launch);
        assert_eq!(parse_verb("snap", &[]).unwrap().cdp, None);
        let launch = parse_verb("open", &a(&["https://a/", "--launch", "--headed"])).unwrap();
        assert!(launch.launch && launch.headed);
        assert_eq!(launch.idle_secs, None);
        let idle = parse_verb("open", &a(&["https://a/", "--launch", "--idle", "90"])).unwrap();
        assert_eq!(idle.idle_secs, Some(90));
        let max = parse_verb("open", &a(&["https://a/", "--idle=86400"])).unwrap();
        assert_eq!(max.idle_secs, Some(86_400));
        for bad in ["0", "86401", "-1", "1.5", "soon"] {
            let err = parse_verb("open", &a(&["https://a/", "--idle", bad])).unwrap_err();
            assert!(err.to_string().contains("--idle"), "{bad}: {err}");
        }
        for (verb, args) in [
            ("snap", &["--idle", "5"][..]),
            ("open", &["https://a/", "--idle", "5", "--idle", "6"][..]),
            ("open", &[][..]),
            ("open", &["https://a/", "--headed"][..]),
            ("snap", &["--launch"][..]),
            ("click", &[][..]),
            ("type", &["@e1"][..]),
            ("key", &["Enter"][..]),
            ("scroll", &["left"][..]),
            ("wait", &[][..]),
            ("wait", &["--timeout", "3"][..]),
            ("wait", &["--timeout", "x", "--url", "/a"][..]),
            ("wait", &["--load", "networkidle"][..]),
            ("wait", &["--gone"][..]),
            ("wait", &["--url", "/a", "--url", "/b"][..]),
            ("snap", &["--confirm", "x"][..]),
            ("snap", &["--json", "--json"][..]),
            ("click", &["@e1", "--mouse=yes"][..]),
            ("click", &["@e1", "--allow-domain", "x"][..]),
            ("open", &["https://a/", "--allow-domain", "https://a/"][..]),
            ("snap", &["--session", "bad name"][..]),
            ("tabs", &["extra"][..]),
            ("dialog", &["maybe"][..]),
            ("reload", &["--mouse"][..]),
            ("check", &[][..]),
            ("focus", &["@e1", "--mouse"][..]),
            ("snap", &["--snap"][..]),
            ("get", &["value", "@e1", "--snap"][..]),
            ("keyboard", &["type"][..]),
            ("keyboard", &["press", "x"][..]),
            ("get", &["attr", "@e1"][..]),
            ("get", &["text", "@e1"][..]),
            ("is", &["hidden", "@e1"][..]),
            ("count", &["Delete"][..]),
            ("state", &["save"][..]),
            ("state", &["dump", "x"][..]),
            ("cookies", &["set", "sid"][..]),
            ("cookies", &["list", "--path", "/"][..]),
            ("cookies", &["clear", "--reveal"][..]),
            ("cookies", &["set", "a", "b", "--expires", "soon"][..]),
            ("storage", &[][..]),
            ("storage", &["cookie"][..]),
            ("storage", &["local", "get"][..]),
            ("storage", &["local", "get", "k", "--reveal"][..]),
            ("set", &["viewport", "wide", "720"][..]),
            ("set", &["viewport", "1", "2", "3", "4"][..]),
            ("set", &["device"][..]),
            ("set", &["media", "sepia"][..]),
            ("set", &["offline", "yes"][..]),
            ("set", &["headers"][..]),
            ("set", &["headers", "novalue"][..]),
            ("set", &["headers", "=v"][..]),
            ("download", &["@e1"][..]),
            ("download", &["@e1", "x", "--timeout-ms", "-1"][..]),
            ("network", &["block"][..]),
            ("network", &["har"][..]),
            ("network", &["har", "stop"][..]),
        ] {
            assert!(parse_verb(verb, &a(args)).is_err(), "{verb} {args:?}");
        }
        assert!(parse_verb("fly", &[]).is_err());
    }

    #[test]
    fn refusals_render_short_and_fail() {
        let click = parse_verb("click", &a(&["@e5"])).unwrap();
        let ok = render(
            &click,
            &reply(
                r#"{"acted":"click","ticket":41,"verification":"navigation"}"#,
                false,
            ),
        )
        .unwrap();
        assert_eq!(ok, "✓ ticket 41 · navigation\n");
        let stale = render(
            &click,
            &reply(
                r#"{"variant":"TicketInvalid","message":"ticket target-gone"}"#,
                true,
            ),
        )
        .unwrap_err();
        assert_eq!(stale.to_string(), "✗ refused: TargetGone · run aui snap");
        let confirm = render(
            &click,
            &reply(
                r#"{"variant":"TicketInvalid","message":"needs-confirm: \"Pay $18\""}"#,
                true,
            ),
        )
        .unwrap_err();
        assert_eq!(
            confirm.to_string(),
            "✗ refused: NeedsConfirm \"Pay $18\" · rerun with --confirm \"Pay \\$18\""
        );
        let gate = render(
            &click,
            &reply(r#"{"variant":"TicketInvalid","message":"occluded"}"#, true),
        )
        .unwrap_err();
        assert_eq!(gate.to_string(), "✗ refused: Occluded · run aui snap");
        let other = render(
            &click,
            &reply(r#"{"variant":"Browser","message":"socket closed"}"#, true),
        )
        .unwrap_err();
        assert_eq!(other.to_string(), "✗ error: socket closed");
        // A daemon-level error (a failed Chrome launch) is shown unchanged.
        let launch = render(
            &click,
            &json!({"jsonrpc": "2.0", "id": 1, "error": {"code": -32000, "message": "no Chrome found"}})
                .to_string(),
        )
        .unwrap_err();
        assert_eq!(launch.to_string(), "✗ error: no Chrome found");
    }

    /// Over MCP a screenshot answers with the image first, then its
    /// metadata as text.
    #[test]
    fn a_screenshot_reply_renders_from_its_text_item() {
        let shot = parse_verb("shot", &a(&["/x/s.png"])).unwrap();
        let reply = json!({"jsonrpc": "2.0", "id": 1, "result": {"content": [
            {"type": "image", "data": "iVBO", "mimeType": "image/png"},
            {"type": "text", "text": r#"{"format":"png","path":"/x/s.png","bytes":4}"#}],
            "isError": false}});
        assert_eq!(
            render(&shot, &reply.to_string()).unwrap(),
            "✓ saved /x/s.png (4 bytes)\n"
        );
    }

    #[test]
    fn new_tool_replies_render_one_short_line() {
        assert_eq!(
            rendered(
                "dialog",
                &[],
                json!({"type": null, "open": false, "tracked": true})
            ),
            "✓ no dialog open\n"
        );
        assert_eq!(
            rendered(
                "dialog",
                &[],
                json!({"type": "confirm", "message": "Delete?", "open": true, "tracked": true})
            ),
            "✓ confirm dialog open · \"Delete?\"\n"
        );
        assert_eq!(
            rendered(
                "dialog",
                &["accept"],
                json!({"type": "prompt", "handled": "accept", "open": false})
            ),
            "✓ accepted prompt dialog\n"
        );
        assert_eq!(
            rendered("forward", &[], json!({"went_forward": true})),
            "✓ went forward\n"
        );
        assert_eq!(
            rendered(
                "check",
                &["@e3"],
                json!({"changed": true, "checked": true, "index": 3})
            ),
            "✓ checked @e3\n"
        );
        assert_eq!(
            rendered(
                "uncheck",
                &["@e3"],
                json!({"changed": false, "checked": false, "index": 3})
            ),
            "✓ already unchecked @e3\n"
        );
        assert_eq!(
            rendered("focus", &["2"], json!({"focused": 2})),
            "✓ focused @e2\n"
        );
        assert_eq!(
            rendered(
                "keyboard",
                &["insert", "hi"],
                json!({"typed": 2, "mode": "insert"})
            ),
            "✓ inserted 2 chars\n"
        );
        assert_eq!(
            rendered(
                "get",
                &["value", "@e1"],
                json!({"value": "kid@example.com"})
            ),
            "kid@example.com\n"
        );
        assert_eq!(
            rendered("get", &["value", "@e1"], json!({"value": null})),
            "(none)\n"
        );
        assert_eq!(
            rendered(
                "get",
                &["box", "@e1"],
                json!({"value": {"x": 20.0, "y": 20.0, "w": 300.0, "h": 28.0}})
            ),
            "20.0 20.0 300.0 28.0\n"
        );
        assert_eq!(
            rendered("is", &["visible", "@e1"], json!({"value": false})),
            "false\n"
        );
        assert_eq!(
            rendered("count", &[], json!({"count": 2, "indexes": [1, 4]})),
            "2  @e1 @e4\n"
        );
        assert_eq!(
            rendered("count", &[], json!({"count": 0, "indexes": []})),
            "0\n"
        );
        let load = rendered(
            "state",
            &["load", "/s.json"],
            json!({"path": "/s.json", "cookies": 3, "applied_origin": "https://a.example",
                   "pending_origins": ["https://b.example"], "hint": "navigate there"}),
        );
        assert_eq!(
            load,
            "✓ loaded state /s.json · 3 cookies · storage for https://a.example\npending origins: https://b.example\nnavigate there\n"
        );
        assert_eq!(
            rendered(
                "cookies",
                &[],
                json!({"count": 1, "cookies": [{"name": "sid", "value": "•••", "domain": "a.example",
                       "path": "/", "http_only": true, "secure": false}]})
            ),
            "sid=•••  a.example/  http-only\n1 cookies\n"
        );
        assert_eq!(
            rendered("cookies", &["clear"], json!({"cleared": "all"})),
            "✓ cleared all cookies\n"
        );
        assert_eq!(
            rendered(
                "storage",
                &["local"],
                json!({"origin": "https://a.example", "area": "local", "count": 1,
                       "entries": {"theme": {"value": "•••", "value_length": 4}}})
            ),
            "theme=•••\n1 local keys · https://a.example\n"
        );
        assert_eq!(
            rendered(
                "set",
                &["viewport", "1280", "720"],
                json!({"set": "viewport", "width": 1280, "height": 720, "scale": 1.0})
            ),
            "✓ viewport 1280x720 @1.0\n"
        );
        assert_eq!(
            rendered(
                "download",
                &["@e2", "/r.pdf"],
                json!({"path": "/r.pdf", "bytes": 5})
            ),
            "✓ saved /r.pdf (5 bytes)\n"
        );
        assert_eq!(
            rendered(
                "network",
                &["block", "*.png"],
                json!({"blocked": ["*.png"]})
            ),
            "✓ blocking *.png\n"
        );
        assert_eq!(
            rendered(
                "network",
                &["har", "stop", "/r.har"],
                json!({"path": "/r.har", "entries": 7})
            ),
            "✓ saved HAR /r.har · 7 entries\n"
        );
    }
}
