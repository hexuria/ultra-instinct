//! `aui batch "<verb args>" …`: several shell verbs against one session, in
//! order, through the same verb code as one-at-a-time calls.
//!
//! With no command arguments the commands are a JSON array of strings on
//! stdin. Each string is split shell-style ([`split_words`]). `--session`,
//! `--cdp` and `--max-output` on `batch` reach every command; a command
//! naming its own `--session` or `--cdp` fails (one batch, one session).
//! `--bail` stops at the first failure; any failure makes the exit
//! non-zero. `--json` prints `[{cmd, ok, output}]`.

use serde_json::{json, Value};

use crate::verbs::{Flags, VERBS};
use crate::CliError;

/// Split one command line into words like a POSIX shell, without
/// expansion: whitespace separates words; `'…'` is literal; `"…"` is
/// literal except `\"`, `\\`, `\$` and `` \` ``; a bare `\` escapes the next
/// character. An unclosed quote or a trailing `\` is an error.
pub fn split_words(line: &str) -> Result<Vec<String>, String> {
    let mut words = Vec::new();
    let mut word: Option<String> = None;
    let mut chars = line.chars();
    while let Some(c) = chars.next() {
        match c {
            c if c.is_whitespace() => {
                if let Some(done) = word.take() {
                    words.push(done);
                }
            }
            '\'' => {
                let current = word.get_or_insert_with(String::new);
                loop {
                    match chars.next() {
                        Some('\'') => break,
                        Some(c) => current.push(c),
                        None => return Err(format!("unclosed ' in `{line}`")),
                    }
                }
            }
            '"' => {
                let current = word.get_or_insert_with(String::new);
                loop {
                    match chars.next() {
                        Some('"') => break,
                        Some('\\') => match chars.next() {
                            Some(c @ ('"' | '\\' | '$' | '`')) => current.push(c),
                            Some(c) => {
                                current.push('\\');
                                current.push(c);
                            }
                            None => return Err(format!("unclosed \" in `{line}`")),
                        },
                        Some(c) => current.push(c),
                        None => return Err(format!("unclosed \" in `{line}`")),
                    }
                }
            }
            '\\' => match chars.next() {
                Some(c) => word.get_or_insert_with(String::new).push(c),
                None => return Err(format!("trailing \\ in `{line}`")),
            },
            c => word.get_or_insert_with(String::new).push(c),
        }
    }
    if let Some(done) = word {
        words.push(done);
    }
    Ok(words)
}

/// What `batch` passes to every command.
#[derive(Debug, Default)]
struct Shared {
    flags: Vec<String>,
    bail: bool,
    json: bool,
}

/// `aui batch …`.
pub(crate) fn batch_command(args: &[String]) -> Result<String, CliError> {
    let (shared, commands) = parse(args)?;
    let commands = if commands.is_empty() {
        let mut input = String::new();
        std::io::Read::read_to_string(&mut std::io::stdin(), &mut input).map_err(|err| {
            CliError::Io {
                path: "stdin".into(),
                message: err.to_string(),
            }
        })?;
        serde_json::from_str::<Vec<String>>(&input).map_err(|err| {
            CliError::Config(format!(
                "aui batch with no commands reads a JSON array of strings on stdin: {err}"
            ))
        })?
    } else {
        commands
    };
    run(&shared, &commands, crate::daemon::verb_command)
}

fn parse(args: &[String]) -> Result<(Shared, Vec<String>), CliError> {
    let mut flags = Flags::scan(args)?;
    let mut shared = Shared {
        bail: flags.switch("--bail"),
        json: flags.switch("--json"),
        ..Shared::default()
    };
    for flag in ["--session", "--cdp", "--max-output"] {
        if let Some(value) = flags.one(flag)? {
            shared.flags.extend([flag.to_owned(), value]);
        }
    }
    flags.finish()?;
    Ok((shared, flags.positional()))
}

/// Run `commands` in order through `verb`; see the module docs.
fn run(
    shared: &Shared,
    commands: &[String],
    mut verb: impl FnMut(&str, &[String]) -> Result<String, CliError>,
) -> Result<String, CliError> {
    if commands.is_empty() {
        return Err(CliError::Config(
            "usage: aui batch [--bail] [--json] \"<verb args>\"... (or a JSON array on stdin)"
                .into(),
        ));
    }
    let mut results: Vec<(String, Result<String, String>)> = Vec::new();
    for command in commands {
        let outcome = one(shared, command, &mut verb).map_err(|err| err.to_string());
        let failed = outcome.is_err();
        results.push((command.clone(), outcome));
        if failed && shared.bail {
            break;
        }
    }
    let failed = results.iter().filter(|(_, r)| r.is_err()).count();
    let body = if shared.json {
        let items: Vec<Value> = results
            .iter()
            .map(|(cmd, outcome)| {
                let (ok, output) = match outcome {
                    Ok(out) => (true, out.trim_end().to_owned()),
                    Err(err) => (false, err.clone()),
                };
                json!({"cmd": cmd, "ok": ok, "output": output})
            })
            .collect();
        format!("{}\n", Value::Array(items))
    } else {
        let mut out = String::new();
        for (_, outcome) in &results {
            match outcome {
                Ok(text) => out.push_str(text),
                Err(err) => out.push_str(&format!("{err}\n")),
            }
            if !out.ends_with('\n') {
                out.push('\n');
            }
        }
        if failed > 0 {
            let skipped = commands.len() - results.len();
            out.push_str(&format!("✗ {failed} of {} failed", commands.len()));
            if skipped > 0 {
                out.push_str(&format!(" · {skipped} not run (--bail)"));
            }
            out.push('\n');
        }
        out
    };
    if failed > 0 {
        return Err(CliError::BatchFailed(body));
    }
    Ok(body)
}

fn one(
    shared: &Shared,
    command: &str,
    verb: &mut impl FnMut(&str, &[String]) -> Result<String, CliError>,
) -> Result<String, CliError> {
    let words = split_words(command).map_err(CliError::Config)?;
    let Some((name, rest)) = words.split_first() else {
        return Err(CliError::Config("empty batch command".into()));
    };
    if !VERBS.contains(&name.as_str()) {
        return Err(CliError::UnknownCommand(name.clone()));
    }
    for own in ["--session", "--cdp"] {
        let prefix = format!("{own}=");
        if rest.iter().any(|w| w == own || w.starts_with(&prefix)) {
            return Err(CliError::Config(format!(
                "a batch runs on one session: put {own} on `aui batch`, not on `{name}`"
            )));
        }
    }
    let mut args = rest.to_vec();
    // `--max-output` on the command wins over the batch's.
    let own_max = rest
        .iter()
        .any(|w| w == "--max-output" || w.starts_with("--max-output="));
    for pair in shared.flags.chunks(2) {
        if pair[0] == "--max-output" && own_max {
            continue;
        }
        args.extend(pair.iter().cloned());
    }
    verb(name, &args)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn w(line: &str) -> Vec<String> {
        split_words(line).unwrap()
    }

    #[test]
    fn words_split_like_a_shell() {
        assert_eq!(w("click @e5"), ["click", "@e5"]);
        assert_eq!(w("  type  @e2   kid  "), ["type", "@e2", "kid"]);
        assert_eq!(
            w(r#"type @e2 "kid smith@example.com""#),
            ["type", "@e2", "kid smith@example.com"]
        );
        assert_eq!(w("type @e2 'a \"b\" $c'"), ["type", "@e2", "a \"b\" $c"]);
        assert_eq!(
            w(r#"key "a\"b" "c\\d" "e\nf""#),
            ["key", "a\"b", "c\\d", "e\\nf"]
        );
        assert_eq!(w(r"type @e1 a\ b"), ["type", "@e1", "a b"]);
        assert_eq!(w(r#"x a"b c"d"#), ["x", "ab cd"]);
        assert_eq!(w(r#"dialog accept """#), ["dialog", "accept", ""]);
        assert_eq!(
            w("click --confirm='Pay $18' @e5"),
            ["click", "--confirm=Pay $18", "@e5"]
        );
        assert!(w("").is_empty());
        for bad in ["type 'x", "type \"x", "type x\\", "type \"x\\"] {
            assert!(split_words(bad).is_err(), "{bad}");
        }
    }

    type Log = std::rc::Rc<std::cell::RefCell<Vec<Vec<String>>>>;

    /// A fake verb runner: `fail` fails, anything else echoes its words.
    fn runner(log: Log) -> impl FnMut(&str, &[String]) -> Result<String, CliError> {
        move |verb, args| {
            let mut words = vec![verb.to_owned()];
            words.extend(args.iter().cloned());
            log.borrow_mut().push(words.clone());
            if args.first().is_some_and(|a| a == "fail") {
                return Err(CliError::Refused(
                    "✗ refused: TargetGone · run aui snap".into(),
                ));
            }
            Ok(format!("ran {}\n", words.join(" ")))
        }
    }

    fn cmds(raw: &[&str]) -> Vec<String> {
        raw.iter().map(|s| (*s).to_owned()).collect()
    }

    #[test]
    fn commands_run_in_order_with_the_batch_session() {
        let (shared, commands) = parse(&cmds(&[
            "--session",
            "shop",
            "snap",
            "click @e5 --confirm \"Pay $18\"",
        ]))
        .unwrap();
        let log = Log::default();
        let out = run(&shared, &commands, runner(log.clone())).unwrap();
        assert_eq!(
            *log.borrow(),
            [
                cmds(&["snap", "--session", "shop"]),
                cmds(&["click", "@e5", "--confirm", "Pay $18", "--session", "shop"]),
            ]
        );
        assert_eq!(
            out,
            "ran snap --session shop\nran click @e5 --confirm Pay $18 --session shop\n"
        );
    }

    #[test]
    fn a_failure_fails_the_batch_and_bail_stops_it() {
        let commands = cmds(&["snap", "click fail", "snap"]);
        let log = Log::default();
        let err = run(&Shared::default(), &commands, runner(log.clone())).unwrap_err();
        assert_eq!(log.borrow().len(), 3, "without --bail every command runs");
        let CliError::BatchFailed(body) = err else {
            panic!("{err:?}")
        };
        assert!(body.contains("✗ refused: TargetGone"), "{body}");
        assert!(body.ends_with("✗ 1 of 3 failed\n"), "{body}");

        let log = Log::default();
        let bail = Shared {
            bail: true,
            ..Shared::default()
        };
        let err = run(&bail, &commands, runner(log.clone())).unwrap_err();
        assert_eq!(log.borrow().len(), 2, "--bail stops at the failure");
        assert!(err
            .to_string()
            .ends_with("✗ 1 of 3 failed · 1 not run (--bail)\n"));
    }

    #[test]
    fn json_lists_each_command_and_its_outcome() {
        let shared = Shared {
            json: true,
            ..Shared::default()
        };
        let err = run(
            &shared,
            &cmds(&["snap", "click fail", "fly away"]),
            runner(Log::default()),
        )
        .unwrap_err();
        let CliError::BatchFailed(body) = err else {
            panic!("{err:?}")
        };
        let items: Value = serde_json::from_str(&body).unwrap();
        assert_eq!(
            items,
            json!([
                {"cmd": "snap", "ok": true, "output": "ran snap"},
                {"cmd": "click fail", "ok": false, "output": "✗ refused: TargetGone · run aui snap"},
                {"cmd": "fly away", "ok": false, "output": "unknown command `fly`"},
            ])
        );
        let ok = run(&shared, &cmds(&["snap"]), runner(Log::default())).unwrap();
        assert_eq!(
            serde_json::from_str::<Value>(&ok).unwrap(),
            json!([{"cmd": "snap", "ok": true, "output": "ran snap"}])
        );
    }

    #[test]
    fn a_command_cannot_name_another_session() {
        let log = Log::default();
        for bad in [
            "snap --session other",
            "snap --cdp=http://h:1",
            "",
            "batch snap",
            "type 'x",
        ] {
            assert!(
                run(&Shared::default(), &cmds(&[bad]), runner(log.clone())).is_err(),
                "{bad}"
            );
        }
        assert!(log.borrow().is_empty(), "nothing ran");
        assert!(run(&Shared::default(), &[], runner(log.clone())).is_err());
    }

    #[test]
    fn max_output_reaches_every_command_unless_it_sets_its_own() {
        let (shared, commands) =
            parse(&cmds(&["--max-output=500", "snap", "snap --max-output 50"])).unwrap();
        let log = Log::default();
        run(&shared, &commands, runner(log.clone())).unwrap();
        assert_eq!(
            *log.borrow(),
            [
                cmds(&["snap", "--max-output", "500"]),
                cmds(&["snap", "--max-output", "50"]),
            ]
        );
        assert!(parse(&cmds(&["--reveal", "snap"])).is_err());
    }
}
