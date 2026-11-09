//! `aui skills list | get <name>`: agent guides that ship inside the binary.
//!
//! The guides live in `crates/ultra-instinct/skills/` and are embedded with
//! `include_str!`, so a crates.io install prints the same text as a
//! checkout. `skills/ultra-instinct/SKILL.md` in the repo gives the first steps and
//! sends the agent here for the rest.

use crate::CliError;

/// Embedded guides: (name, text).
pub const SKILLS: [(&str, &str); 1] = [("core", include_str!("../skills/core.md"))];

pub(crate) fn skills_command(args: &[String]) -> Result<String, CliError> {
    match args {
        [list] if list == "list" => {
            Ok(SKILLS.iter().map(|(name, _)| format!("{name}\n")).collect())
        }
        [get, name] if get == "get" => SKILLS
            .iter()
            .find(|(known, _)| known == name)
            .map(|(_, text)| (*text).to_owned())
            .ok_or_else(|| {
                CliError::Config(format!(
                    "unknown skill `{name}` (aui skills list: {})",
                    SKILLS.map(|(n, _)| n).join(", ")
                ))
            }),
        _ => Err(CliError::Config(
            "usage: aui skills list | aui skills get <name>".into(),
        )),
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
        assert_eq!(skills_command(&a(&["list"])).unwrap(), "core\n");
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
