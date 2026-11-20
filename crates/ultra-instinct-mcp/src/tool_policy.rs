//! The action policy file: which tools a project lets run, by tool name.
//!
//! ```json
//! {"deny": ["browser_exec", "browser_file_upload"]}
//! {"allow": ["browser_navigate", "browser_get_state", "browser_get_text"]}
//! ```
//!
//! `allow` (when present) is the whole list; `deny` takes names off it. A
//! name that is not a tool, another key, or a wrong type is an error, so a
//! typo cannot leave a tool open. A refused tool answers `tool_not_allowed`.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use serde_json::Value;

/// Env var naming the policy file; it wins over [`POLICY_FILE`] in the
/// working directory.
pub const ENV_POLICY_FILE: &str = "AUI_POLICY_FILE";
/// The file `aui` looks for in the working directory.
pub const POLICY_FILE: &str = "aui-policy.json";
/// The agent loop's name on an allow-list (not a tool, see `Server::run_goal`).
const RUN_GOAL: &str = "run_goal";

/// A parsed policy file.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ToolPolicy {
    allow: Option<BTreeSet<String>>,
    deny: BTreeSet<String>,
}

impl ToolPolicy {
    /// Parse the JSON text of a policy file.
    pub fn parse(text: &str) -> Result<Self, String> {
        let value: Value = serde_json::from_str(text).map_err(|err| err.to_string())?;
        let object = value.as_object().ok_or("the policy is not a JSON object")?;
        let mut policy = Self::default();
        for (key, names) in object {
            let names = names
                .as_array()
                .ok_or_else(|| format!("`{key}` is not a list of tool names"))?
                .iter()
                .map(|name| match name.as_str() {
                    Some(name) if name == RUN_GOAL || crate::TOOLS.contains(&name) => {
                        Ok(name.to_owned())
                    }
                    Some(name) => Err(format!("`{name}` in `{key}` is not a tool")),
                    None => Err(format!("`{key}` holds something that is not a tool name")),
                })
                .collect::<Result<BTreeSet<String>, String>>()?;
            match key.as_str() {
                "allow" => policy.allow = Some(names),
                "deny" => policy.deny = names,
                other => return Err(format!("unknown key `{other}` (use `allow` and `deny`)")),
            }
        }
        Ok(policy)
    }

    /// Read and parse `path`; the error names the file.
    pub fn read(path: &Path) -> Result<Self, String> {
        std::fs::read_to_string(path)
            .map_err(|err| err.to_string())
            .and_then(|text| Self::parse(&text))
            .map_err(|err| format!("{}: {err}", path.display()))
    }

    /// Whether the file lets `name` run.
    pub fn allows(&self, name: &str) -> bool {
        self.allow.as_ref().is_none_or(|allow| allow.contains(name)) && !self.deny.contains(name)
    }

    /// Every tool name (and `run_goal`) the file lets run.
    pub fn allowed(&self) -> impl Iterator<Item = &'static str> + '_ {
        crate::TOOLS
            .into_iter()
            .chain([RUN_GOAL])
            .filter(|name| self.allows(name))
    }
}

/// The policy file in force: [`ENV_POLICY_FILE`] when set, else
/// [`POLICY_FILE`] in `dir` when it exists. Absolute when `dir` is.
pub fn find_policy_file(dir: &Path) -> Option<PathBuf> {
    match std::env::var(ENV_POLICY_FILE) {
        Ok(named) if !named.trim().is_empty() => Some(dir.join(named)),
        _ => Some(dir.join(POLICY_FILE)).filter(|file| file.is_file()),
    }
}
