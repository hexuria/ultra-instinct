//! `aui upgrade [--version <tag>]`: replace this binary with a release build.
//!
//! Same steps as `scripts/install.sh`, in the directory the running binary
//! is in: download the archive for this build's target and the release's
//! `SHA256SUMS`, refuse on a checksum mismatch, then rename the new `aui` and
//! `ultra-instinct` over the old ones (on Windows the running old ones are
//! renamed aside first). The tree carries no HTTP client: the
//! download is `curl`, the unpack is `tar`, the digest is `sha256sum`,
//! `shasum` or Windows' `certutil`.

use std::path::{Path, PathBuf};
use std::process::Command;

use crate::CliError;

const RELEASES: &str = "https://github.com/hexuria/ultra-instinct/releases";
/// Overrides [`RELEASES`]; the tests point it at a release on disk.
const BASE_ENV: &str = "AUI_BASE_URL";
/// The archive's (and install dir's) binary names; `.exe` on Windows.
#[cfg(windows)]
const BINARIES: [&str; 2] = ["aui.exe", "ultra-instinct.exe"];
#[cfg(not(windows))]
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
        // Only the x86_64 MSVC build ships for Windows.
        "windows" if arch == "x86_64" => "pc-windows-msvc",
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
    if let Ok(out) = run("sha256sum", &[file]).or_else(|_| run("shasum", &["-a", "256", file])) {
        return Ok(out.split_whitespace().next().unwrap_or_default().to_owned());
    }
    // Windows' built-in digest tool; the hex digest is its own output line,
    // spaced or not depending on the OS version.
    let out = run("certutil", &["-hashfile", file, "SHA256"])?;
    Ok(out
        .lines()
        .map(|line| {
            line.chars()
                .filter(|c| !c.is_whitespace())
                .collect::<String>()
        })
        .find(|line| line.len() == 64 && line.bytes().all(|b| b.is_ascii_hexdigit()))
        .unwrap_or_default()
        .to_owned())
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
    // As root, tar would otherwise restore the archive's (build user's) owner.
    run(
        "tar",
        &["-xzf", &path(&archive), "-C", &path(""), "--no-same-owner"],
    )?;
    Ok(scratch.join(format!("aui-{target}")))
}

/// Rename `new` over `old`. Windows cannot overwrite a running image (this
/// process's own, or a session daemon's) but can rename it, so the old file
/// moves into `scratch` first and stays there until nothing runs it.
fn replace(new: &Path, old: &Path, scratch: &Path) -> Result<(), CliError> {
    let cannot = |err: std::io::Error| fail(format!("cannot replace {}: {err}", old.display()));
    if cfg!(windows) {
        let name = old.file_name().unwrap_or_default().to_string_lossy();
        let aside = scratch.join(format!("{name}.old"));
        std::fs::rename(old, &aside).map_err(cannot)?;
        if let Err(err) = std::fs::rename(new, old) {
            let _ = std::fs::rename(&aside, old);
            return Err(cannot(err));
        }
        return Ok(());
    }
    std::fs::rename(new, old).map_err(cannot)
}

/// Hold `<dir>/.aui-upgrade.lock`; another upgrade holding it is an error.
fn upgrade_lock(dir: &Path) -> Result<std::fs::File, CliError> {
    let path = dir.join(".aui-upgrade.lock");
    let file = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(&path)
        .map_err(|err| fail(format!("cannot write to {}: {err}", dir.display())))?;
    match file.try_lock() {
        Ok(()) => Ok(file),
        Err(std::fs::TryLockError::WouldBlock) => Err(fail(format!(
            "another aui upgrade is running in {}",
            dir.display()
        ))),
        Err(std::fs::TryLockError::Error(err)) => {
            Err(fail(format!("cannot lock {}: {err}", path.display())))
        }
    }
}

/// Scratch dirs an earlier Windows upgrade left behind: the old binaries a
/// running process still held. Best-effort; a still-held one stays.
fn remove_stale_scratch(dir: &Path) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.filter_map(Result::ok) {
        if entry
            .file_name()
            .to_string_lossy()
            .starts_with(".aui-upgrade-")
        {
            let _ = std::fs::remove_dir_all(entry.path());
        }
    }
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
    // Only what is already installed here; `aui` alone stays alone.
    let installed: Vec<&str> = BINARIES
        .into_iter()
        .filter(|bin| dir.join(bin).is_file())
        .collect();
    if installed.is_empty() {
        return Err(fail(format!(
            "no aui or ultra-instinct in {}; nothing to replace",
            dir.display()
        )));
    }
    // Windows: one upgrade per install dir at a time, so the cleanup below
    // only ever finds scratch dirs of finished upgrades. The OS drops the
    // lock when this process exits, however it exits.
    let _lock = if cfg!(windows) {
        let lock = upgrade_lock(dir)?;
        remove_stale_scratch(dir);
        Some(lock)
    } else {
        None
    };
    // Beside the binaries, so the final rename stays on one filesystem.
    let scratch = dir.join(format!(".aui-upgrade-{}", std::process::id()));
    std::fs::create_dir(&scratch)
        .map_err(|err| fail(format!("cannot write to {}: {err}", dir.display())))?;
    let done = fetch(&url, &target, &scratch).and_then(|unpacked| {
        let before = env!("CARGO_PKG_VERSION");
        for bin in &installed {
            replace(&unpacked.join(bin), &dir.join(bin), &scratch)?;
        }
        let after = run(
            &dir.join(installed[0]).display().to_string(),
            &["--version"],
        )?;
        Ok(format!(
            "upgraded {} in {}: {before} -> {}\n",
            installed.join(" + "),
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
                    "x86_64-pc-windows-msvc",
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
    fn a_second_upgrade_waits_out_the_first_before_cleaning_scratch() {
        let dir = std::env::temp_dir().join(format!("aui-upgrade-lock-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let first = upgrade_lock(&dir).unwrap();
        // The first upgrade's backup sits in its scratch dir, mid-replace.
        let scratch = dir.join(".aui-upgrade-1");
        std::fs::create_dir(&scratch).unwrap();
        let second = upgrade_lock(&dir);
        assert!(second.is_err(), "a concurrent upgrade took the lock");
        assert!(scratch.exists());
        drop(first);
        let _third = upgrade_lock(&dir).unwrap();
        remove_stale_scratch(&dir);
        assert!(!scratch.exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn upgrade_rejects_other_arguments() {
        for args in [&["--force"][..], &["--version"], &["--version", "v1", "x"]] {
            let args: Vec<String> = args.iter().map(|arg| (*arg).to_owned()).collect();
            assert!(upgrade_command(&args).is_err(), "{args:?}");
        }
    }
}
