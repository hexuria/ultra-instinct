//! `aui doctor [--json] [--quick] [--cdp <url>]`: what aui needs and what is missing.
//!
//! Checks the Chrome binary aui would launch, whether `--cdp` answers
//! `/json/version`, a real test launch (skipped by `--quick`), and the
//! `$TMPDIR/aui` socket dir. Exits non-zero when a required check fails;
//! the report still goes to stdout first.

use std::io::{Read, Write};
use std::net::{TcpStream, ToSocketAddrs};
use std::path::PathBuf;
use std::time::Duration;

use ultra_instinct_browser::launch::{self, ChromeProcess, LaunchError, LaunchOptions};

use crate::CliError;

/// Start a Chrome owned by the caller (`run --launch`): killed on drop; on
/// `profile` (kept on disk) when given, else a temp profile.
pub(crate) fn launch_chrome(
    headed: bool,
    profile: Option<&std::path::Path>,
) -> Result<ChromeProcess, CliError> {
    launch::launch(&LaunchOptions {
        headed,
        profile_dir: profile.map(std::path::Path::to_path_buf),
        ..LaunchOptions::default()
    })
    .map_err(|err| CliError::Browser(err.to_string()))
}

#[derive(Debug)]
struct Check {
    name: &'static str,
    ok: bool,
    required: bool,
    skipped: bool,
    detail: String,
    hint: String,
}

impl Check {
    fn new(name: &'static str, required: bool, result: Result<String, (String, String)>) -> Self {
        let (ok, detail, hint) = match result {
            Ok(detail) => (true, detail, String::new()),
            Err((detail, hint)) => (false, detail, hint),
        };
        Self {
            name,
            ok,
            required,
            skipped: false,
            detail,
            hint,
        }
    }
}

pub(crate) fn doctor_command(args: &[String]) -> Result<String, CliError> {
    let (mut json, mut quick, mut cdp) = (false, false, None);
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--json" if !json => json = true,
            "--quick" if !quick => quick = true,
            "--cdp" if cdp.is_none() => match args.get(i + 1) {
                Some(v) if !v.starts_with("--") => {
                    cdp = Some(v.clone());
                    i += 1;
                }
                _ => return Err(CliError::MissingValue("--cdp")),
            },
            "--json" | "--quick" | "--cdp" => return Err(CliError::DuplicateFlag("doctor flag")),
            other => match other.strip_prefix("--cdp=") {
                Some(v) if cdp.is_none() => cdp = Some(v.to_owned()),
                _ => return Err(CliError::UnknownFlag(other.to_owned())),
            },
        }
        i += 1;
    }
    let cdp = cdp.unwrap_or_else(|| ultra_instinct_browser::DEFAULT_CDP_HTTP.to_owned());
    let checks = run_checks(&cdp, quick);
    let body = if json {
        render_json(&checks)
    } else {
        render_text(&checks)
    };
    let failed = checks.iter().filter(|c| c.required && !c.ok).count();
    if failed == 0 {
        return Ok(body);
    }
    print!("{body}");
    let _ = std::io::stdout().flush();
    Err(CliError::Config(format!(
        "doctor: {failed} required check(s) failed"
    )))
}

fn run_checks(cdp: &str, quick: bool) -> Vec<Check> {
    let found = launch::find_chrome();
    let chrome = Check::new(
        "chrome",
        true,
        match &found {
            Ok(path) => Ok(match launch::chrome_version(path) {
                Ok(version) => format!("{} ({version})", path.display()),
                Err(err) => format!("{} (version unknown: {err})", path.display()),
            }),
            Err(err @ LaunchError::BadOverride(_)) => Err((
                err.to_string(),
                format!(
                    "point {} at a Chrome binary, or unset it",
                    launch::CHROME_ENV
                ),
            )),
            Err(err) => Err((
                err.to_string(),
                format!(
                    "run `aui install`, install Google Chrome or Chromium, or set {}=<path>",
                    launch::CHROME_ENV
                ),
            )),
        },
    );
    let endpoint = Check::new(
        "cdp",
        false,
        probe_version(cdp)
            .map(|browser| format!("{cdp} answers /json/version: {browser}"))
            .map_err(|err| {
                (
                    format!("{cdp}: {err}"),
                    "optional: start Chrome with --remote-debugging-port=9222, or use `aui run --launch`"
                        .to_owned(),
                )
            }),
    );
    let mut test_launch = Check::new("launch", true, Ok("skipped (--quick)".to_owned()));
    test_launch.skipped = quick;
    if !quick {
        test_launch = Check::new("launch", true, try_launch(found.ok()));
    }
    let socket = Check::new("socket-dir", true, socket_dir_writable());
    let toolchain = Check::new(
        "toolchain",
        false,
        Ok("not needed: aui is a prebuilt binary; Rust only builds it from source".to_owned()),
    );
    let attach = Check::new(
        "attach",
        false,
        launch::find_attach_endpoint()
            .map(|url| {
                format!("your Chrome has remote debugging on: aui open --attach reaches {url}")
            })
            .map_err(|err| {
                (
                    err.to_string(),
                    "optional: only aui open --attach needs it".into(),
                )
            }),
    );
    vec![chrome, endpoint, test_launch, socket, attach, toolchain]
}

fn try_launch(binary: Option<PathBuf>) -> Result<String, (String, String)> {
    let hint = "check the chrome line above; run `aui install` for a Chrome for Testing build";
    let Some(binary) = binary else {
        return Err(("no Chrome to launch".to_owned(), hint.to_owned()));
    };
    let options = LaunchOptions {
        binary: Some(binary),
        ..LaunchOptions::default()
    };
    let chrome = launch::launch(&options).map_err(|err| (err.to_string(), hint.to_owned()))?;
    let endpoint = chrome.endpoint();
    probe_version(&endpoint)
        .map(|browser| format!("headless {browser} answered on {endpoint}"))
        .map_err(|err| (format!("{endpoint}: {err}"), hint.to_owned()))
}

/// `$TMPDIR/aui`, where the background `aui` process keeps its socket.
fn socket_dir() -> PathBuf {
    std::env::temp_dir().join("aui")
}

fn socket_dir_writable() -> Result<String, (String, String)> {
    let dir = socket_dir();
    let hint = || "set TMPDIR to a directory you can write".to_owned();
    let mut builder = std::fs::DirBuilder::new();
    builder.recursive(true);
    #[cfg(unix)]
    std::os::unix::fs::DirBuilderExt::mode(&mut builder, 0o700);
    builder
        .create(&dir)
        .map_err(|err| (format!("{}: {err}", dir.display()), hint()))?;
    let probe = dir.join(format!(".doctor-{}", std::process::id()));
    std::fs::write(&probe, b"ok").map_err(|err| (format!("{}: {err}", dir.display()), hint()))?;
    let _ = std::fs::remove_file(&probe);
    Ok(format!("{} is writable", dir.display()))
}

/// GET `<endpoint>/json/version` and return its `Browser` field.
pub(crate) fn probe_version(endpoint: &str) -> Result<String, String> {
    let rest = endpoint
        .strip_prefix("http://")
        .ok_or_else(|| format!("`{endpoint}` is not an http:// endpoint"))?;
    let host = rest.split('/').next().unwrap_or(rest);
    let addr = host
        .to_socket_addrs()
        .map_err(|err| err.to_string())?
        .next()
        .ok_or_else(|| format!("{host} resolves to nothing"))?;
    let timeout = Duration::from_secs(2);
    let mut stream = TcpStream::connect_timeout(&addr, timeout).map_err(|err| err.to_string())?;
    stream
        .set_read_timeout(Some(timeout))
        .map_err(|err| err.to_string())?;
    let request =
        format!("GET /json/version HTTP/1.1\r\nHost: {host}\r\nConnection: close\r\n\r\n");
    stream
        .write_all(request.as_bytes())
        .map_err(|err| err.to_string())?;
    let mut buf = Vec::new();
    let mut chunk = [0u8; 4096];
    // Chrome may keep the socket open: stop at Content-Length, not EOF.
    while !response_complete(&buf) {
        match stream.read(&mut chunk) {
            Ok(0) => break,
            Ok(n) => buf.extend_from_slice(&chunk[..n]),
            Err(_) if !buf.is_empty() => break,
            Err(err) => return Err(err.to_string()),
        }
    }
    let text = String::from_utf8_lossy(&buf);
    if !text.starts_with("HTTP/1.1 200") && !text.starts_with("HTTP/1.0 200") {
        let status = text.lines().next().unwrap_or("no response");
        return Err(format!("/json/version answered `{status}`"));
    }
    json_string_field(&text, "Browser").ok_or_else(|| "no Browser in /json/version".to_owned())
}

fn response_complete(buf: &[u8]) -> bool {
    let Some(split) = buf.windows(4).position(|w| w == b"\r\n\r\n") else {
        return false;
    };
    let headers = String::from_utf8_lossy(&buf[..split]).to_ascii_lowercase();
    headers
        .lines()
        .find_map(|line| line.strip_prefix("content-length:"))
        .and_then(|n| n.trim().parse::<usize>().ok())
        .is_some_and(|length| buf.len() - split - 4 >= length)
}

fn json_string_field(body: &str, key: &str) -> Option<String> {
    let quoted = format!("\"{key}\"");
    let rest = &body[body.find(&quoted)? + quoted.len()..];
    let rest = rest.trim_start().strip_prefix(':')?.trim_start();
    let rest = rest.strip_prefix('"')?;
    Some(rest[..rest.find('"')?].to_owned())
}

fn render_text(checks: &[Check]) -> String {
    let mut out = String::new();
    for check in checks {
        let mark = match (check.skipped, check.ok) {
            (true, _) => "-",
            (false, true) => "✓",
            (false, false) => "✗",
        };
        let optional = if !check.ok && !check.required {
            " (optional)"
        } else {
            ""
        };
        out.push_str(&format!(
            "{mark} {}: {}{optional}\n",
            check.name, check.detail
        ));
        if !check.ok {
            out.push_str(&format!("    fix: {}\n", check.hint));
        }
    }
    out
}

fn render_json(checks: &[Check]) -> String {
    let ok = checks.iter().all(|c| c.ok || !c.required);
    let items: Vec<String> = checks
        .iter()
        .map(|c| {
            format!(
                "{{\"name\":\"{}\",\"ok\":{},\"required\":{},\"skipped\":{},\"detail\":\"{}\",\"hint\":\"{}\"}}",
                c.name,
                c.ok,
                c.required,
                c.skipped,
                escape(&c.detail),
                escape(&c.hint)
            )
        })
        .collect();
    format!("{{\"ok\":{ok},\"checks\":[{}]}}\n", items.join(","))
}

fn escape(text: &str) -> String {
    let mut out = String::new();
    for ch in text.chars() {
        match ch {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            ch if ch.is_control() => out.push_str(&format!("\\u{:04x}", u32::from(ch))),
            ch => out.push(ch),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn check(ok: bool, required: bool) -> Check {
        let result = if ok {
            Ok("fine".to_owned())
        } else {
            Err(("bad \"x\"".to_owned(), "do y".to_owned()))
        };
        Check::new("chrome", required, result)
    }

    #[test]
    fn text_marks_each_check_and_hints_failures() {
        let text = render_text(&[check(true, true), check(false, false), check(false, true)]);
        assert_eq!(
            text,
            "✓ chrome: fine\n✗ chrome: bad \"x\" (optional)\n    fix: do y\n✗ chrome: bad \"x\"\n    fix: do y\n"
        );
    }

    #[test]
    fn json_is_ok_only_when_required_checks_pass() {
        let json = render_json(&[check(true, true), check(false, false)]);
        let value: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(value["ok"], true);
        assert_eq!(value["checks"][1]["detail"], "bad \"x\"");
        let json = render_json(&[check(false, true)]);
        let value: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(value["ok"], false);
    }

    #[test]
    fn reads_browser_from_json_version() {
        let body = "HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\n{\"Browser\": \"Chrome/131.0\", \"Protocol-Version\":\"1.3\"}";
        assert_eq!(
            json_string_field(body, "Browser").as_deref(),
            Some("Chrome/131.0")
        );
        assert!(response_complete(body.as_bytes()));
        assert!(!response_complete(
            b"HTTP/1.1 200 OK\r\nContent-Length: 99\r\n\r\n{}"
        ));
        assert_eq!(json_string_field("{}", "Browser"), None);
    }

    #[test]
    fn probe_refuses_non_http_and_closed_ports() {
        assert!(probe_version("ws://127.0.0.1:1").is_err());
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        drop(listener);
        assert!(probe_version(&format!("http://127.0.0.1:{port}")).is_err());
    }

    #[test]
    fn doctor_rejects_unknown_and_repeated_flags() {
        assert!(doctor_command(&["--nope".to_owned()]).is_err());
        assert!(doctor_command(&["--json".to_owned(), "--json".to_owned()]).is_err());
        assert!(doctor_command(&["--cdp".to_owned()]).is_err());
    }

    #[test]
    fn socket_dir_is_under_tmpdir() {
        assert_eq!(socket_dir(), std::env::temp_dir().join("aui"));
    }
}
