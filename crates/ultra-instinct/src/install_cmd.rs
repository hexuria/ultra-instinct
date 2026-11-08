//! `aui install [--run]`: how to get a Chrome for Testing build aui finds.
//!
//! The tree carries no download code and no HTTP client. Without `--run`
//! this prints the `@puppeteer/browsers` command for this machine; with
//! `--run` (and `npx` on `PATH`) it runs that command with inherited stdio,
//! then `aui doctor`.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Command;

use ultra_instinct_browser::launch;

use crate::CliError;

/// The `npx` argv that installs Chrome for Testing into `dir`.
fn install_argv(dir: &Path) -> Vec<String> {
    [
        "npx",
        "@puppeteer/browsers",
        "install",
        "chrome@stable",
        "--path",
    ]
    .iter()
    .map(|s| (*s).to_owned())
    .chain([dir.display().to_string()])
    .collect()
}

/// One copy-pasteable line: arguments with spaces are quoted for this shell.
fn shell_line(argv: &[String]) -> String {
    let quote = if cfg!(windows) { '"' } else { '\'' };
    argv.iter()
        .map(
            |arg| match arg.contains([' ', '\'', '"', '$', '&', '(', ')']) {
                true => format!("{quote}{arg}{quote}"),
                false => arg.clone(),
            },
        )
        .collect::<Vec<_>>()
        .join(" ")
}

fn on_path(names: &[&str]) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path)
        .flat_map(|dir| names.iter().map(move |name| dir.join(name)))
        .find(|candidate| candidate.is_file())
}

pub(crate) fn install_command(args: &[String]) -> Result<String, CliError> {
    let mut run = false;
    for arg in args {
        match arg.as_str() {
            "--run" if run => return Err(CliError::DuplicateFlag("--run")),
            "--run" => run = true,
            other => return Err(CliError::UnknownFlag(other.to_owned())),
        }
    }
    let dir = launch::chrome_cache_dir()
        .ok_or_else(|| CliError::Config("HOME (or USERPROFILE) is not set".to_owned()))?;
    let argv = install_argv(&dir);
    let mut out = format!(
        "Install Chrome for Testing where aui looks first:\n  {}\n\
         A system Google Chrome or Chromium works too; `aui doctor` shows which one aui finds.\n\
         Any other binary: set {}=<path>.\n",
        shell_line(&argv),
        launch::CHROME_ENV
    );
    if !run {
        out.push_str("Or let aui run it (needs Node.js): aui install --run\n");
        return Ok(out);
    }
    let npx = on_path(&["npx", "npx.cmd"]).ok_or_else(|| {
        CliError::Config(
            "npx is not on PATH; install Node.js, or install Google Chrome, then run `aui doctor`"
                .to_owned(),
        )
    })?;
    print!("{out}");
    let _ = std::io::stdout().flush();
    let status = Command::new(npx)
        .args(&argv[1..])
        .status()
        .map_err(|err| CliError::Config(format!("cannot run npx: {err}")))?;
    if !status.success() {
        return Err(CliError::Config(format!(
            "chrome install failed ({status})"
        )));
    }
    crate::doctor_cmd::doctor_command(&[])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prints_the_puppeteer_browsers_command_for_the_cache_dir() {
        let argv = install_argv(Path::new("/home/me/.cache/ultra-instinct/chrome"));
        assert_eq!(
            shell_line(&argv),
            "npx @puppeteer/browsers install chrome@stable --path /home/me/.cache/ultra-instinct/chrome"
        );
        let spaced = shell_line(&install_argv(Path::new("/Users/A B/.cache")));
        assert!(spaced.ends_with(if cfg!(windows) {
            "\"/Users/A B/.cache\""
        } else {
            "'/Users/A B/.cache'"
        }));
    }

    #[test]
    fn install_without_run_only_prints() {
        let out = install_command(&[]).unwrap();
        assert!(out.contains("npx @puppeteer/browsers install chrome@stable --path "));
        assert!(out.contains(".cache/ultra-instinct/chrome"));
        assert!(out.contains("system Google Chrome"));
    }

    #[test]
    fn install_rejects_other_flags() {
        assert!(install_command(&["--yes".to_owned()]).is_err());
        assert!(install_command(&["--run".to_owned(), "--run".to_owned()]).is_err());
    }
}
