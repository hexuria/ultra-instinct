//! `aui skills list | get <name> [--ref <ref>]`: agent guides that ship inside the binary.
//!
//! The guides live in `crates/ultra-instinct/skills/` and are embedded with
//! `include_str!`, so a crates.io install prints the same text as a
//! checkout. `skills/ultra-instinct/SKILL.md` in the repo is a stub that
//! sends the agent here, so what it reads always matches the binary.

use crate::CliError;

/// Embedded guides: (name, text).
pub const SKILLS: [(&str, &str); 1] = [("core", include_str!("../skills/core.md"))];

/// References of a guide, read on demand: (guide, ref, what it covers, text).
/// The guide stays short; `aui skills get <guide> --ref <ref>` prints one.
pub const REFS: [(&str, &str, &str, &str); 3] = [
    (
        "core",
        "commands",
        "every shell verb and flag",
        include_str!("../skills/commands.md"),
    ),
    (
        "core",
        "safety",
        "the allow-list and confirm rules",
        include_str!("../skills/safety.md"),
    ),
    (
        "core",
        "sessions",
        "session names, launch / cdp / profile, the user's own Chrome",
        include_str!("../skills/sessions.md"),
    ),
];

const USAGE: &str = "usage: aui skills list | aui skills get <name> [--ref <ref>]";

fn guide(name: &str) -> Result<&'static str, CliError> {
    SKILLS
        .iter()
        .find(|(known, _)| *known == name)
        .map(|(_, text)| *text)
        .ok_or_else(|| {
            CliError::Config(format!(
                "unknown skill `{name}` (aui skills list: {})",
                SKILLS.map(|(n, _)| n).join(", ")
            ))
        })
}

fn reference(name: &str, wanted: &str) -> Result<&'static str, CliError> {
    guide(name)?;
    let of_guide = REFS.iter().filter(|(guide, ..)| *guide == name);
    of_guide
        .clone()
        .find(|(_, known, ..)| *known == wanted)
        .map(|(.., text)| *text)
        .ok_or_else(|| {
            let known: Vec<&str> = of_guide.map(|(_, known, ..)| *known).collect();
            CliError::Config(format!(
                "unknown ref `{wanted}` of `{name}` (refs: {})",
                known.join(", ")
            ))
        })
}

pub(crate) fn skills_command(args: &[String]) -> Result<String, CliError> {
    let args: Vec<&str> = args.iter().map(String::as_str).collect();
    match args[..] {
        ["list"] => {
            let mut out = String::new();
            for (name, _) in SKILLS {
                out.push_str(name);
                out.push('\n');
                for (_, known, covers, _) in REFS.iter().filter(|(guide, ..)| *guide == name) {
                    out.push_str(&format!("{name} --ref {known}  {covers}\n"));
                }
            }
            Ok(out)
        }
        ["get", name] => guide(name).map(str::to_owned),
        ["get", name, "--ref", wanted] | ["get", "--ref", wanted, name] => {
            reference(name, wanted).map(str::to_owned)
        }
        _ => Err(CliError::Config(USAGE.into())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn a(raw: &[&str]) -> Vec<String> {
        raw.iter().map(|s| (*s).to_owned()).collect()
    }

    #[test]
    fn list_names_and_get_prints_the_guide() {
        let list = skills_command(&a(&["list"])).unwrap();
        assert!(list.starts_with("core\ncore --ref commands  "), "{list}");
        for name in ["commands", "safety", "sessions"] {
            assert!(list.contains(&format!("core --ref {name}  ")), "{list}");
            // The core guide routes to every reference it has.
            let route = format!("aui skills get core --ref {name}");
            assert!(
                skills_command(&a(&["get", "core"]))
                    .unwrap()
                    .contains(&route),
                "{route}"
            );
            let text = skills_command(&a(&["get", "core", "--ref", name])).unwrap();
            assert!(text.starts_with("# ultra-instinct: "), "{text}");
        }
        let commands = skills_command(&a(&["get", "core", "--ref", "commands"])).unwrap();
        for verb in ["| `drag <ref> <ref>", "`tab new [url]`", "aui batch"] {
            assert!(commands.contains(verb), "{verb}");
        }
        assert!(skills_command(&a(&["get", "core", "--ref", "nope"])).is_err());
        assert!(skills_command(&a(&["get", "nope", "--ref", "commands"])).is_err());
        let core = skills_command(&a(&["get", "core"])).unwrap();
        assert!(core.starts_with("# ultra-instinct"), "{core}");
        for needle in [
            "aui snap",
            "@e",
            "aui wait",
            "--allow-domain",
            "--confirm-press",
        ] {
            assert!(core.contains(needle), "core guide mentions {needle}");
        }
        assert!(skills_command(&a(&["get", "nope"])).is_err());
        assert!(skills_command(&a(&[])).is_err());
        assert!(skills_command(&a(&["get"])).is_err());
    }
}
