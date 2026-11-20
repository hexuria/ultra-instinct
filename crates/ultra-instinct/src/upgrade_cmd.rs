//! `aui upgrade [--version <tag>]`: replace this binary with a release build.
//!
//! Same steps as `scripts/install.sh`, in the directory the running binary
//! is in: download the archive for this build's target and the release's
//! `SHA256SUMS`, refuse on a checksum mismatch, then rename the new `aui` and
//! `ultra-instinct` over the old ones. The tree carries no HTTP client: the
//! download is `curl`, the unpack is `tar`, the digest is `sha256sum` or
//! `shasum`.

use std::path::{Path, PathBuf};
use std::process::Command;

use crate::CliError;

const RELEASES: &str = "https://github.com/hexuria/ultra-instinct/releases";
/// Overrides [`RELEASES`]; the tests point it at a release on disk.
const BASE_ENV: &str = "AUI_BASE_URL";
const BINARIES: [&str; 2] = ["aui", "ultra-instinct"];

/// The release target this binary was built for, when a release has one.
fn target() -> Option<String> {
    let arch = match std::env::consts::ARCH {
        arch @ ("x86_64" | "aarch64") => arch,
        _ => return None,
    };
    let os = match std::env::consts::OS {
        "macos" => "apple-darwin",
        "linux" if cfg!(target_env = "musl") => "unknown-linux-musl",
        "linux" => "unknown-linux-gnu",
        _ => return None,
    };
    Some(format!("{arch}-{os}"))
}

fn fail(what: impl Into<String>) -> CliError {
    CliError::Config(format!("upgrade: {}", what.into()))
}

/// Run `program`, return its stdout; a non-zero exit is an error naming it.
fn run(program: &str, args: &[&str]) -> Result<String, CliError> {
    let out = Command::new(program)
        .args(args)
        .output()
        .map_err(|err| fail(format!("cannot run {program}: {err}")))?;
    if !out.status.success() {
        let stderr = String::from_utf8_lossy(&out.stderr);
        return Err(fail(format!("{program} failed: {}", stderr.trim())));
    }
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

fn sha256(file: &str) -> Result<String, CliError> {
    let out = run("sha256sum", &[file]).or_else(|_| run("shasum", &["-a", "256", file]))?;
    Ok(out.split_whitespace().next().unwrap_or_default().to_owned())
}

/// The digest `SHA256SUMS` lists for `archive` (`<hex>  <name>` lines).
fn listed(sums: &str, archive: &str) -> Option<String> {
    sums.lines().find_map(|line| {
        let mut words = line.split_whitespace();
        let digest = words.next()?;
        let name = words.next()?;
        (name.trim_start_matches('*') == archive).then(|| digest.to_owned())
    })
}

/// Download, verify and unpack `archive` into `scratch`; the unpacked
/// directory. Nothing outside `scratch` is touched.
fn fetch(url: &str, target: &str, scratch: &Path) -> Result<PathBuf, CliError> {
    let archive = format!("aui-{target}.tar.gz");
    let path = |name: &str| scratch.join(name).display().to_string();
    for name in [archive.as_str(), "SHA256SUMS"] {
        run(
            "curl",
            &[
                "-fsSL",
                "--proto",
                "=https,file",
                "-o",
                &path(name),
                &format!("{url}/{name}"),
            ],
        )?;
    }
    let sums = std::fs::read_to_string(scratch.join("SHA256SUMS"))
        .map_err(|err| fail(format!("SHA256SUMS: {err}")))?;
    let got = sha256(&path(&archive))?;
    match listed(&sums, &archive) {
        Some(want) if want == got => {}
        want => {
            return Err(fail(format!(
                "checksum mismatch for {archive} (expected {}, got {got}); nothing was replaced",
                want.as_deref().unwrap_or("no SHA256SUMS entry")
            )));
        }
    }
    run("tar", &["-xzf", &path(&archive), "-C", &path("")])?;
    Ok(scratch.join(format!("aui-{target}")))
}

pub(crate) fn upgrade_command(args: &[String]) -> Result<String, CliError> {
    let version = match args {
        [] => None,
        [flag, tag] if flag == "--version" => Some(tag.as_str()),
        _ => return Err(fail("usage: aui upgrade [--version <tag>]")),
    };
    let target = target().ok_or_else(|| {
        fail("no release binary for this platform; build from source with scripts/install-aui.sh")
    })?;
    let exe = std::env::current_exe()
        .and_then(std::fs::canonicalize)
        .map_err(|err| fail(format!("cannot find the running binary: {err}")))?;
    let dir = exe
        .parent()
        .ok_or_else(|| fail("the running binary has no directory"))?;
    let base = std::env::var(BASE_ENV).unwrap_or_else(|_| RELEASES.to_owned());
    let url = match version {
        Some(tag) => format!("{base}/download/{tag}"),
        None => format!("{base}/latest/download"),
    };
    // Beside the binaries, so the final rename stays on one filesystem.
    let scratch = dir.join(format!(".aui-upgrade-{}", std::process::id()));
    std::fs::create_dir(&scratch)
        .map_err(|err| fail(format!("cannot write to {}: {err}", dir.display())))?;
    let done = fetch(&url, &target, &scratch).and_then(|unpacked| {
        let before = env!("CARGO_PKG_VERSION");
        let mut replaced = Vec::new();
        for bin in BINARIES {
            let old = dir.join(bin);
            // Only what is already installed here; `aui` alone stays alone.
            if old.is_file() {
                std::fs::rename(unpacked.join(bin), &old)
                    .map_err(|err| fail(format!("cannot replace {}: {err}", old.display())))?;
                replaced.push(bin);
            }
        }
        let after = run(&dir.join(replaced[0]).display().to_string(), &["--version"])?;
        Ok(format!(
            "upgraded {} in {}: {before} -> {}\n",
            replaced.join(" + "),
            dir.display(),
            after.trim().rsplit(' ').next().unwrap_or_default()
        ))
    });
    let _ = std::fs::remove_dir_all(&scratch);
    done
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_target_names_a_release_archive() {
        if let Some(target) = target() {
            assert!(
                [
                    "aarch64-apple-darwin",
                    "x86_64-apple-darwin",
                    "x86_64-unknown-linux-gnu",
                    "aarch64-unknown-linux-gnu",
                    "x86_64-unknown-linux-musl",
                    "aarch64-unknown-linux-musl",
                ]
                .contains(&target.as_str()),
                "{target}"
            );
        }
    }

    #[test]
    fn the_listed_digest_is_the_one_on_the_archive_line() {
        let sums = "aa  aui-a.tar.gz\nbb *aui-b.tar.gz\n";
        assert_eq!(listed(sums, "aui-a.tar.gz").as_deref(), Some("aa"));
        assert_eq!(listed(sums, "aui-b.tar.gz").as_deref(), Some("bb"));
        assert_eq!(listed(sums, "aui-c.tar.gz"), None);
        assert_eq!(listed(sums, "a.tar.gz"), None);
    }

    #[test]
    fn upgrade_rejects_other_arguments() {
        for args in [&["--force"][..], &["--version"], &["--version", "v1", "x"]] {
            let args: Vec<String> = args.iter().map(|arg| (*arg).to_owned()).collect();
            assert!(upgrade_command(&args).is_err(), "{args:?}");
        }
    }
}
